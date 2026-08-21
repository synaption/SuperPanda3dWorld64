//! The things that can be fought, and how they resolve against each other.
//!
//! Two sides: the enemies, and the player with his Marios. Both sides notice
//! each other the same way, pass the alarm along the same way, and walk to
//! whatever they have noticed the same way -- see [`Side`] and [`alert`] -- and
//! then each resolves what it does on arrival in its own terms, because a
//! sword, a punch and walking into a goomba are three different things.
//!
//! The player's half of the combat rules is ported from `Interactions.resolve`
//! in `sm64py/objects.py`, and every distance is that build's, converted from
//! SM64 units to the port's world scale of 1/100.

use crate::{
    audio::{Sfx, SoundQueue},
    console::{ConsoleState, CrowdKind, GameTuning, Request},
    level::LevelData,
    player::{Controller, Motion, Player, FIXED_DT, PLAYER_HEIGHT},
    squad::Ally,
};
use bevy::{platform::collections::HashMap, prelude::*};

/// How far above an enemy's own feet counts as landing on top of it, as a
/// fraction of its height.
const STOMP_MARGIN: f32 = 0.6;

/// How far the player's body reaches in the horizontal plane for the purpose
/// of touching an enemy, which is not the same as the radius he is pushed out
/// of walls with.
const PLAYER_REACH: f32 = 0.37;

/// How far a swing reaches. Wider than a touch on purpose: the Hero swings a
/// sword, and a weapon that only hits what is already standing on him is not a
/// weapon.
const ATTACK_REACH: f32 = 2.2;

/// Up off a stomped enemy, and back off one that got a hit in.
const BOUNCE_VELOCITY: f32 = 12.6;
const KNOCKBACK_SPEED: f32 = 7.2;
const KNOCKBACK_RISE: f32 = 6.0;

/// How long the player is immune after a hit -- 30 frames at 30 Hz in the
/// original. Long enough for the knockback to carry him clear of whatever hit
/// him, which is the entire point of it.
const INVULNERABLE_SECONDS: f32 = 1.0;

/// How fast an enemy ambles about with nobody to chase, which is not how fast
/// it comes for you once it has noticed you -- that is `enemy_speed`.
const WANDER_SPEED: f32 = 1.2;

/// The amble an enemy falls back on while nothing has its attention: how far
/// from where it was placed it will wander, how near it has to get before that
/// counts as arriving, and how long it stands about afterwards before picking
/// somewhere else.
///
/// A walk to a fixed spot followed by a rest, rather than a point it chases
/// continuously -- the same shape [`crate::squad::Ally`] ambles in, and for the
/// same reason: a target that moves every tick is one the walker never arrives
/// at, so it never stands still and its walk cycle restarts forever.
const WANDER_RADIUS: f32 = 7.0;
const WANDER_ARRIVE: f32 = 0.6;
const WANDER_REST: f32 = 1.5;
const WANDER_REST_SPREAD: f32 = 3.0;

/// The tallest thing a walking enemy can get up.
///
/// Not a tuning knob so much as a statement of what a goomba is: a short thing
/// on two feet. Anything taller than this is a wall to it however walkable the
/// top happens to be, which is the difference between climbing a slope and
/// appearing on top of a cliff.
///
/// It is half a unit because that is the tolerance [`LevelData::ground_at`]
/// already answers with, and one step limit is better than two that disagree.
const STEP_UP: f32 = 0.5;

/// How fast an enemy's feet follow the ground under them, climbing and
/// falling, in world units a second.
///
/// The floor query answers where its feet belong; putting them there in one
/// step is what makes a walker crossing a lip or a step look like it changed
/// elevation rather than walked up it. Climbing is the slower of the two on
/// purpose: things fall faster than they clamber.
const CLIMB_SPEED: f32 = 4.0;
const FALL_SPEED: f32 = 14.0;

/// How much room two enemies keep between their bodies, on top of the two
/// bodies themselves.
///
/// They are held apart as the cylinders they are already fought as, so this is
/// only the daylight between them -- but without some, a crowd all chasing the
/// same player converges on the same spot and stacks up into one enemy with
/// several models in it.
const PERSONAL_SPACE: f32 = 0.35;

/// How much of the overlap between two enemies is taken out per tick, how much
/// overlap is beneath noticing, and the furthest one may be shoved in a tick.
///
/// All three exist to keep a crowd still. Both members of a pair are pushed, so
/// even a quarter each closes the gap between them soon enough; the slack means
/// two that are merely touching are left alone rather than shoved apart and
/// pulled back every tick; and the cap stops one in the middle of a press, with
/// neighbours leaning on it from every side, from being fired out of the crowd
/// by the sum of them.
const SPREAD_RATE: f32 = 0.25;
const SPREAD_SLACK: f32 = 0.05;
const SPREAD_LIMIT: f32 = 1.5;

/// Where a crawler's probes start and how far past its feet they reach.
///
/// `PROBE_EYE` is the height the forward probe is cast from -- low, because a
/// bug that meets a wall is put down where its probe struck it, and a probe
/// cast from its back would have it teleport half its own height up the wall.
/// `PROBE_RISE` and `PROBE_DROP` bound the down probe, and between them decide
/// the steepest step it can climb and the widest lip it can walk over before
/// the surface is considered to have run out.
const PROBE_EYE: f32 = 0.12;
const PROBE_RISE: f32 = 0.4;
const PROBE_DROP: f32 = 0.8;

/// How far past its next step a crawler looks for something in its way.
///
/// Short on purpose. A bug that meets a wall is stood where its probe found it,
/// so this is also the furthest it can be moved in a tick by finding one --
/// and a slope counts as something in the way once it rises more than
/// `PROBE_EYE` over this. Reaching a body's width ahead, as looks reasonable,
/// has bugs jumping the better part of a metre up every hill on the lawn.
const PROBE_REACH: f32 = 0.15;

/// How fast a crawler can turn, in radians a second.
///
/// Not decoration. A bug that could turn instantly ping-pongs between the floor
/// and the wall in front of it every single tick -- the wall is the way towards
/// a player stood behind it, and the floor is the way towards him again the
/// moment the bug is on the wall -- and it spends the whole fight spinning on
/// the spot. Turning at a bug's pace, it commits to the climb.
const TURN_RATE: f32 = 3.0;

/// How far off its surface a crawler is held. Nothing to do with looks: the
/// next tick's probes start here, and a probe starting exactly on a triangle is
/// a probe that may or may not find it depending on the last bit of the float.
const CRAWL_SKIN: f32 = 0.02;

/// The enemies the port places. Each is resolved against the player as an
/// upright cylinder, the way the original does: a radius in the horizontal
/// plane and a height above its feet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Goomba,
    Scuttlebug,
}

impl Kind {
    pub fn model(self) -> &'static str {
        match self {
            Self::Goomba => "actors/goomba.glb",
            Self::Scuttlebug => "actors/scuttlebug.glb",
        }
    }

    /// Radius and height of its collision cylinder, from `sm64py/objects.py`.
    pub fn body(self) -> (f32, f32) {
        match self {
            Self::Goomba => (0.70, 1.00),
            Self::Scuttlebug => (0.60, 0.80),
        }
    }

    /// How wide a shadow it casts.
    ///
    /// Narrower than the collision cylinder, which is deliberately generous so
    /// that walking near one of these counts as touching it. A shadow drawn at
    /// that width would stick out well past the model and read as a puddle.
    pub fn shadow_radius(self) -> f32 {
        self.body().0 * 0.7
    }
}

/// Required rather than added at the spawn site, so that no enemy can exist
/// without a tier -- including the ones tests build by hand. A missing
/// `Detail` would not be a compile error, it would be an enemy silently left
/// out of every query that decides how much simulation it gets.
#[derive(Component)]
#[require(Detail)]
pub struct Enemy {
    /// What it is, which is also its collision cylinder and its model: kept as
    /// the one fact rather than as a copy of each thing derived from it.
    pub kind: Kind,
    pub animation: Handle<AnimationClip>,
}

/// Which side of the fight a creature is on.
///
/// On the enemies, on the Marios, and on the player -- the player carries one
/// without being able to notice anything himself, because a side is what makes
/// him worth noticing. Everything [`alert`] does is asked in terms of this
/// rather than of what a creature is: a Mario looks for the nearest thing not
/// on its side exactly as a goomba does.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Hostile,
    Friendly,
}

/// What an enemy has noticed, which is the whole of whether it is coming for
/// you or ambling about.
///
/// Aggro is never lost by walking away. Once an enemy has seen you it comes
/// until it dies or something else takes its attention, which is why the target
/// is kept as *who* rather than as a flag: a second thing worth chasing is a
/// change of target rather than a special case.
#[derive(Component, Default)]
pub struct Aggro {
    /// Who it is after, or `None` while nothing has its attention.
    pub target: Option<Entity>,
    /// Where that target was when [`alert`] last looked.
    ///
    /// The enemy walks to this rather than to the target itself, so it heads
    /// for the last place it knew of when the target is gone -- and so that
    /// the movement step needs nothing but the enemy's own components.
    pub at: Vec3,
}

/// How wide a berth an enemy gives what it is chasing, and how far that berth
/// varies from one to the next.
///
/// Without it every enemy in a brood walks to the same point -- the target's
/// feet -- and a crowd chasing one player is a crowd converging on one spot,
/// which is a scrum that [`spread`] then spends the fight pushing apart. With
/// it they arrive around him instead.
const STAND_OFF: f32 = 1.0;
const STAND_OFF_SPREAD: f32 = 1.6;

/// How far off the straight line to its goal an enemy wanders, and how quickly
/// that wander swings from one side to the other.
const WEAVE_WIDTH: f32 = 0.9;
const WEAVE_RATE: f32 = 1.3;

/// How much faster or slower than the rest one enemy walks, as a fraction.
const PACE_SPREAD: f32 = 0.25;

/// The small differences between one enemy and the next.
///
/// All of it comes off a single number fixed when the enemy is placed, so a
/// brood out of one pipe is a crowd of individuals rather than a marching band,
/// and none of it needs a random number generator -- the field stays
/// reproducible, which is what lets a test walk one of these across the castle
/// and get the same answer twice.
#[derive(Component)]
pub struct Quirk {
    seed: f32,
}

impl Quirk {
    fn new(phase: f32) -> Self {
        Self {
            seed: phase * crate::squad::GOLDEN_ANGLE,
        }
    }

    /// The single number everything else here is derived from.
    ///
    /// Read by [`crate::impostor`] to offset one enemy's walk cycle from the
    /// next's, which is the same job it does for the pace and the weave: a
    /// crowd of sprites all on the same frame is a marching band.
    pub fn seed(&self) -> f32 {
        self.seed
    }

    /// Its own walking pace, as a multiple of the speed for its kind.
    fn pace(&self) -> f32 {
        1.0 + PACE_SPREAD * (self.seed * 1.7).sin()
    }

    /// The spot around a target it makes for, rather than the target itself.
    fn stand_off(&self) -> Vec3 {
        let angle = self.seed * 2.3;
        let reach = STAND_OFF + STAND_OFF_SPREAD * (self.seed * 0.9).sin().abs();
        Vec3::new(angle.sin(), 0.0, angle.cos()) * reach
    }

    /// How far to one side of the straight line to its goal it is at `elapsed`,
    /// which is what keeps a group from converging along one bearing.
    fn weave(&self, elapsed: f32) -> f32 {
        WEAVE_WIDTH * (elapsed * WEAVE_RATE + self.seed).sin()
    }

    /// Where in its walk cycle it starts, in seconds.
    ///
    /// Read when a culled enemy's animation is started again on coming back
    /// into view. Starting every one of them at zero would put a crowd that
    /// crossed the draw distance together into perfect step, which is the one
    /// thing this whole component exists to prevent. A second is longer than
    /// either actor's clip, so the whole cycle is covered whatever its length --
    /// a seek past the end of a looping clip wraps.
    fn animation_phase(&self) -> f32 {
        (self.seed * 0.7).sin().abs()
    }
}

/// Where an enemy mills about while [`Aggro`] is empty.
#[derive(Component)]
pub struct Wander {
    /// The spot it was placed, which is the middle of its patch.
    home: Vec3,
    /// The spot in that patch it is walking to at the moment.
    goal: Vec3,
    /// How long it still has to stand about before it sets off again.
    rest_left: f32,
    /// Its own place in the sequence of spots, which is what keeps a brood out
    /// of lockstep with each other.
    phase: f32,
}

impl Wander {
    fn new(home: Vec3, phase: f32) -> Self {
        let mut wander = Self {
            home,
            goal: home,
            rest_left: 0.0,
            phase,
        };
        wander.somewhere_else();
        wander
    }

    /// Picks the next spot to amble to, and how long to stand about first.
    ///
    /// The golden angle, advanced per enemy: successive spots do not line up
    /// into a path that retraces itself, and two enemies never pick the same
    /// one at the same moment -- with no random number generator anywhere, so a
    /// field of them stays reproducible in a test. The same trick, for the same
    /// reasons, as [`crate::squad::Ally::amble_somewhere_else`].
    fn somewhere_else(&mut self) {
        self.phase += crate::squad::GOLDEN_ANGLE;
        let spread = |scale: f32| (self.phase * scale).sin().abs();
        let reach = WANDER_RADIUS * (0.4 + 0.6 * spread(0.37));
        self.goal = self.home + Vec3::new(self.phase.sin(), 0.0, self.phase.cos()) * reach;
        self.rest_left = WANDER_REST + WANDER_REST_SPREAD * spread(0.21);
    }

    /// Where it is walking this tick, or `None` while it is standing about.
    ///
    /// Arrival is measured in the horizontal plane on purpose. The spot is a
    /// place on the ground, and the ground under it is rarely at the height the
    /// enemy was placed at -- on a hill, or up a wall, a spot judged in three
    /// dimensions is one it can never quite reach.
    fn goal(&mut self, position: Vec3, dt: f32) -> Option<Vec3> {
        if self.rest_left > 0.0 {
            self.rest_left = (self.rest_left - dt).max(0.0);
            return None;
        }
        let there = Vec2::new(self.goal.x - position.x, self.goal.z - position.z);
        if there.length() < WANDER_ARRIVE {
            self.somewhere_else();
            return None;
        }
        Some(self.goal)
    }
}

/// An enemy that walks the level's surfaces rather than its floors.
///
/// A scuttlebug has eight legs and no opinion about which way is down, so it
/// treats a wall and a ceiling as more floor: it keeps its own up vector, which
/// is the normal of whatever it is stuck to at the time, and everything it does
/// -- which way it steps, which way it faces, which way it probes -- is asked
/// relative to that rather than to the world's Y.
#[derive(Component)]
pub struct Crawler {
    /// Up for this bug: the normal of the surface under its feet.
    pub up: Vec3,
    /// The way it is walking: a unit vector lying in that surface, which is
    /// also the way its model is turned.
    ///
    /// Kept rather than worked out afresh each tick from where it wants to be,
    /// because where it wants to be can be behind a wall -- and a bug that
    /// reconsiders that every tick never gets anywhere. See [`TURN_RATE`].
    pub heading: Vec3,
}

impl Default for Crawler {
    fn default() -> Self {
        // Whatever it is eventually stuck to, it starts the right way up and
        // finds out on its first step.
        Self {
            up: Vec3::Y,
            heading: Vec3::Z,
        }
    }
}

/// Puts one enemy in the world.
///
/// Shared by the level's own placements and by the warp pipes so the two
/// cannot drift apart -- a pipe spawning something subtly different from what
/// the level places is exactly the kind of difference that is invisible until
/// it is a bug report.
pub fn spawn(
    commands: &mut Commands,
    assets: &AssetServer,
    kind: Kind,
    position: Vec3,
    phase: f32,
) -> Entity {
    commands
        .spawn((
            Enemy {
                kind,
                animation: assets.load(format!("{}#Animation0", kind.model())),
            },
            Side::Hostile,
            Aggro::default(),
            Quirk::new(phase),
            Wander::new(position, phase),
            WorldAssetRoot(assets.load(format!("{}#Scene0", kind.model()))),
            Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
            // Parts of both of these are flat quads the original turns to face
            // the camera every frame.
            crate::billboard::BillboardActor,
            crate::shadow::ShadowCaster::new(kind.shadow_radius()),
        ))
        .insert_if(Crawler::default(), || kind == Kind::Scuttlebug)
        .id()
}

/// Where a benchmark crowd is centred, how far out it spreads, and the height
/// its floor query is asked from.
///
/// The centre and reach cover the castle grounds -- the collision spans roughly
/// x -82..82 and z -81..68 -- and the sky is above everything in it, so the
/// query finds the highest surface under each spot rather than starting inside
/// a hill.
const CROWD_CENTRE: Vec2 = Vec2::new(0.0, -6.0);
const CROWD_REACH: f32 = 70.0;
const CROWD_SKY: f32 = 90.0;

/// How many spots a crowd will try before giving up on reaching its count.
///
/// A spiral spot can land off the edge of the collision, where there is no
/// floor to stand something on, so the walk takes more spots than it places.
/// Four to one is far more slack than the castle needs and still terminates.
const CROWD_TRIES: usize = 4;

/// The spots a crowd of `count` is placed on: a sunflower spiral over the
/// castle, each one dropped onto whatever is under it.
///
/// A spiral by the golden angle with the radius going as the square root, which
/// is what spreads the field evenly instead of leaving it in rings -- the same
/// reason [`crate::squad::GOLDEN_ANGLE`] is used everywhere else here. No random
/// number generator, so `crowd 2000` puts the same field down every time and
/// two runs of the benchmark differ by the build rather than by the layout.
fn crowd_spots(count: usize, level: &LevelData) -> Vec<Vec3> {
    let mut spots = Vec::with_capacity(count);
    for step in 0..count * CROWD_TRIES {
        if spots.len() == count {
            break;
        }
        let angle = step as f32 * crate::squad::GOLDEN_ANGLE;
        // Against the count rather than the step, so the field covers the same
        // ground whether it holds two hundred or two thousand.
        let reach = CROWD_REACH * (step as f32 / count as f32).sqrt();
        let at = CROWD_CENTRE + Vec2::new(angle.sin(), angle.cos()) * reach;
        let sky = Vec3::new(at.x, CROWD_SKY, at.y);
        if let Some(floor) = level.floor_height(sky) {
            spots.push(Vec3::new(at.x, floor, at.y));
        }
    }
    spots
}

/// Carries out the console's `crowd` command.
///
/// Runs whether or not the console is open, because the console is open at the
/// moment the command is typed and a crowd that only appeared once you shut it
/// would be a crowd you never saw arrive.
pub fn crowd(
    mut commands: Commands,
    assets: Res<AssetServer>,
    level: Res<LevelData>,
    mut console: ResMut<ConsoleState>,
    existing: Query<Entity, With<Enemy>>,
) {
    // Taken before the loop and unconditionally: leaving a request in the queue
    // because this frame had nothing to do with it is a crowd placed again on
    // the next one.
    for request in console.take_requests() {
        let (count, kind) = match request {
            Request::ClearCrowd => (0, CrowdKind::Mix),
            Request::Crowd(count, kind) => (count, kind),
        };
        // Both cases clear first. `crowd 2000` twice is a field of two
        // thousand, not of four -- which is the whole point of a command whose
        // job is to make one number reproducible.
        for enemy in &existing {
            commands.entity(enemy).despawn();
        }
        for (index, spot) in crowd_spots(count, &level).into_iter().enumerate() {
            let kind = match kind {
                CrowdKind::Goomba => Kind::Goomba,
                CrowdKind::Scuttlebug => Kind::Scuttlebug,
                CrowdKind::Mix if index % 2 == 0 => Kind::Goomba,
                CrowdKind::Mix => Kind::Scuttlebug,
            };
            spawn(&mut commands, &assets, kind, spot, index as f32);
        }
    }
}

/// How much of the simulation an enemy is getting this tick.
///
/// The crowd budget is a *count*, not a distance: the nearest
/// `sim_budget` enemies are simulated in full and everything else is carried by
/// the flow field, however many of them there are. That is what makes a field of
/// five thousand cost the same as a field of five hundred -- the expensive tier
/// has a fixed size, and the cheap tier is O(1) an enemy.
///
/// What [`Full`](Detail::Full) buys, and what a [`Crowd`](Detail::Crowd) enemy
/// therefore does without:
///
/// * collision against the level -- walls, ledges, the step-up rule
/// * being held out of its neighbours by [`spread`]
/// * the aggro chain in [`alert`], and being a thing the player can hit
/// * a crawler's ability to walk up a wall
///
/// None of that is visible on something the size of a thumbnail, which is the
/// only kind of enemy that is ever in the cheap tier.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Detail {
    #[default]
    Full,
    Crowd,
}

/// How far along the flow field's route the cheap tier notices the player, as a
/// multiple of `enemy_sight`.
///
/// **One, deliberately: the cheap tier notices at exactly the range the
/// expensive one does, and no further.**
///
/// The temptation is to be generous here, because the cheap tier has no
/// [`alert`] chain -- a near enemy also hears about the player from a neighbour
/// who saw him, and comes from further than it can see, so an approximation
/// with no chain looks under-alert on paper. Both attempts at compensating for
/// that were wrong, and instructively so:
///
/// * four times sight, or about fifty metres, had the whole map charging the
///   player the instant it spawned. The lawn emptied and the field appeared to
///   have lost most of its enemies. It had not; they were all behind the camera.
/// * sight plus one hop of earshot, about twenty-three metres, was subtler and
///   still wrong. Every crowd enemy tests the range *independently*, so the
///   whole band inside it acquires at once -- where the real chain only spreads
///   between enemies that are near each other and only from one that has just
///   taken the alarm. The result was a visible front, with emptied lawn behind
///   it, at whatever radius was chosen.
///
/// The lesson is that an approximation of a chain is not a bigger radius, and
/// that the failure looks like missing enemies rather than like over-eager
/// ones. **A cheaper tier may look worse; it must not behave differently.**
/// Matching the near tier's own acquisition range under-alerts a little, in a
/// way that corrects itself the moment an enemy is promoted -- which is the
/// direction an error here should point.
const CROWD_SIGHT: f32 = 1.0;

/// Sorts the field by distance and hands the nearest `sim_budget` the full
/// simulation.
///
/// The cutoff is found with `select_nth_unstable`, which partitions in linear
/// time rather than sorting -- the exact order of two thousand enemies is not
/// wanted, only the boundary between the nearest two hundred and the rest.
pub fn assign_detail(
    tuning: Res<GameTuning>,
    player: Query<&Transform, With<Player>>,
    mut enemies: Query<(&Transform, &mut Detail), With<Enemy>>,
    mut ranked: Local<Vec<f32>>,
) {
    let Ok(player) = player.single() else {
        return;
    };
    let here = player.translation;
    let budget = tuning.sim_budget.max(0.0) as usize;
    ranked.clear();
    ranked.extend(
        enemies
            .iter()
            .map(|(transform, _)| here.distance_squared(transform.translation)),
    );
    // Everything fits in the budget, so nothing is demoted. The common case
    // while the field is small, and worth not partitioning for.
    let cutoff = if ranked.len() <= budget {
        f32::INFINITY
    } else {
        *ranked.select_nth_unstable_by(budget, f32::total_cmp).1
    };
    for (transform, mut detail) in &mut enemies {
        let wanted = if here.distance_squared(transform.translation) < cutoff {
            Detail::Full
        } else {
            Detail::Crowd
        };
        // Assigned only on a change. Writing the same value back would mark the
        // component changed for every enemy in the field every tick, which is
        // the sort of thing that costs more than the system it belongs to.
        if *detail != wanted {
            *detail = wanted;
        }
    }
}

/// Everything that can be noticed: where it is and whose side it is on.
///
/// `Detail` is optional because the player and the Marios are creatures too and
/// carry no tier of their own -- they are always simulated in full.
type Creatures<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Side,
        Option<&'static Detail>,
    ),
>;

/// Everything that does the noticing, which is the same crowd less the player,
/// who is told what to chase by whoever is holding the controller.
type Hunters<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Side,
        &'static mut Aggro,
        Option<&'static Detail>,
    ),
>;

/// Whether a creature is in the tier that gets the expensive treatment.
///
/// Anything without a tier -- the player, the Marios -- always is.
fn detailed(detail: Option<&Detail>) -> bool {
    detail != Some(&Detail::Crowd)
}

/// Who has noticed whom, and who has been told about it.
///
/// One rule for both sides. A creature with nothing to chase looks for the
/// nearest creature not on its side within `enemy_sight`; the moment it finds
/// one, everything on *its* side within `enemy_alert` hears about it, and
/// everything within `enemy_alert` of them hears it in turn, until the chain
/// runs out of neighbours. A crowd with one lookout in it all turns round at
/// once, which is what makes walking into the middle of one a mistake rather
/// than a series of separate small ones.
///
/// Only creatures that took the alarm this tick pass it on. One already in a
/// fight is not shouting about it forever, or the alarm would creep across the
/// whole field through whatever incidental pairs drift within earshot.
///
/// Nothing is ever given up on except by dying: aggro is not a leash, and an
/// enemy that has seen you comes until it or you are gone. What it does lose is
/// a target that has been despawned, which is not giving up but having won.
pub fn alert(tuning: Res<GameTuning>, everyone: Creatures, mut hunters: Hunters) {
    // Only the tier being simulated in full takes part. Everything this system
    // does is quadratic-ish in the crowd it is handed -- two spatial grids and a
    // flood fill -- so handing it two hundred rather than two thousand is most
    // of what the budget buys. The rest of the field notices the player through
    // the flow field instead, in [`update`], at no cost at all.
    let crowd: Vec<(Entity, Vec3, Side)> = everyone
        .iter()
        .filter(|(_, _, _, detail)| detailed(*detail))
        .map(|(entity, transform, side, _)| (entity, transform.translation, *side))
        .collect();
    let mut hunting: Vec<(Entity, Vec3, Side, Option<Entity>)> = hunters
        .iter()
        .filter(|(_, _, _, _, detail)| detailed(*detail))
        .map(|(entity, transform, side, aggro, _)| {
            let target = aggro
                .target
                // A target that is no longer in the world is no target. This is
                // how a Mario that has just flattened a goomba goes looking for
                // the next one rather than standing over the spot.
                .filter(|target| everyone.get(*target).is_ok());
            (entity, transform.translation, *side, target)
        })
        .collect();
    // Who can see something to go for. These are the seeds of the chain, and
    // the only ones that shout.
    let sight = tuning.enemy_sight;
    let seen = Neighbourhood::new(crowd.iter().map(|(_, at, _)| *at), sight);
    let mut nearby = Vec::new();
    let seeds: Vec<(usize, Entity)> = hunting
        .iter()
        .enumerate()
        .filter(|(_, (_, _, _, target))| target.is_none())
        .filter_map(|(index, &(_, at, side, _))| {
            seen.near(at, &mut nearby);
            nearby
                .iter()
                .map(|&other| crowd[other])
                .filter(|(_, _, theirs)| *theirs != side)
                .map(|(entity, theirs, _)| (entity, at.distance_squared(theirs)))
                .filter(|(_, range)| *range < sight * sight)
                .min_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(quarry, _)| (index, quarry))
        })
        .collect();
    let mut shouting: Vec<usize> = Vec::new();
    for (index, quarry) in seeds {
        hunting[index].3 = Some(quarry);
        shouting.push(index);
    }
    // And the chain: everything on the same side within earshot of a shout
    // takes the same target and shouts in its turn.
    let earshot = tuning.enemy_alert;
    let earshot_grid = Neighbourhood::new(hunting.iter().map(|(_, at, _, _)| *at), earshot);
    while let Some(index) = shouting.pop() {
        let (_, from, side, target) = hunting[index];
        earshot_grid.near(from, &mut nearby);
        for &other in &nearby {
            let (_, theirs, their_side, their_target) = hunting[other];
            if their_target.is_some()
                || their_side != side
                || theirs.distance_squared(from) >= earshot * earshot
            {
                continue;
            }
            hunting[other].3 = target;
            shouting.push(other);
        }
    }
    // Written back by entity rather than by position in the iteration, which is
    // not a thing to bet a fight on.
    let decided: HashMap<Entity, Option<Entity>> = hunting
        .iter()
        .map(|(entity, _, _, target)| (*entity, *target))
        .collect();
    for (entity, transform, _, mut aggro, detail) in &mut hunters {
        // A crowd-tier enemy keeps whatever it was chasing when it was last in
        // the near tier, and is not given a new target here. Losing one by
        // walking away would break the rule that aggro is never given up.
        if !detailed(detail) {
            continue;
        }
        let target = decided.get(&entity).copied().flatten();
        if aggro.target != target {
            aggro.target = target;
        }
        // Where that target is now, for the movement step to walk towards.
        aggro.at = match target.and_then(|target| everyone.get(target).ok()) {
            Some((_, seen, _, _)) => seen.translation,
            None => transform.translation,
        };
    }
}

/// The enemies that are held out of each other. Anything still flying the arc a
/// pipe threw it is left out, for the same reason the movement step leaves it
/// out: a shove during the launch is a launch that lands somewhere else.
type Jostling<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Enemy,
        &'static mut Transform,
        Option<&'static Crawler>,
        &'static Detail,
    ),
    (Without<Player>, Without<crate::pipe::Launched>),
>;

/// Holds enemies out of one another, so that a crowd chasing one player stays a
/// crowd rather than converging into a single stack of models.
///
/// A positional shove rather than a force: they are already resolved against
/// the player as cylinders, and this resolves them against each other the same
/// way. Crawlers are pushed within the surface they are stuck to -- shoving a
/// bug off its wall is the one thing this must not do -- and the walkers within
/// the horizontal plane, where their own floor query will catch them.
pub fn spread(mut enemies: Jostling) {
    // The near tier only. Two distant goombas standing in the same spot are two
    // pixels standing in the same spot, and untangling them costs a spatial
    // grid over the whole field to fix something nobody can see.
    let crowd: Vec<(Entity, Vec3, f32, Vec3)> = enemies
        .iter()
        .filter(|(_, _, _, _, detail)| **detail == Detail::Full)
        .map(|(entity, enemy, transform, crawler, _)| {
            (
                entity,
                transform.translation,
                enemy.kind.body().0,
                crawler.map_or(Vec3::Y, |crawler| crawler.up),
            )
        })
        .collect();
    let widest = crowd
        .iter()
        .fold(0.0_f32, |most, (_, _, radius, _)| most.max(*radius));
    let grid = Neighbourhood::new(
        crowd.iter().map(|(_, at, _, _)| *at),
        widest * 2.0 + PERSONAL_SPACE,
    );
    let mut near = Vec::new();
    for (index, &(entity, position, radius, up)) in crowd.iter().enumerate() {
        let mut push = Vec3::ZERO;
        grid.near(position, &mut near);
        for &other in &near {
            if other == index {
                continue;
            }
            let (_, theirs, their_radius, _) = crowd[other];
            let room = radius + their_radius + PERSONAL_SPACE;
            let apart = position - theirs;
            let overlap = room - apart.length() - SPREAD_SLACK;
            if overlap <= 0.0 {
                continue;
            }
            // Stood in exactly the same place, which two spawned by the same
            // pipe on the same tick genuinely are, there is no direction to be
            // pushed in and one has to be invented. The golden angle again, so
            // that a pile does not unfold along one line.
            let away = tangent(apart, up);
            let away = if away == Vec3::ZERO {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                tangent(Vec3::new(angle.sin(), 0.0, angle.cos()), up)
            } else {
                away
            };
            push += away * overlap * SPREAD_RATE;
        }
        if push != Vec3::ZERO {
            if let Ok((_, _, mut transform, _, _)) = enemies.get_mut(entity) {
                transform.translation += push.clamp_length_max(SPREAD_LIMIT * FIXED_DT);
            }
        }
    }
}

/// A crowd bucketed by where its members are standing, so that "everyone near
/// this one" costs its neighbours rather than the whole field.
///
/// Square cells in the horizontal plane, looked up nine at a time. Height is
/// left to the caller's own distance check: enemies are spread over a castle
/// rather than a tower, and a third axis of buckets would be mostly empty.
struct Neighbourhood {
    cell: f32,
    buckets: HashMap<(i32, i32), Vec<usize>>,
}

impl Neighbourhood {
    /// Buckets `points` into cells of `cell` on a side, which must be at least
    /// the distance the caller intends to ask about -- [`Self::near`] looks one
    /// cell out in each direction and no further.
    fn new(points: impl Iterator<Item = Vec3>, cell: f32) -> Self {
        let cell = cell.max(0.001);
        let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, point) in points.enumerate() {
            buckets
                .entry(Self::at(cell, point))
                .or_default()
                .push(index);
        }
        Self { cell, buckets }
    }

    fn at(cell: f32, point: Vec3) -> (i32, i32) {
        (
            (point.x / cell).floor() as i32,
            (point.z / cell).floor() as i32,
        )
    }

    /// Everything in the nine cells around `point`, appended to `found` after
    /// emptying it. The caller passes the same buffer back in each time rather
    /// than allocating a fresh one per member of the crowd.
    fn near(&self, point: Vec3, found: &mut Vec<usize>) {
        found.clear();
        let (x, z) = Self::at(self.cell, point);
        for z in z - 1..=z + 1 {
            for x in x - 1..=x + 1 {
                if let Some(bucket) = self.buckets.get(&(x, z)) {
                    found.extend_from_slice(bucket);
                }
            }
        }
    }
}

/// Connects the AnimationPlayer created inside a GLB scene to its enemy root.
///
/// The clip node comes along with the owner because [`sync_animation_visibility`]
/// *stops* a culled enemy's animation rather than pausing it, and something
/// stopped has to be started again by name when it comes back into view. An
/// enemy has exactly one clip, so one node is the whole of what that takes.
#[derive(Component)]
pub struct EnemyAnimationRoot {
    pub owner: Entity,
    pub clip: AnimationNodeIndex,
}

/// The enemies the AI step is allowed to move.
///
/// Anything still in the air on the arc a pipe threw it is flown by `pipe::fly`
/// and left out here: a behaviour that writes its own speed every tick would
/// eat the launch within a tick or two and drop it back on the pipe it came out
/// of. `Without<Player>` is the usual disjointness proof -- Bevy takes nothing
/// on trust from a `With` filter.
type WalkingEnemies<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Enemy,
        &'static mut Transform,
        &'static mut Visibility,
        // Mutable, because the cheap tier acquires its own target from the flow
        // field rather than being told about one by `alert`.
        &'static mut Aggro,
        &'static Quirk,
        &'static mut Wander,
        Option<&'static mut Crawler>,
        &'static Detail,
    ),
    (With<Enemy>, Without<Player>, Without<crate::pipe::Launched>),
>;

pub fn update(
    fixed_time: Res<Time<Fixed>>,
    level: Res<LevelData>,
    field: Res<crate::flow::FlowField>,
    player: Query<(Entity, &Transform), With<Player>>,
    mut enemies: WalkingEnemies,
    tuning: Res<GameTuning>,
    mut fixed_tick: Local<u32>,
) {
    let Ok((hero, player)) = player.single() else {
        return;
    };
    let player = player.translation;
    let elapsed = fixed_time.elapsed_secs();
    *fixed_tick = fixed_tick.wrapping_add(1);
    let tick = *fixed_tick;
    enemies.par_iter_mut().for_each(
        |(
            entity,
            enemy,
            mut transform,
            mut visibility,
            mut aggro,
            quirk,
            mut wander,
            crawler,
            detail,
        )| {
            let distance_squared = player.distance_squared(transform.translation);
            *visibility = if distance_squared > tuning.enemy_draw * tuning.enemy_draw {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
            let stride = if distance_squared > tuning.enemy_lod_far * tuning.enemy_lod_far {
                4
            } else if distance_squared > tuning.enemy_lod_near * tuning.enemy_lod_near {
                2
            } else {
                1
            };
            if !(tick + entity.index().index()).is_multiple_of(stride) {
                return;
            }
            // A reduced-rate step covers the skipped fixed ticks, so far actors
            // retain the same average movement and animation-independent AI time.
            let dt = crate::player::FIXED_DT * stride as f32;
            // Where it is going, and how fast. Something it has noticed is
            // chased at the chase speed and never given up on -- at its own
            // spot around it rather than at it, so a brood arrives around what
            // it is after instead of inside it. The rest of the time it ambles
            // around its own patch, and standing about between one spot and the
            // next is a goal of `None`.
            // The cheap tier: everything it needs is one array lookup, and it
            // asks the level nothing at all.
            if *detail == Detail::Crowd {
                crowd_step(
                    &field,
                    &mut transform,
                    &mut aggro,
                    &mut wander,
                    quirk,
                    hero,
                    &tuning,
                    dt,
                );
                return;
            }
            let (goal, speed) = match aggro.target {
                Some(_) => (Some(aggro.at + quirk.stand_off()), tuning.enemy_speed),
                None => (wander.goal(transform.translation, dt), WANDER_SPEED),
            };
            let speed = speed * quirk.pace();
            let up = crawler.as_ref().map_or(Vec3::Y, |crawler| crawler.up);
            // A wander across the line it is walking, so that a group heading
            // for one place does not converge on it along one bearing.
            let goal = goal.map(|goal| {
                let across = (goal - transform.translation).cross(up).normalize_or_zero();
                goal + across * quirk.weave(elapsed)
            });
            let Some(mut crawler) = crawler else {
                // The plain walkers stay in the horizontal plane, are stopped by
                // anything too steep to walk, and follow the ground under them.
                if let Some(goal) = goal {
                    let dir = tangent(goal - transform.translation, Vec3::Y);
                    transform.translation = walk(
                        &level,
                        transform.translation,
                        dir * dt * speed,
                        enemy.kind.body().0,
                    );
                    transform.rotation = Quat::from_rotation_y(dir.x.atan2(dir.z));
                }
                transform.translation = settle(&level, transform.translation, dt);
                return;
            };
            // A crawler heads for the same goal, but only along the surface it
            // is on, and only as fast as it can turn. Standing about, it is
            // still asked to walk nowhere in the direction it already faces:
            // that re-seats it on ground that may have shifted under it.
            let (goal, speed) = match goal {
                Some(goal) => (goal, speed),
                None => (transform.translation + crawler.heading, 0.0),
            };
            transform.translation =
                crawl_towards(&level, transform.translation, &mut crawler, goal, speed, dt);
            transform.rotation =
                orientation(crawler.up, crawler.heading).unwrap_or(transform.rotation);
        },
    );
}

/// One tick of a crowd-tier enemy: the whole of what a distant enemy costs.
///
/// No ray casts, no floor queries, no neighbours -- one lookup into
/// [`crate::flow::FlowField`], which answered all of those questions once for
/// everybody. What it gives up against [`walk`] and [`settle`] is collision:
/// this will walk through a fence and clip the corner of a wall. What it keeps
/// is everything that reads at a distance -- enemies that stream toward the
/// player round the moat rather than into it, that mill about when nothing has
/// their attention, and that stand on the ground rather than in it.
///
/// The two behaviours it picks between are the same two the near tier has, so
/// an enemy crossing the boundary carries on doing what it was doing rather
/// than visibly changing its mind.
#[allow(clippy::too_many_arguments)]
fn crowd_step(
    field: &crate::flow::FlowField,
    transform: &mut Transform,
    aggro: &mut Aggro,
    wander: &mut Wander,
    quirk: &Quirk,
    player: Entity,
    tuning: &GameTuning,
    dt: f32,
) {
    let guide = field.at(transform.translation);
    // Noticing the player, without [`alert`]. The field already knows how far
    // away he is *along walkable ground*, which is a better question than the
    // straight-line one the near tier asks -- something on the far side of the
    // moat is not as close as it looks. Once noticed he is never given up on,
    // exactly as in the near tier.
    let earshot = tuning.enemy_sight * CROWD_SIGHT;
    if aggro.target.is_none()
        && guide
            .steps
            .is_some_and(|steps| steps as f32 * field.cell_size() < earshot)
    {
        aggro.target = Some(player);
    }
    let speed = quirk.pace()
        * if aggro.target.is_some() {
            tuning.enemy_speed
        } else {
            WANDER_SPEED
        };
    // Chasing follows the field; ambling heads for its own patch, which is
    // ground it was placed on and so ground it can stand on.
    let towards = if aggro.target.is_some() {
        guide.towards
    } else {
        let goal = wander.goal(transform.translation, dt);
        goal.map_or(Vec2::ZERO, |goal| {
            Vec2::new(goal.x - transform.translation.x, goal.z - transform.translation.z)
                .normalize_or_zero()
        })
    };
    if towards != Vec2::ZERO {
        let step = towards * speed * dt;
        let ahead = transform.translation + Vec3::new(step.x, 0.0, step.y);
        // The one check that survives from the near tier, and the cheapest
        // possible version of it: the survey already recorded which cells have
        // ground, so walking off the map is a lookup rather than a ray cast.
        if field.at(ahead).walkable {
            transform.translation = ahead;
        }
        transform.rotation = Quat::from_rotation_y(step.x.atan2(step.y));
    }
    // Settled onto the cell's ground at a pace rather than snapped to it, for
    // the same reason [`settle`] does: a column's answer jumps at a cell
    // boundary, and taking it whole makes an enemy climbing a slope appear to
    // teleport up it a step at a time.
    let guide = field.at(transform.translation);
    if guide.walkable {
        let (rise, drop) = (CLIMB_SPEED * dt, FALL_SPEED * dt);
        transform.translation.y += (guide.ground - transform.translation.y).clamp(-drop, rise);
    }
}

/// Takes one step along the ground, if there is ground to take it onto.
///
/// Two questions, and between them they are the whole of what a walking enemy
/// may do with a cliff:
///
///  * is there something too steep to walk in the way? Probed at [`STEP_UP`]
///    above its feet, so that a kerb it could step up passes underneath the
///    probe while a wall, a cliff face or a slope past
///    [`crate::level::GROUND_NORMAL_Y`] does not. Steepness is measured with the
///    same threshold the collision grid sorts walls by, because a walker's idea
///    of too steep and the level's had better be one idea.
///  * is there ground at the far end to put its feet on? Ground it can get up
///    onto in a step, which is what [`LevelData::ground_at`] answers.
///
/// A refused step is retried one axis at a time, so a goomba that walks into a
/// wall at an angle slides along it instead of standing there pushing.
///
/// This replaced snapping to whatever the floor query answered, which is how a
/// goomba at the bottom of a cliff used to arrive at the top of it: the query
/// was happy to hand back a surface two body-heights up, and being handed it was
/// the same thing as standing on it.
fn walk(level: &LevelData, position: Vec3, step: Vec3, radius: f32) -> Vec3 {
    let knee = Vec3::Y * STEP_UP;
    for candidate in [
        step,
        Vec3::new(step.x, 0.0, 0.0),
        Vec3::new(0.0, 0.0, step.z),
    ] {
        if candidate.length_squared() < 1e-12 {
            continue;
        }
        // A body's width past where it would end up, so it stops with its face
        // at the wall rather than inside it.
        let reach = candidate + candidate.normalize() * radius;
        let blocked = level
            .surface_hit(position + knee, position + knee + reach)
            .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y);
        if blocked {
            continue;
        }
        let there = position + candidate;
        if level.ground_at(there).is_some() {
            return there;
        }
    }
    position
}

/// Walks `position` towards whatever it should be standing on, at a pace rather
/// than by teleport.
///
/// Three cases, and which one applies is the whole of how a walker treats a
/// cliff:
///
///  * there is ground under it -- *ground*, meaning something it could walk up
///    onto rather than merely something solid. [`LevelData::ground_at`] refuses
///    both the too-steep and the too-high, so what comes back is a step and not
///    a climb, and a goomba at the foot of a cliff is not offered the top of it.
///  * there is something under it but nothing it can stand on: a cliff face, the
///    slope it just walked off. It falls, and no further than the thing it fell
///    onto.
///  * there is nothing under it at all, which over this castle means it is
///    *inside* something rather than above it -- several of the level's own
///    placements are authored below the lawn they belong on. What is over its
///    head is the underside of that lawn, so it climbs out onto it.
///
/// The pace matters as much as the choice. The floor query answers a column and
/// a column's answer jumps; taking it whole is what made goombas appear to morph
/// between elevations rather than walk between them.
fn settle(level: &LevelData, position: Vec3, dt: f32) -> Vec3 {
    let (rise, drop) = (CLIMB_SPEED * dt, FALL_SPEED * dt);
    let towards = |height: f32, most: f32| {
        Vec3::new(
            position.x,
            position.y + (height - position.y).clamp(-drop, most),
            position.z,
        )
    };
    if let Some((ground, _)) = level.ground_at(position) {
        return towards(ground, rise);
    }
    match level.floor_height(position + Vec3::Y * STEP_UP) {
        // Nothing to stand on, but something to land on.
        Some(floor) => towards(floor, 0.0),
        // Nothing either: it is in the hill rather than on it.
        None => match level.ceiling_height(position, 0.0) {
            Some(top) => towards(top, rise),
            None => position,
        },
    }
}

/// Is there anything at all for an enemy to stand on here?
///
/// Asked of the crawlers' fallback, which needs to know whether the bug it is
/// dropping has landed rather than what height that landing is at.
fn ground_under(level: &LevelData, position: Vec3) -> bool {
    level.ground_at(position).is_some()
}

/// Walks a crawler one tick towards `goal`, turning it as it goes, and reports
/// where it ended up.
///
/// This is the whole of what a scuttlebug does, kept out of [`update`] so that a
/// bug can be walked across a level in a test without an app around it.
fn crawl_towards(
    level: &LevelData,
    position: Vec3,
    crawler: &mut Crawler,
    goal: Vec3,
    speed: f32,
    dt: f32,
) -> Vec3 {
    // The part of the way to the goal it could actually walk. Behind a wall,
    // that is the wall -- and a bug that walks into a wall climbs it.
    let wanted = tangent(goal - position, crawler.up);
    crawler.heading = steer(crawler.heading, wanted, crawler.up, TURN_RATE * dt);
    match crawl(level, position, crawler.up, crawler.heading * speed * dt) {
        Some((moved, up)) => {
            // The heading is carried round the corner rather than recomputed:
            // a bug that walks over the lip of a table is still walking the
            // way it was, it is just that the way it was has been bent by the
            // edge it went round.
            crawler.heading = tangent(
                Quat::from_rotation_arc(crawler.up, up) * crawler.heading,
                up,
            );
            crawler.up = up;
            moved
        }
        // Nothing within reach in any direction, so there is no surface to walk
        // and nothing to be the right way up for: it is over open space, or
        // under the map at a spawn point placed below the hill it was meant to
        // be on. It carries on the way the plain walkers do -- straight there,
        // and dropped onto the first floor that appears under it -- which is
        // also how it gets out from under that hill.
        None => {
            let drifted = position + crawler.heading * speed * dt;
            if ground_under(level, drifted) {
                crawler.up = Vec3::Y;
            }
            settle(level, drifted, dt)
        }
    }
}

/// Turns `heading` towards `target` within the surface `up` names, by at most
/// `most` radians.
fn steer(heading: Vec3, target: Vec3, up: Vec3, most: f32) -> Vec3 {
    let heading = tangent(heading, up);
    if heading == Vec3::ZERO {
        return target;
    }
    if target == Vec3::ZERO {
        return heading;
    }
    let angle = heading.angle_between(target);
    if angle <= most {
        return target;
    }
    // Which way round the shorter turn is. Dead astern it is neither way, and
    // the bug picks one rather than standing there unable to choose.
    let sign = if heading.cross(target).dot(up) < 0.0 {
        -1.0
    } else {
        1.0
    };
    Quat::from_axis_angle(up, sign * most) * heading
}

/// The part of `vector` that lies in the surface whose normal is `up`, as a
/// direction. Zero when `vector` has nothing to say about where to go along the
/// surface, which is the case a caller has to handle rather than normalise.
fn tangent(vector: Vec3, up: Vec3) -> Vec3 {
    (vector - up * vector.dot(up)).normalize_or_zero()
}

/// Stands a model on `up` and turns it to face `forward`, which is the thing
/// `from_rotation_y` cannot express once up has stopped being up.
///
/// `None` when the two are the same direction and there is no facing to build
/// out of them -- a bug walking exactly into the surface it is stuck to, which
/// the caller answers by leaving it turned the way it already was.
fn orientation(up: Vec3, forward: Vec3) -> Option<Quat> {
    let forward = tangent(forward, up);
    if forward == Vec3::ZERO {
        return None;
    }
    // Right-handed, and orthonormal because up and forward are perpendicular
    // unit vectors by construction: exactly what `from_mat3` requires.
    let right = up.cross(forward);
    Some(Quat::from_mat3(&Mat3::from_cols(right, up, forward)))
}

/// Walks a crawler one step of `step` along whatever it is stuck to, and
/// reports where it ended up and which way is up there.
///
/// Three questions, and the order they are asked in is the whole of it:
///
/// * is something in the way? An inside corner -- the foot of a wall, or the
///   top of one where it meets the ceiling. The surface it ran into becomes the
///   surface it is standing on, which is how a bug gets off the floor and,
///   eventually, onto the ceiling.
/// * is there anything under the step? The ordinary case, and every slope with
///   it, since the answer carries the new surface's normal.
/// * is there anything under the *lip* it just walked over? An outside corner --
///   the edge of a table -- where the far side of the edge becomes the floor and
///   the bug carries on down it upside down relative to where it started.
///
/// `None` when all three miss, which means open space rather than a surface.
fn crawl(level: &LevelData, position: Vec3, up: Vec3, step: Vec3) -> Option<(Vec3, Vec3)> {
    let distance = step.length();
    if distance < 1e-6 {
        // Still worth asking what it is standing on -- the ground under a bug
        // that has stopped is the ground it should be lying along.
        let start = position + up * PROBE_RISE;
        return level
            .surface_hit(start, position - up * PROBE_DROP)
            .map(|(hit, normal)| (hit + normal * CRAWL_SKIN, normal))
            .or(Some((position, up)));
    }
    let direction = step / distance;
    // Cast from just off the surface: a probe starting on the floor it is
    // standing on finds that floor and nothing else.
    let eye = position + up * PROBE_EYE;
    if let Some((hit, normal)) = level.surface_hit(eye, eye + direction * (distance + PROBE_REACH))
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    let ahead = position + step;
    if let Some((hit, normal)) = level.surface_hit(ahead + up * PROBE_RISE, ahead - up * PROBE_DROP)
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    // Under and past the edge, looking back the way it came: what it finds is
    // the far face of the lip it just walked off. Reaching back exactly as far
    // as the edge can be and no further, because whatever this finds the bug is
    // put on, and a longer reach is a longer hop round the corner.
    let under = ahead - up * PROBE_DROP;
    if let Some((hit, normal)) =
        level.surface_hit(under, under - direction * (distance + PROBE_REACH))
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    None
}

/// Resolves the player against every enemy once a tick: a swing defeats what
/// is in front of him, coming down on one stomps it, and touching one any
/// other way throws him back.
///
/// Ported from `Interactions.resolve` in `sm64py/objects.py`, including the
/// three things that make it a fight rather than a mutual accident.
#[allow(clippy::type_complexity)]
pub fn combat(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut player: Query<(&Transform, &mut Controller), With<Player>>,
    enemies: Query<(Entity, &Enemy, &Transform, &Detail), Without<Player>>,
) {
    let Ok((player_transform, mut controller)) = player.single_mut() else {
        return;
    };
    // The cooldown gates the whole resolution rather than only the damage.
    // That is not a detail: a knocked-back player is thrown up and off the
    // enemy that hit him and comes down on its head, so without this every
    // enemy that touches somebody standing perfectly still stomps *itself*
    // within a couple of seconds. A warp pipe whose every goomba destroys
    // itself before you turn round is a warp pipe that appears to spawn
    // nothing at all.
    if controller.invulnerable_left > 0.0 {
        controller.invulnerable_left = (controller.invulnerable_left - FIXED_DT).max(0.0);
        return;
    }
    let here = player_transform.translation;
    let facing = player_transform.rotation * Vec3::Z;
    for (entity, enemy, transform, detail) in &enemies {
        // The crowd tier is by definition further away than the nearest two
        // hundred, and the player's reach is two metres. Nothing out there can
        // be touched, hit or stomped, so nothing out there is tested.
        if *detail == Detail::Crowd {
            continue;
        }
        let offset = transform.translation - here;
        let horizontal = Vec3::new(offset.x, 0.0, offset.z);
        let distance_squared = horizontal.length_squared();
        let bearing = horizontal.normalize_or_zero();
        if controller.attack_left > 0.0
            && distance_squared < ATTACK_REACH * ATTACK_REACH
            && facing.dot(bearing) > -0.15
        {
            commands.entity(entity).despawn();
            sounds.push(Sfx::Defeat);
            continue;
        }
        let (radius, height) = enemy.kind.body();
        let reach = radius + PLAYER_REACH;
        if distance_squared > reach * reach {
            continue;
        }
        // Vertical overlap: his feet below its head, his head above its feet.
        // Without this he is "touching" it from a storey up.
        //
        // Which end of a crawler is its head depends on what it is stuck to --
        // one hanging from a ceiling has its head *below* its feet -- so the
        // band it occupies is measured from the direction its own model is
        // stood on rather than assumed to run upwards. For everything that does
        // stand upright that reads as it always did.
        let head = transform.translation + (transform.rotation * Vec3::Y) * height;
        let bottom = transform.translation.y.min(head.y);
        let top = transform.translation.y.max(head.y);
        if here.y > top || here.y + PLAYER_HEIGHT < bottom {
            continue;
        }
        if controller.velocity.y < 0.0 && here.y > bottom + (top - bottom) * STOMP_MARGIN {
            commands.entity(entity).despawn();
            sounds.push(Sfx::Defeat);
            controller.velocity.y = BOUNCE_VELOCITY;
            controller.grounded = false;
            continue;
        }
        controller.velocity = -bearing * KNOCKBACK_SPEED + Vec3::Y * KNOCKBACK_RISE;
        controller.grounded = false;
        controller.invulnerable_left = INVULNERABLE_SECONDS;
        controller.health = controller.health.saturating_sub(1);
        sounds.push(Sfx::Hurt);
        // One hit a tick: walking into a cluster of them costs one heart, not
        // one per enemy in the cluster.
        return;
    }
}

/// How long a Mario's punch takes from the moment he starts it, and how far it
/// lands. The reach is the player's sword's less the difference between a
/// sword and a fist.
const MARIO_SWING: f32 = 0.45;
const MARIO_REACH: f32 = 1.6;

/// The Marios' half of the fight: a Mario stood over what it has noticed hits
/// it, and what it hits dies.
///
/// The blow lands at the *end* of the swing rather than the moment it starts,
/// so what kills a goomba is the punch connecting rather than the decision to
/// punch -- one that wanders out of reach mid-swing is missed, which is the
/// only thing that makes standing next to one of these dangerous at all.
///
/// Runs after the allies have been walked, because the swing overrides what
/// they were doing: a Mario in the middle of a punch is punching, whatever the
/// walk step made of it.
pub fn ally_combat(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut allies: Query<(&mut Ally, &Transform, &Aggro)>,
    enemies: Query<&Transform, (With<Enemy>, Without<Ally>)>,
) {
    for (mut ally, transform, aggro) in &mut allies {
        let quarry = aggro
            .target
            .and_then(|target| enemies.get(target).ok().map(|at| (target, at.translation)));
        let in_reach = quarry.is_some_and(|(_, at)| {
            let apart = at - transform.translation;
            Vec3::new(apart.x, 0.0, apart.z).length() < MARIO_REACH
        });
        if ally.swing_left > 0.0 {
            ally.swing_left = (ally.swing_left - FIXED_DT).max(0.0);
            ally.state.motion = Motion::Attack;
            ally.state.still_for = 0.0;
            if ally.swing_left == 0.0 && in_reach {
                let (target, _) = quarry.expect("in reach of nothing");
                commands.entity(target).despawn();
                sounds.push(Sfx::Defeat);
            }
            continue;
        }
        if in_reach {
            ally.swing_left = MARIO_SWING;
            // Punches alternate, so a Mario stood over a crowd is throwing a
            // combination rather than the same punch on a loop.
            ally.state.combo ^= 1;
            ally.state.motion = Motion::Attack;
            ally.state.still_for = 0.0;
        }
    }
}

/// Hidden skinned actors do not need their bones evaluated.
///
/// **Stopped rather than paused, and the difference is the whole saving.**
/// Pausing was what this did, and it saved nothing at all: in
/// `bevy_animation`, `advance_animations` checks `paused` only to skip the
/// clock, and `animate_targets` checks it only to skip firing animation
/// events. A paused player still has its curves sampled and still writes a
/// pose onto every joint it owns, every rendered frame -- for a field of two
/// thousand that is tens of thousands of skeleton entities being posed to
/// exactly the value they already held.
///
/// Worse than the wasted sampling is what the writes cost downstream.
/// `bevy_transform` skips whole subtrees whose transforms have not changed,
/// and a pose written every frame keeps every joint permanently dirty, so
/// propagation walks the entire crowd as well.
///
/// Stopping empties the player's active animations, so the clip lookup finds
/// nothing and returns immediately, no joint is written, and the dirty-tree
/// skip finally engages. What it costs is that resuming has to start the clip
/// again rather than unpause it -- which is what `restart` below does, from
/// the graph node the player was given when its scene arrived.
pub fn sync_animation_visibility(
    console: Res<crate::console::ConsoleState>,
    roots: Query<(&Visibility, &Quirk), With<Enemy>>,
    mut players: Query<(&EnemyAnimationRoot, &mut AnimationPlayer)>,
) {
    for (root, mut player) in &mut players {
        let Ok((visibility, quirk)) = roots.get(root.owner) else {
            continue;
        };
        // Three states, and which one an enemy wants is worth naming because
        // the console's case and the culled case are deliberately *not* the
        // same. Culled means stopped, because nobody is looking and the whole
        // point is to stop paying for the pose. The console open means merely
        // paused, because you are looking straight at the crowd and stopping it
        // would restart every clip from its first frame the moment you shut the
        // console -- turning a field of individuals into a marching band.
        let hidden = *visibility == Visibility::Hidden;
        // Read through the shared reference: merely *asking* what a player is
        // doing must not mark it changed. Every branch below is guarded on
        // this, because taking `&mut` unconditionally would flag the component
        // on every enemy on every frame, which is a change-detection storm in
        // place of the one being removed.
        let started = player.playing_animations().next().is_some();
        let paused = player.all_paused();
        match (hidden, console.open, started, paused) {
            // Out of view and still costing something: stop it.
            (true, _, true, _) => {
                player.stop_all();
            }
            // Nothing to do -- already in the state it wants.
            (true, _, false, _) => {}
            (false, open, true, held) if open == held => {}
            // In view, and the console has just opened or shut over it.
            (false, true, true, _) => {
                player.pause_all();
            }
            (false, false, true, _) => {
                player.resume_all();
            }
            // Back into view after being culled. An enemy's whole animation is
            // the single looping clip its kind loads, and `EnemyAnimationRoot`
            // carries the node it sits at in the graph its scene was given.
            //
            // Seeded from the enemy's own quirk rather than started at zero, so
            // a crowd that walks back into the draw distance together does not
            // arrive in lockstep -- the same reason nothing else about these is
            // shared either.
            (false, _, false, _) => {
                let phase = quirk.animation_phase();
                player.play(root.clip).repeat().seek_to(phase);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A room with a floor, one wall across the far end of it and a ceiling:
    /// the three surfaces a scuttlebug is supposed to treat as the same thing.
    ///
    /// The wall is at `x = 4` and the ceiling at `y = 4`, both spanning the
    /// floor's footprint, so a bug that keeps walking in `+x` meets each of
    /// them in turn.
    fn room() -> LevelData {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let base = vertices.len() as u32;
            vertices.extend([a, b, c, d]);
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        };
        quad(
            Vec3::new(-4., 0., -4.),
            Vec3::new(4., 0., -4.),
            Vec3::new(4., 0., 4.),
            Vec3::new(-4., 0., 4.),
        );
        quad(
            Vec3::new(4., 0., -4.),
            Vec3::new(4., 4., -4.),
            Vec3::new(4., 4., 4.),
            Vec3::new(4., 0., 4.),
        );
        quad(
            Vec3::new(-4., 4., -4.),
            Vec3::new(4., 4., -4.),
            Vec3::new(4., 4., 4.),
            Vec3::new(-4., 4., 4.),
        );
        LevelData::new(vertices, triangles, Vec::new())
    }

    /// Walks a bug towards `goal` the way [`update`] does, and reports every
    /// place it stood on the way and which way was up there.
    fn trail(
        level: &LevelData,
        start: (Vec3, Vec3),
        goal: Vec3,
        steps: usize,
    ) -> Vec<(Vec3, Vec3)> {
        let (mut position, up) = start;
        let mut crawler = Crawler {
            up,
            heading: tangent(goal - position, up),
        };
        (0..steps)
            .map(|_| {
                position = crawl_towards(level, position, &mut crawler, goal, 3.0, FIXED_DT);
                (position, crawler.up)
            })
            .collect()
    }

    /// The whole point of the thing: a scuttlebug chasing something it cannot
    /// reach along the floor climbs the wall in its way, carries on over the
    /// top of it onto the ceiling, and walks that upside down.
    #[test]
    fn a_crawler_walks_up_a_wall_and_across_the_ceiling() {
        let level = room();
        // Beyond the wall, so that the way to it is through the wall rather
        // than across the floor.
        let climb = trail(
            &level,
            (Vec3::new(-3., 0., 0.), Vec3::Y),
            Vec3::new(10., 4., 0.),
            200,
        );
        let climbed = climb
            .iter()
            .find(|(position, up)| up.x < -0.99 && position.y > 0.5)
            .unwrap_or_else(|| panic!("it never climbed the wall: {:?}", climb.last()));
        assert!(
            (climbed.0.x - 4.).abs() < 0.1,
            "climbing thin air: {climbed:?}"
        );
        let hanging = climb
            .iter()
            .find(|(_, up)| up.y < -0.99)
            .unwrap_or_else(|| panic!("it never made it onto the ceiling: {:?}", climb.last()));
        assert!(
            (hanging.0.y - 4.).abs() < 0.1,
            "hanging off nothing: {hanging:?}"
        );
        // And once there it walks the ceiling like any other floor, which the
        // corner it climbed in at would hide.
        let across = trail(&level, *hanging, Vec3::new(-10., 4., 0.), 40);
        let (position, up) = *across.last().unwrap();
        assert!(
            up.y < -0.99 && (position.y - 4.).abs() < 0.1 && position.x < hanging.0.x - 1.0,
            "it did not cross the ceiling: at {position:?} with up {up:?}"
        );
    }

    /// The other kind of corner. Walking off the end of a slab, the bug follows
    /// the far side of it down rather than stepping into the air.
    #[test]
    fn a_crawler_follows_an_edge_round_onto_the_underside() {
        let level = LevelData::new(
            vec![
                Vec3::new(-4., 0., -4.),
                Vec3::new(4., 0., -4.),
                Vec3::new(4., 0., 4.),
                Vec3::new(-4., 0., 4.),
                // The slab's outer face, hanging below its edge.
                Vec3::new(4., 0., -4.),
                Vec3::new(4., -4., -4.),
                Vec3::new(4., -4., 4.),
                Vec3::new(4., 0., 4.),
            ],
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            Vec::new(),
        );
        let (position, up) = crawl(&level, Vec3::new(4.0, 0., 0.), Vec3::Y, Vec3::X * 0.3)
            .expect("the bug stepped into the air rather than round the edge");
        assert!(up.x > 0.99, "not clinging to the outer face: up {up:?}");
        assert!(position.y < 0.0, "still on top of the slab: {position:?}");
    }

    /// Turned loose on the real castle, a bug chasing a player it cannot reach
    /// walks the place rather than fighting it.
    ///
    /// The failure this pins is the one the turn rate is there for. Deciding
    /// afresh every tick which way the player is, a bug at the foot of a wall
    /// climbs it (the way to the player is up), immediately steps back down
    /// (the way to the player is along the floor), and repeats: six hundred
    /// changes of surface in half a minute, which on screen is a scuttlebug
    /// spinning like a top in the corner.
    #[test]
    fn a_crawler_on_the_castle_does_not_spin_between_surfaces() {
        let (level, _) = crate::level::load();
        for start in [Vec3::new(-29., 3., 21.), Vec3::new(4., 3., 19.)] {
            // Somewhere it has to cross the castle to get to, so the walk takes
            // in walls, corners and the courtyard rather than open lawn.
            let goal = Vec3::new(0., 8., -30.);
            let mut position = start;
            let mut crawler = Crawler::default();
            let mut was = crawler.up;
            let (mut flips, mut furthest) = (0, 0.0_f32);
            for _ in 0..900 {
                let before = position;
                position = crawl_towards(&level, position, &mut crawler, goal, 4.0, FIXED_DT);
                if crawler.up.distance(was) > 0.5 {
                    flips += 1;
                }
                was = crawler.up;
                // The floor snap is allowed its jump; a step along a surface is
                // not, and going round an edge is the longest of them.
                if crawler.up != Vec3::Y {
                    furthest = furthest.max(position.distance(before));
                }
            }
            assert!(
                flips < 30,
                "from {start:?} it changed surface {flips} times in 900 ticks"
            );
            assert!(
                furthest < 1.0,
                "from {start:?} it jumped {furthest:.2} in one tick"
            );
            assert!(
                position.distance(start) > 5.0,
                "from {start:?} it never got anywhere, ending at {position:?}"
            );
        }
    }

    /// Nothing within reach in any direction is not a surface, and the caller
    /// puts a bug that finds itself there back on the world's floor.
    #[test]
    fn a_crawler_over_open_space_finds_nothing() {
        assert!(crawl(&room(), Vec3::new(0., 20., 0.), Vec3::Y, Vec3::X * 0.1).is_none());
    }

    /// A bug on the ceiling is stood on its head, and its model has to be too.
    #[test]
    fn a_crawler_is_stood_on_the_surface_it_is_stuck_to() {
        let rotation = orientation(Vec3::NEG_Y, Vec3::X).expect("no facing was built");
        assert!((rotation * Vec3::Y).abs_diff_eq(Vec3::NEG_Y, 1e-5));
        assert!((rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1e-5));
        // Walking straight into the surface it is stuck to says nothing about
        // which way it is facing, and it keeps the facing it had.
        assert!(orientation(Vec3::Y, Vec3::Y).is_none());
    }

    /// The benchmark field: the size that was asked for, on real ground, in the
    /// same places every run.
    ///
    /// Reproducibility is the whole value of the command. A crowd scattered by
    /// a random number generator would make two runs of the benchmark differ by
    /// the layout as well as by the build, which is exactly the comparison it
    /// exists to make.
    #[test]
    fn a_benchmark_crowd_is_the_size_asked_for_and_stands_on_the_castle() {
        let (level, _) = crate::level::load();
        for count in [200, 2000] {
            let spots = crowd_spots(count, &level);
            assert_eq!(spots.len(), count, "a crowd of {count} came up short");
            for spot in &spots {
                assert!(
                    level.floor_height(*spot + Vec3::Y * 0.5).is_some(),
                    "a benchmark enemy at {spot:?} is standing on nothing"
                );
            }
        }
        assert_eq!(
            crowd_spots(500, &level),
            crowd_spots(500, &level),
            "two runs of the benchmark got two different fields"
        );
    }

    /// It spreads rather than piling up: a crowd stacked on one spot measures
    /// the cost of a stack, and `spread` would spend the whole run untangling it.
    #[test]
    fn a_benchmark_crowd_is_spread_over_the_grounds() {
        let (level, _) = crate::level::load();
        let spots = crowd_spots(1000, &level);
        let far = spots
            .iter()
            .map(|spot| Vec2::new(spot.x, spot.z).distance(CROWD_CENTRE))
            .fold(0.0_f32, f32::max);
        assert!(far > CROWD_REACH * 0.5, "the crowd only reached {far}");
        // And no two of them are placed in exactly the same spot.
        let mut apart = f32::MAX;
        for (index, spot) in spots.iter().enumerate().take(200) {
            for other in &spots[index + 1..] {
                apart = apart.min(spot.distance(*other));
            }
        }
        assert!(apart > 0.01, "two benchmark enemies are {apart} apart");
    }

    /// A player at the origin and enemies stood where they are asked for.
    fn field(placed: &[Vec3]) -> (World, Entity, Vec<Entity>) {
        let mut world = World::new();
        world.insert_resource(GameTuning::default());
        let player = world
            .spawn((Player, Side::Friendly, Transform::default()))
            .id();
        let enemies = placed
            .iter()
            .map(|at| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Goomba,
                            animation: Handle::default(),
                        },
                        Side::Hostile,
                        Aggro::default(),
                        Wander::new(*at, 0.0),
                        Transform::from_translation(*at),
                    ))
                    .id()
            })
            .collect();
        (world, player, enemies)
    }

    fn aggro(world: &mut World, enemy: Entity) -> Option<Entity> {
        world.get::<Aggro>(enemy).expect("no aggro").target
    }

    /// The chain reaction. One enemy sees the player; the one behind it hears
    /// that, and the one behind that hears it in turn -- all on the tick the
    /// first one noticed, and all pointed at the same player. The fourth is a
    /// stride too far back and hears nothing.
    #[test]
    fn one_enemy_noticing_the_player_turns_the_whole_line_round() {
        let tuning = GameTuning::default();
        let (sight, earshot) = (tuning.enemy_sight, tuning.enemy_alert);
        let line = [
            Vec3::new(sight - 1.0, 0., 0.),
            Vec3::new(sight - 1.0 + earshot - 1.0, 0., 0.),
            Vec3::new(sight - 1.0 + (earshot - 1.0) * 2.0, 0., 0.),
            Vec3::new(sight - 1.0 + (earshot - 1.0) * 2.0 + earshot + 2.0, 0., 0.),
        ];
        let (mut world, player, enemies) = field(&line);
        world.run_system_once(alert).expect("alert could not run");
        for (index, enemy) in enemies.iter().take(3).enumerate() {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(player),
                "number {index} in the line never heard the alarm"
            );
        }
        assert_eq!(
            aggro(&mut world, enemies[3]),
            None,
            "the alarm carried past the gap in the line"
        );
        // And what it heard is where the player is, which is what it walks to.
        let heard = world.get::<Aggro>(enemies[2]).unwrap().at;
        assert_eq!(heard, Vec3::ZERO);
    }

    /// Aggro is not a leash. Once an enemy has noticed the player it keeps
    /// coming however far away he gets, and keeps being told where he is.
    #[test]
    fn an_enemy_that_has_noticed_the_player_never_loses_interest() {
        let (mut world, player, enemies) = field(&[Vec3::new(2., 0., 0.)]);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(aggro(&mut world, enemies[0]), Some(player));
        // Right across the castle, far outside anything it could see.
        let away = Vec3::new(300., 0., 300.);
        world.get_mut::<Transform>(player).unwrap().translation = away;
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(
            aggro(&mut world, enemies[0]),
            Some(player),
            "it gave up because he walked away"
        );
        assert_eq!(
            world.get::<Aggro>(enemies[0]).unwrap().at,
            away,
            "it is still walking to where he used to be"
        );
    }

    /// Two enemies may not stand in the same place, however hard the thing
    /// they are both chasing pulls them together.
    #[test]
    fn enemies_are_held_out_of_one_another() {
        let together = Vec3::new(5., 0., 5.);
        let (mut world, _, enemies) = field(&[together, together]);
        for _ in 0..60 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let apart = world.get::<Transform>(enemies[0]).unwrap().translation
            - world.get::<Transform>(enemies[1]).unwrap().translation;
        let room = Kind::Goomba.body().0 * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        assert!(
            apart.length() > room - 0.01,
            "two enemies settled {} apart, inside the {room} they are owed",
            apart.length()
        );
    }

    /// A crowd untangles and then holds still. The one in the middle of a press
    /// has neighbours leaning on it from every side, and if the shove takes out
    /// the whole overlap every tick, what it does with that is vibrate.
    #[test]
    fn a_packed_crowd_settles_instead_of_jittering() {
        let heap: Vec<Vec3> = (0..20)
            .map(|index| {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                Vec3::new(angle.sin(), 0., angle.cos()) * (index as f32 * 0.05)
            })
            .collect();
        let (mut world, _, enemies) = field(&heap);
        let places = |world: &mut World| -> Vec<Vec3> {
            enemies
                .iter()
                .map(|enemy| world.get::<Transform>(*enemy).unwrap().translation)
                .collect()
        };
        for _ in 0..600 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let settled = places(&mut world);
        for _ in 0..30 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let after = places(&mut world);
        let moved = settled
            .iter()
            .zip(&after)
            .fold(0.0_f32, |most, (was, now)| most.max(was.distance(*now)));
        assert!(
            moved < 0.01,
            "a settled crowd was still shuffling {moved} a tick"
        );
        let room = Kind::Goomba.body().0 * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        for (index, one) in after.iter().enumerate() {
            for other in &after[index + 1..] {
                assert!(
                    one.distance(*other) > room - 0.05,
                    "two of the crowd ended up {} apart",
                    one.distance(*other)
                );
            }
        }
    }

    /// And a crawler is shoved along its surface rather than off it: two bugs
    /// jostling on a wall stay on the wall.
    #[test]
    fn crawlers_are_shoved_along_the_surface_they_are_stuck_to() {
        let mut world = World::new();
        let wall = Vec3::new(4., 3., 0.);
        let pair: Vec<Entity> = [wall, wall + Vec3::new(0., 0.2, 0.)]
            .iter()
            .map(|at| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Scuttlebug,
                            animation: Handle::default(),
                        },
                        Crawler {
                            up: Vec3::NEG_X,
                            heading: Vec3::Y,
                        },
                        Transform::from_translation(*at),
                    ))
                    .id()
            })
            .collect();
        for _ in 0..60 {
            world.run_system_once(spread).expect("spread could not run");
        }
        for bug in pair {
            let at = world.get::<Transform>(bug).unwrap().translation;
            assert!(
                (at.x - wall.x).abs() < 1e-4,
                "a bug was shoved off its wall, to {at:?}"
            );
        }
    }

    /// The amble: a spot in its own patch, a walk to it, and a rest before the
    /// next one. The rest is the point -- a walker that picks a new spot the
    /// moment it arrives never stands still and never finishes a stride.
    #[test]
    fn a_wandering_enemy_walks_its_patch_and_rests_between_spots() {
        let home = Vec3::new(10., 0., -4.);
        let mut wander = Wander::new(home, 1.0);
        let goal_after =
            |wander: &mut Wander, at: Vec3| (0..600).find_map(|_| wander.goal(at, FIXED_DT));
        let first = goal_after(&mut wander, home).expect("it never set off anywhere");
        assert!(
            Vec2::new(first.x - home.x, first.z - home.z).length() <= WANDER_RADIUS + 1e-4,
            "it ambled clean out of its patch, to {first:?}"
        );
        // Stood on the spot, it stops rather than turning straight round.
        assert_eq!(wander.goal(first, FIXED_DT), None, "it never stops");
        assert_eq!(wander.goal(first, FIXED_DT), None, "its rest was one tick");
        let next = goal_after(&mut wander, first).expect("it never set off again");
        assert!(next != first, "it went back to the spot it was already on");
    }

    /// A Mario and a goomba, stood within arm's reach of each other, with the
    /// Mario having noticed it.
    fn duel(apart: f32) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.insert_resource(SoundQueue::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::default(),
            ))
            .id();
        let goomba = world
            .spawn((
                Enemy {
                    kind: Kind::Goomba,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Transform::from_xyz(apart, 0., 0.),
            ))
            .id();
        world.get_mut::<Aggro>(mario).unwrap().target = Some(goomba);
        (world, mario, goomba)
    }

    fn swing(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world
                .run_system_once(ally_combat)
                .expect("ally_combat could not run");
        }
    }

    /// A Mario stood over something on the other side hits it, and what it hits
    /// dies. It takes the length of the punch to do it: the blow lands when the
    /// swing finishes, not when it starts.
    #[test]
    fn a_mario_punches_what_it_has_noticed() {
        let (mut world, mario, goomba) = duel(1.0);
        swing(&mut world, 1);
        assert!(
            world.get_entity(goomba).is_ok(),
            "the goomba died on the wind-up"
        );
        assert_eq!(
            world.get::<Ally>(mario).unwrap().state.motion,
            Motion::Attack
        );
        swing(&mut world, (MARIO_SWING / FIXED_DT).ceil() as usize + 1);
        assert!(
            world.get_entity(goomba).is_err(),
            "the punch landed on nothing"
        );
    }

    /// And one that wanders out of reach while the punch is in the air is
    /// missed, which is the only thing that makes standing next to a Mario
    /// survivable.
    #[test]
    fn a_mario_misses_what_walks_out_of_the_punch() {
        let (mut world, _, goomba) = duel(1.0);
        swing(&mut world, 1);
        world.get_mut::<Transform>(goomba).unwrap().translation.x = MARIO_REACH + 3.0;
        swing(&mut world, (MARIO_SWING / FIXED_DT).ceil() as usize + 1);
        assert!(
            world.get_entity(goomba).is_ok(),
            "the punch followed it across the lawn"
        );
    }

    /// Out of reach to begin with, a Mario does not swing at the air: walking
    /// to it is the movement step's business.
    #[test]
    fn a_mario_does_not_punch_at_something_across_the_lawn() {
        let (mut world, mario, goomba) = duel(MARIO_REACH + 2.0);
        swing(&mut world, 30);
        assert!(world.get_entity(goomba).is_ok());
        assert_eq!(world.get::<Ally>(mario).unwrap().swing_left, 0.0);
    }

    /// Both sides notice each other, off the one rule. The Mario has a goomba
    /// to hit and the goomba has a Mario to chase, out of a single pass.
    #[test]
    fn a_mario_and_a_goomba_notice_each_other() {
        let mut world = World::new();
        world.insert_resource(GameTuning::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::default(),
            ))
            .id();
        let goomba = world
            .spawn((
                Enemy {
                    kind: Kind::Goomba,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Aggro::default(),
                Transform::from_xyz(4., 0., 0.),
            ))
            .id();
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(world.get::<Aggro>(mario).unwrap().target, Some(goomba));
        assert_eq!(world.get::<Aggro>(goomba).unwrap().target, Some(mario));
        // And what a Mario kills stops being a target, so it looks for the next
        // one rather than standing over the spot.
        world.despawn(goomba);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(world.get::<Aggro>(mario).unwrap().target, None);
    }

    /// A step at `height`: ground to the west of the origin, a face rising at
    /// x = 0, and more ground on top of it to the east. A kerb and a cliff are
    /// the same shape at two heights, which is the point.
    fn ledge(height: f32) -> LevelData {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let base = vertices.len() as u32;
            vertices.extend([a, b, c, d]);
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        };
        quad(
            Vec3::new(-9., 0., -9.),
            Vec3::new(0., 0., -9.),
            Vec3::new(0., 0., 9.),
            Vec3::new(-9., 0., 9.),
        );
        quad(
            Vec3::new(0., 0., -9.),
            Vec3::new(0., height, -9.),
            Vec3::new(0., height, 9.),
            Vec3::new(0., 0., 9.),
        );
        quad(
            Vec3::new(0., height, -9.),
            Vec3::new(9., height, -9.),
            Vec3::new(9., height, 9.),
            Vec3::new(0., height, 9.),
        );
        LevelData::new(vertices, triangles, Vec::new())
    }

    /// Walks a goomba east for a while and reports where it got to.
    fn trudge(level: &LevelData, from: Vec3, ticks: usize) -> Vec3 {
        let mut at = from;
        for _ in 0..ticks {
            at = walk(level, at, Vec3::X * 0.06, Kind::Goomba.body().0);
            at = settle(level, at, FIXED_DT);
        }
        at
    }

    /// The bug this was written for. A goomba at the bottom of a cliff stays at
    /// the bottom of the cliff: the top is not a step, however near it is in a
    /// straight line, and being handed it by the floor query is not the same as
    /// having climbed it.
    #[test]
    fn a_walker_does_not_arrive_on_top_of_a_cliff() {
        let cliff = ledge(2.5);
        let at = trudge(&cliff, Vec3::new(-3., 0., 0.), 200);
        assert!(
            at.y < 0.01,
            "it got up a two-and-a-half unit cliff, to {at:?}"
        );
        assert!(at.x < 0.01, "it walked into the cliff, to {at:?}");
        // It did set off, rather than being stuck from the first tick.
        assert!(at.x > -3.0 + 0.5, "it never walked anywhere: {at:?}");
    }

    /// And it is not stopped by everything: a kerb it could step onto it steps
    /// onto, and a slope it could walk up it walks up.
    #[test]
    fn a_walker_steps_up_what_it_can() {
        let kerb = ledge(0.35);
        let at = trudge(&kerb, Vec3::new(-3., 0., 0.), 200);
        assert!(
            (at.y - 0.35).abs() < 0.01 && at.x > 1.0,
            "it was stopped by a kerb, ending at {at:?}"
        );
    }

    /// Ground is followed at a walking pace rather than assigned, which is the
    /// other half of the same complaint: the floor query answers a column, a
    /// column's answer jumps, and taking it whole is a walker that morphs
    /// between elevations instead of walking between them.
    #[test]
    fn a_walker_climbs_and_falls_at_a_pace() {
        let kerb = ledge(0.35);
        // Stood at the foot of the step, in the column its top occupies.
        let climbing = settle(&kerb, Vec3::new(0.5, 0., 0.), FIXED_DT);
        assert!(
            (climbing.y - CLIMB_SPEED * FIXED_DT).abs() < 1e-6,
            "it climbed {} in one tick",
            climbing.y
        );
        // Dropped from above it, it comes down faster than it goes up, and it
        // settles exactly rather than shivering around the ground.
        let mut at = Vec3::new(-3., 6., 0.);
        let first = settle(&kerb, at, FIXED_DT);
        assert!((first.y - (6.0 - FALL_SPEED * FIXED_DT)).abs() < 1e-6);
        for _ in 0..300 {
            at = settle(&kerb, at, FIXED_DT);
        }
        assert_eq!(at.y, 0.0);
    }

    /// No two enemies walk quite alike: they keep different paces, make for
    /// different spots around what they are chasing, and weave differently on
    /// the way. A brood that shares one number for all of it marches.
    #[test]
    fn no_two_enemies_walk_quite_alike() {
        let (first, second) = (Quirk::new(1.0), Quirk::new(2.0));
        assert!((first.pace() - second.pace()).abs() > 0.01);
        assert!(first.stand_off().distance(second.stand_off()) > 0.1);
        assert!((first.weave(3.0) - second.weave(3.0)).abs() > 0.01);
        // Within bounds, both of them: a quirk is a difference, not a licence.
        for quirk in [&first, &second] {
            assert!((quirk.pace() - 1.0).abs() <= PACE_SPREAD + 1e-6);
            assert!(quirk.stand_off().length() <= STAND_OFF + STAND_OFF_SPREAD + 1e-6);
            assert!(quirk.weave(7.0).abs() <= WEAVE_WIDTH + 1e-6);
        }
    }

    /// A player, and one enemy standing on his toes.
    fn world(player_y: f32, velocity: Vec3) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(SoundQueue::default());
        let mut controller = Controller::default();
        controller.velocity = velocity;
        world.spawn((Player, Transform::from_xyz(0.0, player_y, 0.0), controller));
        let enemy = world
            .spawn((
                Enemy {
                    kind: Kind::Goomba,
                    animation: Handle::default(),
                },
                Transform::from_xyz(0.5, 0.0, 0.0),
            ))
            .id();
        (world, enemy)
    }

    /// Velocity, health and immunity: everything these tests read.
    fn controller(world: &mut World) -> (Vec3, u8, f32) {
        let mut query = world.query_filtered::<&Controller, With<Player>>();
        let ctrl = query.single(world).unwrap();
        (ctrl.velocity, ctrl.health, ctrl.invulnerable_left)
    }

    /// Walking into one costs a heart and throws the player clear.
    #[test]
    fn touching_an_enemy_hurts_and_knocks_the_player_back() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok(), "the enemy died on contact");
        let (velocity, health, immune) = controller(&mut world);
        assert_eq!(health, 2);
        assert!(velocity.x < 0.0, "not thrown away from it: {velocity:?}");
        assert!(velocity.y > 0.0);
        assert_eq!(immune, INVULNERABLE_SECONDS);
    }

    /// The reported bug, at its root. A player thrown into the air by a hit
    /// comes back down on the enemy that hit him, and the descent alone would
    /// stomp it -- so an enemy that touches somebody standing perfectly still
    /// destroys itself, and a warp pipe looks like it spawns nothing.
    #[test]
    fn an_enemy_is_not_stomped_by_the_fall_from_its_own_hit() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        // Now airborne from the knockback, coming down on its head.
        {
            let mut query = world.query_filtered::<&mut Transform, With<Player>>();
            query.single_mut(&mut world).unwrap().translation.y = 0.8;
            let mut query = world.query_filtered::<&mut Controller, With<Player>>();
            query.single_mut(&mut world).unwrap().velocity = Vec3::new(0.0, -6.0, 0.0);
        }
        for _ in 0..20 {
            world.run_system_once(combat).expect("combat could not run");
        }
        assert!(
            world.get_entity(enemy).is_ok(),
            "the enemy stomped itself on a player who did nothing"
        );
        assert_eq!(controller(&mut world).1, 2, "hit again while immune");
    }

    /// Coming down on one deliberately still defeats it. Without this the
    /// test above would pass just as well if stomping had been removed.
    #[test]
    fn landing_on_an_enemy_defeats_it() {
        let (mut world, enemy) = world(0.8, Vec3::new(0.0, -6.0, 0.0));
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_err(), "the stomp did nothing");
        let (velocity, health, _) = controller(&mut world);
        assert_eq!(velocity.y, BOUNCE_VELOCITY);
        assert_eq!(health, 3, "a stomp is not supposed to hurt");
    }

    /// A scuttlebug hanging from a ceiling reaches down out of it, and the
    /// player walking underneath is walking into the bug rather than under it.
    /// Measured upright -- from its feet upwards -- it would be a storey away.
    #[test]
    fn an_enemy_hanging_upside_down_reaches_downwards() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        {
            let mut query = world.query_filtered::<&mut Transform, Without<Player>>();
            let mut transform = query.single_mut(&mut world).unwrap();
            transform.translation = Vec3::new(0.5, 2.2, 0.0);
            transform.rotation = orientation(Vec3::NEG_Y, Vec3::X).unwrap();
        }
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok(), "it was somehow stomped");
        assert_eq!(
            controller(&mut world).1,
            2,
            "hung out of reach of the player"
        );
    }

    /// Standing on a roof directly above one is not touching it.
    #[test]
    fn an_enemy_a_storey_below_is_out_of_reach() {
        let (mut world, enemy) = world(4.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok());
        assert_eq!(controller(&mut world).1, 3, "hurt from a storey up");
    }
}
