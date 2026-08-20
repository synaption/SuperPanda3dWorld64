//! The things that can be fought, and how they resolve against the player.
//!
//! The combat rules are ported from `Interactions.resolve` in
//! `sm64py/objects.py`, and every distance is that build's, converted from
//! SM64 units to the port's world scale of 1/100.

use crate::{
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    level::LevelData,
    player::{Controller, Player, FIXED_DT, PLAYER_HEIGHT},
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

/// How much room two enemies keep between their bodies, on top of the two
/// bodies themselves.
///
/// They are held apart as the cylinders they are already fought as, so this is
/// only the daylight between them -- but without some, a crowd all chasing the
/// same player converges on the same spot and stacks up into one enemy with
/// several models in it.
const PERSONAL_SPACE: f32 = 0.35;

/// How much of the overlap between two enemies is taken out per tick.
///
/// Not all of it: both of them are pushed, so a pair closes half the gap
/// between them each tick anyway, and shoving the whole overlap out at once
/// makes a dense crowd pop rather than settle.
const SPREAD_RATE: f32 = 0.5;

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

#[derive(Component)]
pub struct Enemy {
    /// What it is, which is also its collision cylinder and its model: kept as
    /// the one fact rather than as a copy of each thing derived from it.
    pub kind: Kind,
    pub animation: Handle<AnimationClip>,
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
            Aggro::default(),
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

/// Every enemy's whereabouts and what it has noticed.
type Crowd<'w, 's> =
    Query<'w, 's, (Entity, &'static Transform, &'static mut Aggro), (With<Enemy>, Without<Player>)>;

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
    ),
    (Without<Player>, Without<crate::pipe::Launched>),
>;

/// Who has noticed the player, and who has been told about it.
///
/// Two things, because they are the same thing: an enemy notices the player by
/// itself when he comes within `enemy_sight`, and the moment one does, every
/// other enemy within `enemy_alert` of it hears the alarm -- and so does every
/// enemy within `enemy_alert` of *them*, out and out until the chain runs out
/// of neighbours. A crowd with one lookout in it is a crowd that all turns
/// round at once, which is what makes walking into the middle of one a mistake
/// rather than a series of separate small mistakes.
///
/// Only enemies that took the alarm this tick pass it on. An enemy already
/// chasing you is not shouting about it forever, or the alarm would creep
/// across the whole field through whatever incidental pairs happen to drift
/// within earshot of each other.
///
/// "Aligned" is every other enemy: they are all one side here, and the day they
/// are not is the day this grows a side to compare rather than another system.
pub fn alert(
    tuning: Res<GameTuning>,
    player: Query<(Entity, &Transform), With<Player>>,
    whereabouts: Query<&Transform>,
    mut enemies: Crowd,
) {
    let Ok((hero, hero_transform)) = player.single() else {
        return;
    };
    let hero_at = hero_transform.translation;
    let crowd: Vec<(Entity, Vec3, Option<Entity>)> = enemies
        .iter()
        .map(|(entity, transform, aggro)| (entity, transform.translation, aggro.target))
        .collect();
    let mut targets: Vec<Option<Entity>> = crowd.iter().map(|(_, _, target)| *target).collect();
    // Whoever can see him from where they are standing. These are the seeds of
    // the chain, and the only enemies that shout.
    let sight = tuning.enemy_sight;
    let mut shouting: Vec<usize> = Vec::new();
    for (index, (_, position, target)) in crowd.iter().enumerate() {
        if target.is_none() && position.distance_squared(hero_at) < sight * sight {
            targets[index] = Some(hero);
            shouting.push(index);
        }
    }
    // And the chain itself: everything within earshot of a shout takes the same
    // target and shouts in its turn. Bucketed by earshot so that a field of
    // five thousand does not cost every pair of them.
    let earshot = tuning.enemy_alert;
    let crowd_grid = Neighbourhood::new(crowd.iter().map(|(_, at, _)| *at), earshot);
    let mut heard = Vec::new();
    while let Some(index) = shouting.pop() {
        let (_, from, _) = crowd[index];
        crowd_grid.near(from, &mut heard);
        for &other in &heard {
            if targets[other].is_some()
                || crowd[other].1.distance_squared(from) >= earshot * earshot
            {
                continue;
            }
            targets[other] = targets[index];
            shouting.push(other);
        }
    }
    // Written back by entity rather than by position in the iteration, which is
    // not a thing to bet a chase on.
    let decided: HashMap<Entity, Option<Entity>> = crowd
        .iter()
        .zip(&targets)
        .filter(|((_, _, was), now)| was != *now)
        .map(|((entity, _, _), now)| (*entity, *now))
        .collect();
    for (entity, transform, mut aggro) in &mut enemies {
        if let Some(target) = decided.get(&entity) {
            aggro.target = *target;
        }
        // Where the target is now, for the movement step to walk towards. An
        // enemy whose target has been despawned keeps the last place it saw it
        // and goes there, which is the closest thing to looking for it.
        if let Some(target) = aggro.target {
            if let Ok(seen) = whereabouts.get(target) {
                aggro.at = seen.translation;
            }
        } else {
            aggro.at = transform.translation;
        }
    }
}

/// Holds enemies out of one another, so that a crowd chasing one player stays a
/// crowd rather than converging into a single stack of models.
///
/// A positional shove rather than a force: they are already resolved against
/// the player as cylinders, and this resolves them against each other the same
/// way. Crawlers are pushed within the surface they are stuck to -- shoving a
/// bug off its wall is the one thing this must not do -- and the walkers within
/// the horizontal plane, where their own floor query will catch them.
pub fn spread(mut enemies: Jostling) {
    let crowd: Vec<(Entity, Vec3, f32, Vec3)> = enemies
        .iter()
        .map(|(entity, enemy, transform, crawler)| {
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
            let overlap = room - apart.length();
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
            if let Ok((_, _, mut transform, _)) = enemies.get_mut(entity) {
                transform.translation += push;
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
#[derive(Component)]
pub struct EnemyAnimationRoot(pub Entity);

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
        &'static mut Transform,
        &'static mut Visibility,
        &'static Aggro,
        &'static mut Wander,
        Option<&'static mut Crawler>,
    ),
    (With<Enemy>, Without<Player>, Without<crate::pipe::Launched>),
>;

pub fn update(
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    mut enemies: WalkingEnemies,
    tuning: Res<GameTuning>,
    mut fixed_tick: Local<u32>,
) {
    let Ok(player) = player.single() else {
        return;
    };
    let player = player.translation;
    *fixed_tick = fixed_tick.wrapping_add(1);
    let tick = *fixed_tick;
    enemies.par_iter_mut().for_each(
        |(entity, mut transform, mut visibility, aggro, mut wander, crawler)| {
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
            // chased at the chase speed and never given up on; the rest of the
            // time it ambles around its own patch, and standing about between
            // one spot and the next is a goal of `None`.
            let (goal, speed) = match aggro.target {
                Some(_) => (Some(aggro.at), tuning.enemy_speed),
                None => (wander.goal(transform.translation, dt), WANDER_SPEED),
            };
            let Some(mut crawler) = crawler else {
                // The plain walkers stay in the horizontal plane and are
                // dropped onto whatever floor is under them.
                if let Some(goal) = goal {
                    let dir = tangent(goal - transform.translation, Vec3::Y);
                    transform.translation += dir * dt * speed;
                    transform.rotation = Quat::from_rotation_y(dir.x.atan2(dir.z));
                }
                if let Some(floor) = ground_under(&level, transform.translation) {
                    transform.translation.y = floor;
                }
                return;
            };
            // A crawler heads for the same goal, but only along the surface it
            // is on, and only as fast as it can turn. Standing about, it is
            // still asked to walk nowhere in the direction it already faces:
            // that re-seats it on ground that may have shifted under it.
            let (goal, step) = match goal {
                Some(goal) => (goal, speed * dt),
                None => (transform.translation + crawler.heading, 0.0),
            };
            transform.translation = crawl_towards(
                &level,
                transform.translation,
                &mut crawler,
                goal,
                step,
                TURN_RATE * dt,
            );
            transform.rotation =
                orientation(crawler.up, crawler.heading).unwrap_or(transform.rotation);
        },
    );
}

/// The height an enemy stands at over `position`: the floor under it, or, when
/// there is none, the top of whatever it is inside.
///
/// The second half is not a curiosity. Several of the level's own placements
/// are authored below the lawn they are meant to be standing on, and an enemy
/// that only ever looks *down* for ground never finds any -- it spends the
/// session sliding about inside the hill. What is over its head there is the
/// underside of that hill, so the thing to do with it is climb out.
fn ground_under(level: &LevelData, position: Vec3) -> Option<f32> {
    level
        .floor_height(position + Vec3::Y * 2.0)
        .or_else(|| level.ceiling_height(position, 0.0))
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
    step: f32,
    turn: f32,
) -> Vec3 {
    // The part of the way to the goal it could actually walk. Behind a wall,
    // that is the wall -- and a bug that walks into a wall climbs it.
    let wanted = tangent(goal - position, crawler.up);
    crawler.heading = steer(crawler.heading, wanted, crawler.up, turn);
    match crawl(level, position, crawler.up, crawler.heading * step) {
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
            let drifted = position + crawler.heading * step;
            match ground_under(level, drifted) {
                Some(floor) => {
                    crawler.up = Vec3::Y;
                    Vec3::new(drifted.x, floor, drifted.z)
                }
                None => drifted,
            }
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
    enemies: Query<(Entity, &Enemy, &Transform), Without<Player>>,
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
    for (entity, enemy, transform) in &enemies {
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

/// Hidden skinned actors do not need their bones evaluated. This runs after
/// the console-wide animation pause so visible enemies resume while culled
/// enemies remain stopped.
pub fn sync_animation_visibility(
    console: Res<crate::console::ConsoleState>,
    roots: Query<&Visibility, With<Enemy>>,
    mut players: Query<(&EnemyAnimationRoot, &mut AnimationPlayer)>,
) {
    for (root, mut player) in &mut players {
        let hidden = roots
            .get(root.0)
            .map_or(true, |visibility| *visibility == Visibility::Hidden);
        if console.open || hidden {
            player.pause_all();
        } else {
            player.resume_all();
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
    fn walk(level: &LevelData, start: (Vec3, Vec3), goal: Vec3, steps: usize) -> Vec<(Vec3, Vec3)> {
        let (mut position, up) = start;
        let mut crawler = Crawler {
            up,
            heading: tangent(goal - position, up),
        };
        (0..steps)
            .map(|_| {
                position = crawl_towards(level, position, &mut crawler, goal, 0.1, TURN_RATE / 30.);
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
        let trail = walk(
            &level,
            (Vec3::new(-3., 0., 0.), Vec3::Y),
            Vec3::new(10., 4., 0.),
            200,
        );
        let climbed = trail
            .iter()
            .find(|(position, up)| up.x < -0.99 && position.y > 0.5)
            .unwrap_or_else(|| panic!("it never climbed the wall: {:?}", trail.last()));
        assert!(
            (climbed.0.x - 4.).abs() < 0.1,
            "climbing thin air: {climbed:?}"
        );
        let hanging = trail
            .iter()
            .find(|(_, up)| up.y < -0.99)
            .unwrap_or_else(|| panic!("it never made it onto the ceiling: {:?}", trail.last()));
        assert!(
            (hanging.0.y - 4.).abs() < 0.1,
            "hanging off nothing: {hanging:?}"
        );
        // And once there it walks the ceiling like any other floor, which the
        // corner it climbed in at would hide.
        let across = walk(&level, *hanging, Vec3::new(-10., 4., 0.), 40);
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
                position = crawl_towards(
                    &level,
                    position,
                    &mut crawler,
                    goal,
                    4.0 * FIXED_DT,
                    TURN_RATE * FIXED_DT,
                );
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

    /// A player at the origin and enemies stood where they are asked for.
    fn field(placed: &[Vec3]) -> (World, Entity, Vec<Entity>) {
        let mut world = World::new();
        world.insert_resource(GameTuning::default());
        let player = world.spawn((Player, Transform::default())).id();
        let enemies = placed
            .iter()
            .map(|at| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Goomba,
                            animation: Handle::default(),
                        },
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
        let room = Kind::Goomba.body().0 * 2.0 + PERSONAL_SPACE;
        assert!(
            apart.length() > room - 0.01,
            "two enemies settled {} apart, inside the {room} they are owed",
            apart.length()
        );
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
