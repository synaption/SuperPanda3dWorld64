//! The Marios: a field of allies, whistled into a squad and sent somewhere.
//!
//! Ported from `sm64py/squad.py`. Two commands on one button, told apart by
//! how long it is held:
//!
//!   * **held** -- a circle grows on the ground where the view is pointing, up
//!     to a cap. Every ally inside it when the button comes up joins the squad
//!     and follows.
//!   * **tapped** -- the squad is sent to whatever spot the same aim resolves
//!     to. They walk there, spread out around it, and are on their own again
//!     once they arrive.
//!
//! Pikmin's shape rather than an RTS's: there is no cursor to drag a box with,
//! so the selection is aimed the same way a throw would be.
//!
//! The aiming and the formation are plain arithmetic over positions and the
//! level's collision, so both are exercised headless -- the Panda3D build
//! checks the same maths in `tools/check_squad.py`.
//!
//! Every distance here is the Panda3D build's, converted from SM64 units to
//! the port's world scale of 1/100.

use crate::{
    animation::{AllyAnimationRoot, AnimationState, CharacterAnimations},
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    level::LevelData,
    player::{Player, PLAYER_HEIGHT},
    ActiveCharacter,
};
use bevy::prelude::*;

// -- aiming -----------------------------------------------------------------

/// How near and how far the aim may land, measured out from the player in the
/// horizontal plane. Beyond the far one the throw stops meaning anything, and
/// inside the near one the circle is drawn around his own feet.
const AIM_MIN_RANGE: f32 = 2.5;
const AIM_MAX_RANGE: f32 = 26.0;

/// The ray march that turns a view direction into a spot on the ground: how
/// far apart the samples are, how many times the crossing is then halved, and
/// how far above a sample the floor is looked for. The probe has to cover a
/// whole step or a sample landing just under a slope reads as above it.
const AIM_STEP: f32 = 1.5;
const AIM_REFINE: u32 = 6;
const AIM_PROBE: f32 = AIM_STEP + 0.8;

/// Walking the target back toward the player when the ray never met ground --
/// which over the moat and off the edge of the map is the usual answer.
const AIM_BACKOFF: f32 = 2.0;

/// Longer than a tap, in seconds. Under this the press is an order to the
/// squad it already has; over it, a whistle for a new one.
pub const TAP_SECONDS: f32 = 0.18;

/// The whistle circle: where it starts, where it stops, and how long it takes
/// to grow between the two.
const CIRCLE_MIN_RADIUS: f32 = 2.2;
const CIRCLE_MAX_RADIUS: f32 = 11.0;
const CIRCLE_GROW_SECONDS: f32 = 1.1;

/// How far above his feet an ally may be and still be whistled. A circle is a
/// flat thing drawn on the ground and reads as one, so the height is generous
/// but not unbounded: somebody on the castle roof is not in a circle drawn on
/// the lawn beneath him.
const RECRUIT_HEIGHT: f32 = 8.0;

// -- the formation ----------------------------------------------------------

/// Where the group gathers relative to the leader, and how far apart its
/// members stand once they are there.
const FOLLOW_DISTANCE: f32 = 3.3;
const FOLLOW_SPACING: f32 = 1.7;
const FOLLOW_ARRIVE: f32 = 1.1;

/// The same, for a spot they have been sent to. Wider, because nothing is
/// moving the target around and a tight cluster there just means they shove
/// each other.
const SEND_SPACING: f32 = 2.0;
pub(crate) const SEND_ARRIVE: f32 = 1.4;

/// The angle between one slot in a cluster and the next. The golden angle is
/// what keeps a spiral from lining its points up into spokes, which is the
/// same reason a sunflower uses it: any simpler step leaves the allies in rows
/// with gaps between them.
pub(crate) const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// How near an ally has to be before it counts as standing on its slot.
const ALLY_RADIUS: f32 = 0.4;

/// How close a Mario walks to what it is going to hit, measured from that
/// thing's body rather than from its middle. Inside its own punch's reach, so
/// that arriving and connecting are not two separate strides.
///
/// **Wider than `enemy::PERSONAL_SPACE`, and that is the constraint rather than
/// a preference.** `enemy::spread` will not let a Mario stand nearer a body than
/// the two radii plus that gap, so a strike range shorter than it is an order to
/// stand somewhere the shove immediately undoes -- and the Mario spends the
/// fight being walked in and pushed out, inside the thing it is punching. As an
/// absolute 1.2 m it was a metre and a half inside an ant, which is exactly what
/// that looked like.
pub(crate) const STRIKE_RANGE: f32 = 0.5;

/// The amble an ally falls back on with nobody to follow: how far from where
/// it was left it will wander, how near it has to get before that counts as
/// arriving, and how long it stands about afterwards before ambling somewhere
/// else.
///
/// It is a walk to a fixed spot followed by a rest, rather than a point the
/// ally continuously chases. That distinction is the whole of it: the first
/// version orbited a target around the ally at a speed it could out-walk in
/// one step, so it alternated between walking and standing every few ticks.
/// Each of those changes restarts the clip, and a walk cycle restarted three
/// times a second never gets past its first frames -- which is a field of
/// Marios stuck mid-stride, going nowhere.
const WANDER_RADIUS: f32 = 6.0;
const WANDER_ARRIVE: f32 = 0.5;
const WANDER_REST: f32 = 2.0;
const WANDER_REST_SPREAD: f32 = 3.0;

/// How fast it ambles, which is not how fast it follows: an ally under orders
/// has to keep up with the player, and one with nowhere to be does not. This
/// is the speed Mario's walk clip was authored to cover ground at, so an
/// ambling Mario's feet are planted -- the same reason the amble is long
/// enough to be a walk at all rather than a step and a stop.
const AMBLE_SPEED: f32 = crate::animation::MARIO_STRIDE_SPEED;

/// An ally in the field. The only thing the squad writes onto one is its goal;
/// an ally with none is nobody's business and goes back to ambling.
#[derive(Component)]
pub struct Ally {
    /// Where it should be standing, and how near counts as there.
    pub goal: Option<(Vec2, f32)>,
    /// What it decided to do with itself this tick.
    ///
    /// Written by [`crate::goap::plan`] and read by nothing but [`move_allies`],
    /// which walks towards whatever it says and asks no questions about why.
    /// [`Self::goal`] is still the *record* of what the player ordered; this is
    /// the decision about whether that is the thing to be doing right now, which
    /// is a different question and now has its own module. See [`crate::goap`]
    /// for why a priority chain could not answer it.
    pub plan: crate::goap::Goal,
    /// Where it ambles around when it has no goal: the spot it was left, the
    /// spot it is currently walking to, how long it still has to stand about
    /// first, and its phase, which is what keeps a crowd from moving as one
    /// body.
    home: Vec3,
    stroll: Vec2,
    rest_left: f32,
    phase: f32,
    pub velocity: Vec3,
    pub state: AnimationState,
    /// How long is left of the punch it is throwing, or zero.
    ///
    /// Thrown and resolved by [`crate::enemy::ally_combat`]; kept here because
    /// a Mario in the middle of one stands still to throw it, which is this
    /// module's business.
    pub swing_left: f32,
    /// How long it still cannot be hurt for, which is what stops a Mario
    /// standing in a crowd losing all twenty points in a third of a second.
    ///
    /// The player's equivalent lives on [`crate::player::Controller`] and does
    /// the same job for the same reason. Written down by
    /// [`crate::enemy::maul`] and counted off by it.
    pub hurt_left: f32,
    /// Whether it was out of its depth last tick.
    ///
    /// Kept rather than recomputed where it is wanted, because what it is for is
    /// noticing the *edge*: entering and leaving the water is a splash, and a
    /// depth read fresh each tick can say "wet" a hundred times running without
    /// anything having happened. [`crate::player::Controller::submersion`] is
    /// the same field doing the same job for the player.
    pub swimming: bool,
}

impl Ally {
    /// A new Mario standing where it was put, about to amble.
    pub fn new(home: Vec3, phase: f32) -> Self {
        let mut ally = Self {
            goal: None,
            plan: crate::goap::Goal::Idle,
            home,
            stroll: Vec2::new(home.x, home.z),
            rest_left: 0.0,
            phase,
            velocity: Vec3::ZERO,
            state: AnimationState::default(),
            swing_left: 0.0,
            hurt_left: 0.0,
            swimming: false,
        };
        ally.amble_somewhere_else();
        ally
    }

    /// Picks the next spot to amble to, and how long to stand about first.
    ///
    /// The golden angle again, advanced per ally: successive destinations do
    /// not line up into a path that retraces itself, and two allies never pick
    /// the same one at the same moment -- with no random number generator
    /// anywhere, so a whole crowd stays reproducible in a test.
    fn amble_somewhere_else(&mut self) {
        self.phase += GOLDEN_ANGLE;
        let spread = |scale: f32| (self.phase * scale).sin().abs();
        let reach = WANDER_RADIUS * (0.5 + 0.5 * spread(0.37));
        self.stroll = Vec2::new(self.home.x, self.home.z)
            + Vec2::new(self.phase.sin(), self.phase.cos()) * reach;
        self.rest_left = WANDER_REST + WANDER_REST_SPREAD * spread(0.21);
    }

    /// Standing still, wherever that is.
    fn stand(&mut self, dt: f32) {
        self.velocity = Vec3::ZERO;
        self.state.motion = crate::player::Motion::Idle;
        self.state.speed = 0.0;
        self.state.still_for += dt;
    }
}

/// Which allies are following and which have been sent somewhere.
///
/// Entities rather than indices, so a Mario that is despawned mid-order drops
/// out cleanly instead of shuffling the formation under everyone else.
#[derive(Resource, Default)]
pub struct Squad {
    pub members: Vec<Entity>,
    /// Sent to a spot, and whether they are standing on it yet. They keep the
    /// goal once they are: an ally sent somewhere holds it until whistled up
    /// again, which is what makes sending them an order rather than a
    /// suggestion.
    pub sent: Vec<(Entity, Vec2, bool)>,
    /// Where the followers are gathering, kept between ticks so it can trail the
    /// leader rather than be recomputed from where he happens to be facing.
    ///
    /// See [`update_goals`] for what goes wrong without it.
    anchor: Option<Vec2>,
}

/// The live whistle: how long the button has been down and how big the circle
/// has grown, or `None` while nothing is held.
#[derive(Resource, Default)]
pub struct Whistle {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    pub radius: f32,
}

impl Whistle {
    /// The circle is only drawn once the press has outlasted a tap.
    pub fn showing(&self) -> bool {
        self.held_for.is_some_and(|held| held >= TAP_SECONDS)
    }
}

/// Offset of the index'th member of a loose cluster, in the plane.
///
/// Not rotated to face anything: the cluster is placed by its caller, and the
/// leader turning on the spot should not send everyone shuffling around him to
/// keep a formation they were never in.
pub fn slot(index: usize, spacing: f32) -> Vec2 {
    let radius = spacing * (index as f32).sqrt();
    let angle = index as f32 * GOLDEN_ANGLE;
    Vec2::new(radius * angle.sin(), radius * angle.cos())
}

/// How wide the circle has grown after being held this long.
pub fn circle_radius(held_for: f32) -> f32 {
    let grown = ((held_for - TAP_SECONDS) / CIRCLE_GROW_SECONDS).clamp(0.0, 1.0);
    CIRCLE_MIN_RADIUS + (CIRCLE_MAX_RADIUS - CIRCLE_MIN_RADIUS) * grown
}

/// Is this point at or below the floor beneath it?
fn underground(level: &LevelData, point: Vec3) -> bool {
    level
        .floor_height(point + Vec3::Y * AIM_PROBE)
        .is_some_and(|height| point.y <= height)
}

/// Marches a ray until it goes underground, returning how far along it that
/// happened.
///
/// Coarse steps and then a handful of bisections rather than fine steps
/// throughout: this runs every frame the button is held and each sample is a
/// collision query. Six halvings of a 1.5-unit step land the crossing inside
/// two and a half centimetres, far finer than anything downstream of it.
fn ray_ground(
    level: &LevelData,
    origin: Vec3,
    direction: Vec3,
    start: f32,
    end: f32,
) -> Option<f32> {
    let mut previous = start;
    let mut distance = start;
    while distance <= end {
        if underground(level, origin + direction * distance) {
            let (mut low, mut high) = (previous, distance);
            for _ in 0..AIM_REFINE {
                let middle = (low + high) * 0.5;
                if underground(level, origin + direction * middle) {
                    high = middle;
                } else {
                    low = middle;
                }
            }
            return Some(high);
        }
        previous = distance;
        distance += AIM_STEP;
    }
    None
}

/// Where on the ground the crosshair is pointing.
///
/// The crosshair is the middle of the screen and the aim is the ray out of it,
/// marched until it meets ground. Left and right is where the view points; up
/// and down is range, since a view tilted down meets the ground nearer and one
/// tilted up throws the meeting further out. That is the whole of the aim, and
/// it is why the reticle never has to leave the middle of the screen.
///
/// The answer is a point in front of the *player* rather than the ray's own
/// hit -- on the bearing from him to that hit -- so the camera sitting off his
/// shoulder does not skew where the order lands. It is pulled back to
/// `AIM_MAX_RANGE` when it is beyond range, pushed out to `AIM_MIN_RANGE` when
/// the view is pointed at his own feet, and walked back toward him until there
/// is floor under it when it is out over the moat or off the edge of the
/// world. An order does not have to land exactly where it was pointed; it does
/// have to land somewhere.
pub fn aim_point(level: &LevelData, origin: Vec3, direction: Vec3, player: Vec3) -> Vec3 {
    let flat = Vec2::new(direction.x, direction.z).length();
    if flat < 1e-4 {
        // Straight down. Nothing to aim along; put it at his feet.
        return player;
    }
    let heading = Vec2::new(direction.x, direction.z) / flat;
    // The march starts where the player is along the ray rather than at the
    // camera: the ground between the camera and his back is behind him, and a
    // target there points the order the wrong way.
    let start = (player - origin).dot(direction).max(1.0);
    let hit = ray_ground(level, origin, direction, start, start + AIM_MAX_RANGE * 1.5);
    let range = match hit {
        Some(distance) => {
            let point = origin + direction * distance;
            Vec2::new(point.x - player.x, point.z - player.z).length()
        }
        None => AIM_MAX_RANGE,
    };
    let mut range = range.clamp(AIM_MIN_RANGE, AIM_MAX_RANGE);
    // Walk back until there is ground under the target.
    while range > AIM_MIN_RANGE {
        let candidate = Vec3::new(
            player.x + heading.x * range,
            player.y + PLAYER_HEIGHT,
            player.z + heading.y * range,
        );
        if let Some(height) = level.floor_height(candidate) {
            return Vec3::new(candidate.x, height, candidate.z);
        }
        range -= AIM_BACKOFF;
    }
    let x = player.x + heading.x * AIM_MIN_RANGE;
    let z = player.z + heading.y * AIM_MIN_RANGE;
    let y = level
        .floor_height(Vec3::new(x, player.y + PLAYER_HEIGHT, z))
        .unwrap_or(player.y);
    Vec3::new(x, y, z)
}

impl Squad {
    /// Whistles up everyone inside the circle, returning how many joined.
    ///
    /// One already on the way somewhere is called back rather than ignored:
    /// the whistle is how an order is taken back, and an ally who kept walking
    /// to the last spot because he was already walking would read as deaf.
    pub fn recruit(&mut self, inside: &[Entity]) -> usize {
        let mut joined = 0;
        for ally in inside {
            if self.members.contains(ally) {
                continue;
            }
            self.sent.retain(|(sent, _, _)| sent != ally);
            self.members.push(*ally);
            joined += 1;
        }
        joined
    }

    /// Sends the whole squad to a spot, spread around it.
    pub fn send(&mut self, target: Vec2) -> usize {
        let count = self.members.len();
        for (index, ally) in self.members.drain(..).enumerate() {
            self.sent
                .push((ally, target + slot(index, SEND_SPACING), false));
        }
        count
    }

    pub fn disband(&mut self) -> usize {
        let count = self.members.len() + self.sent.len();
        self.members.clear();
        self.sent.clear();
        count
    }

    /// Where the `index`'th follower should be standing, and how near counts.
    ///
    /// Handed out rather than read off `Ally::goal` so that [`crate::goap`] can
    /// score an order without the walk step having already written it down --
    /// the record of what was asked for and the decision about it are separate
    /// things now. `None` before the leader has been seen at all, which is the
    /// first tick of a session.
    pub fn follow_slot(&self, index: usize) -> Option<(Vec2, f32)> {
        self.anchor
            .map(|anchor| (anchor + slot(index, FOLLOW_SPACING), FOLLOW_ARRIVE))
    }

    /// Sent somewhere and not there yet.
    pub fn marching(&self) -> usize {
        self.sent.iter().filter(|(_, _, arrived)| !arrived).count()
    }
}

/// Is an ally inside a whistle circle drawn at `centre`?
pub fn in_circle(ally: Vec3, centre: Vec3, radius: f32) -> bool {
    let flat = Vec2::new(ally.x - centre.x, ally.z - centre.z).length();
    flat <= radius + ALLY_RADIUS && (ally.y - centre.y).abs() <= RECRUIT_HEIGHT
}

/// Puts one ally in the field, as whichever character was asked for.
///
/// **Either playable character can be an ally**, which is the whole of what
/// "Luna is AI-playable too" means here: the squad is not a crowd of Marios
/// with a Luna hard-wired into the player's hands, it is a field of characters
/// of which one happens to be driven by a controller. An AI Luna is the same
/// model at the same scale, animating off the same clip table and fighting
/// with the same rules, as the Luna the player is driving -- see
/// [`crate::ActiveCharacter::model`], which is where both of them get their
/// scene from.
///
/// Shared by the console's population counts and by the Mario warp pipe, so no
/// two callers can produce subtly different allies -- the same reason
/// `enemy::spawn` is shared between the level's placements and the enemy pipes.
pub fn spawn_ally(
    commands: &mut Commands,
    assets: &AssetServer,
    character: ActiveCharacter,
    home: Vec3,
    phase: f32,
) -> Entity {
    let (model, scale) = character.model();
    commands
        .spawn((
            Ally::new(home, phase),
            // Drawn between two ticks rather than at them, the way the player
            // has always been. See [`Glide`].
            Glide::default(),
            // An ally is on the player's side, and goes for what it notices on
            // the other one exactly as an enemy goes for him.
            crate::enemy::Side::Friendly,
            // And can be worn down like one. A Mario's twenty points is seven
            // ant touches -- long enough that one sent at something wins or
            // loses on whether the rest of the squad went with it -- and a Luna
            // carries the player's hundred, which is what makes filling the
            // field with one or the other a decision.
            crate::health::Health::new(character.ally_health()),
            crate::enemy::Aggro::default(),
            // Allies animate off the same tables the playable characters do,
            // and this is which table.
            character,
            // And stand on the ground the same way, so they get the same disc
            // under them as the player.
            crate::shadow::ShadowCaster::new(
                crate::player::PLAYER_RADIUS,
                crate::player::PLAYER_HEIGHT,
            ),
            WorldAssetRoot(assets.load(model)),
            Transform::from_translation(home).with_scale(Vec3::splat(scale)),
        ))
        .id()
}

/// The allies the console's population counts answer for: the field's standing
/// crowd, with the warp pipe's own brood left out of it.
///
/// The character comes with them, because there are two counts now -- one per
/// playable character -- and reconciling either one means knowing which of the
/// standing allies are that one.
type StandingCrowd<'w, 's> =
    Query<'w, 's, (Entity, &'static ActiveCharacter), (With<Ally>, Without<crate::pipe::Brood>)>;

/// Keeps the field's Mario population at whatever the console asks for.
///
/// Spawning is reconciled against a count rather than driven by a command, so
/// the console's existing `<name> <value>` grammar is all it takes to fill the
/// lawn with Marios or clear it -- and the count is a live number rather than
/// a one-shot that cannot be undone.
///
/// The Mario pipe's brood is not in the count. A pipe is responsible for
/// exactly what it produced and for replacing it when it dies, and a count that
/// swept those up would either despawn a Mario the instant it came out of the
/// pipe or stop the lawn filling at all. It is the same rule from the other
/// side: the enemy pipes leave the hand-placed enemies to the level.
pub fn maintain_population(
    mut commands: Commands,
    assets: Res<AssetServer>,
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    allies: StandingCrowd,
    mut squad: ResMut<Squad>,
) {
    // One reconciliation per character, against that character's own count.
    // Two independent numbers rather than a total and a ratio: `ally_count 8`
    // has always meant eight Marios and still does, and asking for four Lunas
    // beside them should not take any of the Marios away.
    let live: Vec<(Entity, ActiveCharacter)> = allies
        .iter()
        .map(|(entity, character)| (entity, *character))
        .collect();
    // Where in the cluster the next arrival stands. Counted across both
    // characters, so a Luna and a Mario are never put down in the same slot.
    let mut placed = live.len();
    for character in ActiveCharacter::ALL {
        let wanted = match character {
            ActiveCharacter::Luna => tuning.luna_count,
            ActiveCharacter::Mario => tuning.ally_count,
        }
        .round() as usize;
        let standing: Vec<Entity> = live
            .iter()
            .filter(|(_, kind)| *kind == character)
            .map(|(entity, _)| *entity)
            .collect();
        if standing.len() > wanted {
            for entity in standing.iter().skip(wanted) {
                squad.members.retain(|member| member != entity);
                squad.sent.retain(|(sent, _, _)| sent != entity);
                commands.entity(*entity).despawn();
                placed -= 1;
            }
            continue;
        }
        let Ok(leader) = player.single() else {
            return;
        };
        // New arrivals stand around the leader in the same cluster the squad
        // uses to follow him, so a crowd summoned from the console is not a
        // pile.
        for _ in standing.len()..wanted {
            let offset = slot(placed, FOLLOW_SPACING * 1.5);
            let x = leader.translation.x + offset.x;
            let z = leader.translation.z + offset.y;
            let y = level
                .floor_height(Vec3::new(x, leader.translation.y + PLAYER_HEIGHT, z))
                .unwrap_or(leader.translation.y);
            let home = Vec3::new(x, y, z);
            spawn_ally(
                &mut commands,
                &assets,
                character,
                home,
                placed as f32 * GOLDEN_ANGLE,
            );
            placed += 1;
        }
    }
}

/// Refreshes every goal, once a tick, before the allies move.
pub fn update_goals(
    mut squad: ResMut<Squad>,
    player: Query<&Transform, With<Player>>,
    mut allies: Query<(&mut Ally, &Transform)>,
) {
    let Ok(leader) = player.single() else {
        return;
    };
    // Drop anyone who is no longer in the field. Their goal goes with them.
    squad.members.retain(|ally| allies.contains(*ally));
    squad.sent.retain(|(ally, _, _)| allies.contains(*ally));

    // Behind the leader, so walking forward drags the group along rather than
    // through him -- but *behind* meaning the side they are already on, not the
    // side his shoulders happen to be pointing away from.
    //
    // Taking it from his facing is what made the formation jitter, and it is
    // worth being precise about why, because the fix looks like a nicety and is
    // not. The anchor sat on a three-metre arm off his back. Turning on the spot
    // -- which a mouse does several times a second and which moves the player
    // nowhere at all -- swept that arm around him at a speed no Mario can walk,
    // and the whole squad spent its time chasing a target orbiting them. On
    // screen: eight Marios shuffling on the spot, never arriving, their walk
    // clips restarting.
    //
    // [`slot`] already says this in its own doc -- "the leader turning on the
    // spot should not send everyone shuffling around him" -- and takes care not
    // to rotate the cluster. The anchor it was placed at then rotated it anyway.
    //
    // A trailing anchor has no facing in it. It stays wherever it is relative to
    // him and is simply held at arm's length, so turning moves it not at all and
    // walking pulls it round behind him on its own.
    let here = Vec2::new(leader.translation.x, leader.translation.z);
    let trail = squad
        .anchor
        .map(|anchor| anchor - here)
        .filter(|arm| arm.length_squared() > 1e-6)
        .unwrap_or_else(|| {
            // Nothing to trail yet -- the first tick, or the leader standing
            // exactly on it. His back is as good a guess as any, and it is only
            // ever a seed.
            let behind = leader.rotation * Vec3::Z;
            -Vec2::new(behind.x, behind.z)
        });
    let anchor = here + trail.normalize_or_zero() * FOLLOW_DISTANCE;
    squad.anchor = Some(anchor);
    for (index, entity) in squad.members.iter().enumerate() {
        if let Ok((mut ally, _)) = allies.get_mut(*entity) {
            ally.goal = Some((anchor + slot(index, FOLLOW_SPACING), FOLLOW_ARRIVE));
        }
    }
    let mut arrivals = Vec::new();
    for (index, (entity, target, arrived)) in squad.sent.iter().enumerate() {
        let Ok((mut ally, transform)) = allies.get_mut(*entity) else {
            continue;
        };
        ally.goal = Some((*target, SEND_ARRIVE));
        let here = Vec2::new(transform.translation.x, transform.translation.z);
        if !arrived && here.distance(*target) <= SEND_ARRIVE {
            arrivals.push(index);
        }
    }
    for index in arrivals {
        squad.sent[index].2 = true;
    }
}

/// How quickly a swimming ally is pulled to the height it floats at, as a
/// fraction of the remaining gap a second.
///
/// A pull rather than a snap for [`settle`]'s reason, and slow enough that
/// walking off a ledge into the moat is a body sinking and bobbing back up
/// rather than one that changes height between two frames.
const SWIM_RISE: f32 = 3.0;

/// How far off a straight line a Mario will swing to keep out of the water, and
/// how many deflections it tries before giving up and wading in.
///
/// Tried outward in pairs, left and right of where it wanted to go, so it takes
/// the smallest detour that works rather than always swinging the same way
/// round a pond. Nine tries at ten degrees covers a right angle either side,
/// which is enough to get round anything short of walking into a bay -- and
/// walking into a bay is what the last resort is for.
const SKIRT_STEP: f32 = std::f32::consts::PI / 18.0;
const SKIRT_TRIES: usize = 9;

/// How far ahead a Mario looks for water, in strides.
///
/// One step is a fortieth of a second of walking and far too short to steer on:
/// by the time the next footfall is wet the one after it is in the middle of the
/// moat. Looking a second or so ahead is what turns this from a body that stops
/// at the water's edge into one that goes round.
const SKIRT_LOOKAHEAD: f32 = 30.0;

/// Bends a step to keep out of deep water.
///
/// Returns the heading to actually walk, as a unit vector. The straight line is
/// tried first and taken whenever it is dry, so this costs one water lookup for
/// everything not near a shore.
///
/// **It gives up rather than refuses**, and that is the whole of what makes it
/// safe. A Mario already standing in the moat, or one whose ball has rolled into
/// it, would otherwise be pinned: every direction is wet, no heading is
/// acceptable, and it stands there for the rest of the session. So a Mario that
/// cannot find a dry way walks the way it wanted to and swims -- see
/// [`move_allies`], which is what actually carries it once it is in.
///
/// Separate from the walk step and taking a `LevelData` rather than a query, so
/// the rule can be exercised against a hand-built water box with no world
/// around it.
pub fn skirt(level: &LevelData, from: Vec3, heading: Vec2, reach: f32) -> Vec2 {
    let dry = |direction: Vec2| {
        let ahead = from + Vec3::new(direction.x, 0.0, direction.y) * reach;
        // The floor under the probe rather than the walker's own height: a
        // shoreline is a slope, and a point measured at the height it is
        // standing now reads as dry right up until it is swimming.
        let ground = level.floor_height(ahead + Vec3::Y * PLAYER_HEIGHT);
        let at = Vec3::new(ahead.x, ground.unwrap_or(ahead.y), ahead.z);
        !level
            .water_depth(at)
            .is_some_and(|depth| depth > SWIMMING_DEPTH)
    };
    if dry(heading) {
        return heading;
    }
    for try_ in 1..=SKIRT_TRIES {
        let turn = try_ as f32 * SKIRT_STEP;
        for side in [turn, -turn] {
            let (sin, cos) = side.sin_cos();
            let bent = Vec2::new(
                heading.x * cos - heading.y * sin,
                heading.x * sin + heading.y * cos,
            );
            if dry(bent) {
                return bent;
            }
        }
    }
    // Ringed by water, or already in it. Go where it meant to.
    heading
}

/// How deep the water has to be before a Mario swims in it rather than walking
/// through it.
///
/// The player's own number, so a Mario in the squad and the Mario the player is
/// driving change behaviour at the same depth -- see
/// [`crate::player::submersion`], which is the rule this mirrors.
pub(crate) const SWIMMING_DEPTH: f32 = crate::player::SUBMERGED_DEPTH;

/// Walks each ally toward its plan, swims it if it is out of its depth, or lets
/// it amble where it stands.
pub fn move_allies(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    mut sounds: ResMut<SoundQueue>,
    // One still in the air out of the Mario pipe is flown by `pipe::fly`, and
    // walking it toward a goal at the same time would drag it out of its arc.
    mut allies: Query<(Entity, &mut Ally, &mut Transform), Without<crate::pipe::Launched>>,
) {
    let dt = crate::player::FIXED_DT;
    for (_entity, mut ally, mut transform) in &mut allies {
        // The order it was given outlives being released by exactly one tick,
        // which is how being released is noticed: it stands where the order left
        // it and ambles from there, rather than walking back to wherever it was
        // recruited. The *plan* is rewritten from scratch every tick by
        // `goap::plan`, so nothing here has to expire it.
        if !matches!(ally.plan, crate::goap::Goal::Obey { .. }) && ally.goal.take().is_some() {
            ally.home = transform.translation;
            ally.amble_somewhere_else();
        }
        // How deep it is standing, which decides everything below: whether it
        // walks or swims, how fast, and whether it is held at the surface or
        // dropped onto the floor.
        let depth = level.water_depth(transform.translation);
        let swimming = depth.is_some_and(|depth| depth > SWIMMING_DEPTH);
        if swimming != ally.swimming {
            ally.swimming = swimming;
            sounds.push_at(Sfx::Splash, transform.translation);
        }
        // Mid-punch it stands where it is and throws it. `ally_combat` owns
        // the swing itself; what it means here is that a Mario is not walking.
        if ally.swing_left > 0.0 {
            ally.velocity = Vec3::ZERO;
            continue;
        }
        // Standing about between ambles -- but only when there is genuinely
        // nothing to do, which is exactly what an idle plan means. An ally with
        // nowhere to be is still for seconds at a time, which is what lets the
        // idle clip actually play.
        if ally.rest_left > 0.0 && !ally.plan.urgent() {
            ally.rest_left -= dt;
            ally.stand(dt);
            settle(&level, &mut transform, depth, swimming, dt);
            continue;
        }
        let here = Vec2::new(transform.translation.x, transform.translation.z);
        // Where it decided to go. Ambling is the only thing without a
        // destination of its own, and it walks to the spot it picked last time.
        //
        // Everything about *choosing* between a fight, an order, a ball and a
        // mast now lives in [`crate::goap`], which is a pure function over one
        // struct. What is left here is a body walking to a point -- see that
        // module for why the two had to come apart.
        let (target, arrive) = ally
            .plan
            .destination()
            .unwrap_or((ally.stroll, WANDER_ARRIVE));
        let to_target = target - here;
        let distance = to_target.length();
        if distance <= arrive {
            if !ally.plan.urgent() {
                // Arrived nowhere in particular: stand about a while, then
                // amble somewhere else. A real plan is somewhere in particular
                // -- picking a new stroll on top of one would send the Mario
                // away from the ball it is standing on before `nuclonium::haul` had
                // a chance to notice it had got there.
                ally.amble_somewhere_else();
            }
            ally.stand(dt);
            settle(&level, &mut transform, depth, swimming, dt);
            continue;
        }
        // Round the pond rather than through it -- unless it is already in the
        // water, or what it is going to get is, in which case the detour is a
        // Mario walking in circles round the thing it came for. See [`skirt`].
        let straight = to_target / distance;
        let goal_is_wet = level
            .water_depth(Vec3::new(target.x, transform.translation.y, target.y))
            .is_some_and(|depth| depth > SWIMMING_DEPTH);
        let heading = if swimming || goal_is_wet {
            straight
        } else {
            skirt(
                &level,
                transform.translation,
                straight,
                SKIRT_LOOKAHEAD * dt,
            )
        };
        // Ease off over the last stride so arriving is not a hard stop. A plan
        // is a job and is walked at the squad's marching pace; ambling is not.
        // Swimming is its own speed, and the player's own -- a Mario in the
        // squad and the Mario the player is driving cross the moat together.
        let pace = match (swimming, ally.plan.urgent()) {
            (true, _) => tuning.mario_swim,
            (false, true) => tuning.ally_speed,
            (false, false) => AMBLE_SPEED,
        };
        let speed = pace * (distance / arrive.max(0.001)).min(1.0);
        let step = heading * speed * dt;
        transform.translation.x += step.x;
        transform.translation.z += step.y;
        // Re-read after the step: it has moved, so the depth it is riding is
        // the depth where it now is rather than where it set off from.
        let arrived_depth = level.water_depth(transform.translation);
        settle(&level, &mut transform, arrived_depth, swimming, dt);
        // Faced along the way it is actually going rather than at the thing it
        // is going to, so a Mario swinging round a bay is not walking sideways
        // for the length of the detour.
        transform.rotation = Quat::from_rotation_y(step.x.atan2(step.y));
        ally.velocity = Vec3::new(step.x / dt, 0.0, step.y / dt);
        ally.state.motion = match swimming {
            // Mario has a swim clip and the ally animation reads the same
            // tables the player does, so this is the whole of making one swim.
            true => crate::player::Motion::Swim,
            false => crate::player::Motion::Run,
        };
        ally.state.speed = speed;
        ally.state.still_for = 0.0;
    }
}

/// Puts an ally at the height it belongs at: floating if it is swimming,
/// standing on the ground if it is not.
///
/// Its own function because three places in [`move_allies`] need it -- walking,
/// arriving and resting -- and an ally that is only re-seated on the ticks it
/// moves is one that sinks to the bottom of the moat the moment it stops.
///
/// The float is a pull rather than a snap, so breaking the surface is a body
/// rising through it rather than a body teleporting to it, and it is the
/// player's own `SWIM_FLOAT_DEPTH` so the two ride the water at the same height.
fn settle(
    level: &LevelData,
    transform: &mut Transform,
    depth: Option<f32>,
    swimming: bool,
    dt: f32,
) {
    if swimming {
        if let Some(depth) = depth {
            let rise = depth - crate::player::SWIM_FLOAT_DEPTH;
            transform.translation.y += rise * (SWIM_RISE * dt).min(1.0);
        }
        return;
    }
    if let Some(height) = level.floor_height(transform.translation + Vec3::Y * PLAYER_HEIGHT) {
        transform.translation.y = height;
    }
}

// -- gliding ----------------------------------------------------------------

/// How far an ally may have moved in one tick and still be drawn as having
/// travelled there.
///
/// A Mario walks four metres a second, so a third of a metre is a busy tick and
/// six metres is not a walk at all -- it is a warp pipe, a level swapping under
/// it, or a body being put somewhere. Interpolating one of those draws the ally
/// sliding across the map over the next thirty-third of a second, through
/// everything in between. `player::sync_visual` has no equivalent because the
/// player is never moved that way with the visual still attached.
const GLIDE_JUMP: f32 = 6.0;

/// Where an ally stood at the end of each of the last two ticks.
///
/// **The simulation runs at thirty steps a second and the game is drawn at
/// whatever the monitor does.** Luna has been drawn between two of those steps
/// since the beginning -- that is [`crate::player::RenderPose`], and it is the
/// only reason she does not judder -- but a Mario was drawn *at* them: the
/// same pose held for two or three frames and then jumped a whole tick's stride
/// at once. Beside a leader who glides, that reads as the squad stuttering
/// along behind her, which is exactly what it is.
///
/// The fix cannot be to interpolate the ally's `Transform` in place, because
/// that transform is not a picture -- it is where the Mario *is*. Forty systems
/// read it: the fight measures reach against it, the planner scores errands off
/// it, the walk steps from it. Smoothing it in place would feed a drawn
/// half-step back into the next tick's arithmetic, and a simulation whose input
/// depends on the frame rate is not a simulation.
///
/// So the pose is banked instead. [`bank`] writes down where the tick left the
/// ally, [`steady`] puts that exact pose back before the next tick reads it,
/// and [`glide`] draws whatever is in between. Every system in the fixed step
/// sees the same numbers it always saw; the drawn frames in between get the
/// smoothing, and nothing that runs on the fixed step can tell the difference.
///
/// One component rather than a field on [`Ally`], because it is nothing to do
/// with being an ally -- it is a body being drawn between two steps, which is
/// what anything simulated on a fixed step and drawn on a variable one needs.
#[derive(Component, Clone, Copy, Default)]
pub struct Glide {
    /// The tick before last, and the last tick. `None` until a tick has
    /// actually happened: an ally spawned this frame has one pose and nothing
    /// to interpolate it against.
    was: Option<(Vec3, Quat)>,
    now: Option<(Vec3, Quat)>,
}

impl Glide {
    /// Where to draw it, `alpha` of the way through the step after the one that
    /// has been simulated.
    ///
    /// A teleport is not interpolated -- see [`GLIDE_JUMP`] -- it is simply
    /// where it now is.
    pub fn between(&self, alpha: f32) -> Option<(Vec3, Quat)> {
        let (was, now) = (self.was?, self.now?);
        if was.0.distance(now.0) > GLIDE_JUMP {
            return Some(now);
        }
        Some((was.0.lerp(now.0, alpha), was.1.slerp(now.1, alpha)))
    }
}

/// Puts every ally back on its simulated pose, before the tick that reads it.
///
/// First in the fixed step, and that is the whole of its correctness: the
/// frames since the last tick have been drawing the ally somewhere between two
/// poses, and the tick about to run must start from the second of them rather
/// than from wherever the last drawn frame happened to leave it. See [`Glide`].
pub fn steady(mut allies: Query<(&Glide, &mut Transform), With<Ally>>) {
    for (glide, mut at) in &mut allies {
        let Some((translation, rotation)) = glide.now else {
            continue;
        };
        at.translation = translation;
        at.rotation = rotation;
    }
}

/// Writes down where this tick left every ally.
///
/// Last in the fixed step, after everything that could have moved one: the walk,
/// the fight, the warp pipe's arc. See [`Glide`].
pub fn bank(mut allies: Query<(&mut Glide, &Transform), With<Ally>>) {
    for (mut glide, at) in &mut allies {
        let pose = (at.translation, at.rotation);
        glide.was = glide.now.or(Some(pose));
        glide.now = Some(pose);
    }
}

/// Draws every ally between the last two ticks, once per frame.
///
/// The same job [`crate::player::sync_visual`] does for Luna, and off the same
/// clock: `overstep_fraction` is how far through the current step the wall
/// clock has got, so an ally is drawn exactly as far along its stride. See
/// [`Glide`].
pub fn glide(
    fixed_time: Res<Time<Fixed>>,
    mut allies: Query<(&Glide, &mut Transform), With<Ally>>,
) {
    let alpha = fixed_time.overstep_fraction().clamp(0.0, 1.0);
    for (glide, mut at) in &mut allies {
        let Some((translation, rotation)) = glide.between(alpha) else {
            continue;
        };
        at.translation = translation;
        at.rotation = rotation;
    }
}

/// Plays each ally's own clip, off the same tables the player uses.
pub fn animate_allies(
    animations: Res<CharacterAnimations>,
    allies: Query<(&Ally, &ActiveCharacter)>,
    mut players: Query<(
        &AllyAnimationRoot,
        &mut AnimationPlayer,
        &mut AnimationTransitions,
    )>,
) {
    for (root, mut player, mut transitions) in &mut players {
        let Ok((ally, character)) = allies.get(root.0) else {
            continue;
        };
        // Asked per ally rather than once for the system: a field can hold
        // Marios and Lunas at the same time, and one glTF finishing loading
        // before the other must not hold up the half that is ready.
        if !animations.ready(*character) {
            continue;
        }
        let (name, rate) = crate::animation::resolve(*character, &ally.state);
        let Some(clip) = animations.named(*character, name) else {
            continue;
        };
        // Through the shared applier rather than played here, so which clips
        // cycle and which hold their last pose is decided in one place for
        // the allies and the player alike.
        crate::animation::apply(&mut player, &mut transitions, clip, name, rate, false);
    }
}

/// The ring drawn on the ground while the whistle is open.
#[derive(Component)]
pub struct WhistleCircle;

/// The ring's own transform.
///
/// Every exclusion here is load-bearing. Bevy proves two queries disjoint from
/// their `Without` filters alone -- `With<WhistleCircle>` and `With<Ally>`
/// describe different entities to a reader, but nothing to the scheduler -- so
/// a write to `Transform` that does not name every other `Transform` query in
/// the same system is rejected when the system is initialised, which in a
/// windowed build is a game that opens and shuts without a word.
type CircleQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (
        With<WhistleCircle>,
        Without<Player>,
        Without<Camera3d>,
        Without<Ally>,
    ),
>;

/// A flat annulus of unit outer radius, scaled to the circle's size when
/// drawn. Built here rather than from a torus so the ring stays flat on the
/// ground and its thickness stays proportional to how wide it has grown.
///
/// Shared with [`crate::stellarator`], which draws a machine's footprint with
/// it. Two rings a player is asked to read while holding two different buttons
/// should be visibly the same kind of mark, and one mesh is how that stays
/// true.
pub fn ring_mesh() -> Mesh {
    const SEGMENTS: usize = 64;
    const INNER: f32 = 0.94;
    let mut positions = Vec::with_capacity(SEGMENTS * 2);
    let mut indices = Vec::with_capacity(SEGMENTS * 6);
    for step in 0..SEGMENTS {
        let angle = step as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        positions.push([sin, 0.0, cos]);
        positions.push([sin * INNER, 0.0, cos * INNER]);
        let outer = (step * 2) as u32;
        let inner = outer + 1;
        let next_outer = ((step + 1) % SEGMENTS * 2) as u32;
        let next_inner = next_outer + 1;
        indices.extend_from_slice(&[outer, inner, next_inner, outer, next_inner, next_outer]);
    }
    let count = positions.len();
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; count]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; count]);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    mesh
}

/// Spawns the (initially hidden) whistle ring. Called from startup.
pub fn spawn_circle(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.spawn((
        WhistleCircle,
        bevy::light::NotShadowCaster,
        Mesh3d(meshes.add(ring_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.94, 0.45, 0.85),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        })),
        Visibility::Hidden,
    ));
}

/// The whistle button: held opens a circle, released gives the order.
///
/// Runs at the render rate rather than on the fixed step, because the circle
/// grows with wall-clock time and is drawn every frame; the orders it produces
/// are one-shot writes onto the squad, which the fixed step then acts on.
#[allow(clippy::too_many_arguments)]
pub fn whistle(
    time: Res<Time>,
    mut input: ResMut<crate::input::InputState>,
    level: Res<LevelData>,
    mut whistle: ResMut<Whistle>,
    mut squad: ResMut<Squad>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
    player: Query<&Transform, With<Player>>,
    allies: Query<(Entity, &Transform), With<Ally>>,
    mut circle: CircleQuery,
) {
    let (Ok(camera), Ok(leader)) = (camera.single(), player.single()) else {
        return;
    };
    let released = crate::input::InputState::take(&mut input.squad_released);
    if input.squad || released {
        // The aim is refreshed on the press as well as on the hold, so a tap
        // too short to have grown a circle still sends the squad somewhere.
        whistle.aim = aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
        );
    }
    if input.squad {
        let held = whistle.held_for.unwrap_or(0.0) + time.delta_secs();
        whistle.held_for = Some(held);
        whistle.radius = circle_radius(held);
    }
    if released {
        let held = whistle.held_for.take().unwrap_or(0.0);
        if held < TAP_SECONDS {
            // A tap is an order to the squad it already has.
            squad.send(Vec2::new(whistle.aim.x, whistle.aim.z));
        } else {
            let inside: Vec<_> = allies
                .iter()
                .filter(|(_, transform)| {
                    in_circle(transform.translation, whistle.aim, whistle.radius)
                })
                .map(|(entity, _)| entity)
                .collect();
            squad.recruit(&inside);
        }
    }
    if let Ok((mut transform, mut visibility)) = circle.single_mut() {
        let showing = whistle.showing();
        *visibility = if showing {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if showing {
            // Just clear of the ground, so the ring is not half-buried in the
            // slope it is drawn on.
            transform.translation = whistle.aim + Vec3::Y * 0.05;
            transform.scale = Vec3::new(whistle.radius, 1.0, whistle.radius);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drawn pose is somewhere on the stride, never past either end of it.
    ///
    /// The whole safety argument for [`Glide`] is that it only ever draws
    /// *between* two poses the simulation actually produced. Anything that ran
    /// ahead of the second one would be a Mario drawn where the fight has not
    /// put it yet, which is the extrapolation this deliberately is not.
    #[test]
    fn the_drawn_pose_stays_between_the_two_ticks_it_is_between() {
        let (was, now) = (Vec3::ZERO, Vec3::new(0.12, 0.0, 0.0));
        let glide = Glide {
            was: Some((was, Quat::IDENTITY)),
            now: Some((now, Quat::from_rotation_y(1.0))),
        };
        for step in 0..=10 {
            let alpha = step as f32 / 10.0;
            let (at, _) = glide.between(alpha).expect("a banked pose was not drawn");
            let along = (at - was).dot(now - was) / (now - was).length_squared();
            assert!(
                (-1e-5..=1.0 + 1e-5).contains(&along),
                "drawn {along} of the way along a stride at alpha {alpha}"
            );
        }
        assert_eq!(glide.between(0.0).unwrap().0, was);
        assert_eq!(glide.between(1.0).unwrap().0, now);
    }

    /// A Mario put somewhere is put there, rather than sliding across the map.
    ///
    /// A warp pipe, a respawn and a level swap all move a body outright, and
    /// interpolating one of those draws it travelling through everything in
    /// between over the next thirty-third of a second. See [`GLIDE_JUMP`].
    #[test]
    fn a_mario_that_was_put_somewhere_does_not_glide_there() {
        let there = Vec3::new(GLIDE_JUMP * 3.0, 0.0, 0.0);
        let glide = Glide {
            was: Some((Vec3::ZERO, Quat::IDENTITY)),
            now: Some((there, Quat::IDENTITY)),
        };
        assert_eq!(
            glide.between(0.5).unwrap().0,
            there,
            "a teleport was drawn as a walk"
        );
    }

    /// Nothing to interpolate is drawn as nothing, not as the origin.
    ///
    /// An ally spawned this frame has one pose and no history, and a `Glide`
    /// that answered `Vec3::ZERO` to that would put every new Mario at the
    /// middle of the map for one frame.
    #[test]
    fn a_mario_with_no_history_is_left_where_it_stands() {
        assert!(Glide::default().between(0.5).is_none());
        let banked = Glide {
            was: None,
            now: Some((Vec3::X, Quat::IDENTITY)),
        };
        assert!(banked.between(0.5).is_none());
    }

    #[test]
    fn a_cluster_spreads_without_stacking_or_lining_up() {
        let places: Vec<_> = (0..24).map(|index| slot(index, FOLLOW_SPACING)).collect();
        for (i, a) in places.iter().enumerate() {
            for (j, b) in places.iter().enumerate() {
                if i != j {
                    assert!(
                        a.distance(*b) > 0.4,
                        "slots {i} and {j} are on top of each other"
                    );
                }
            }
        }
        // The golden angle exists to stop spokes forming: no two consecutive
        // members should share a bearing.
        for pair in places.windows(2).skip(1) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                a.normalize().dot(b.normalize()) < 0.99,
                "the cluster lined up into a spoke"
            );
        }
        // And it grows outward rather than piling into one ring.
        assert!(places[23].length() > places[1].length());
    }

    #[test]
    fn the_circle_grows_from_a_tap_to_its_cap() {
        assert_eq!(circle_radius(0.0), CIRCLE_MIN_RADIUS);
        assert_eq!(circle_radius(TAP_SECONDS), CIRCLE_MIN_RADIUS);
        let half = circle_radius(TAP_SECONDS + CIRCLE_GROW_SECONDS * 0.5);
        assert!(
            half > CIRCLE_MIN_RADIUS && half < CIRCLE_MAX_RADIUS,
            "{half}"
        );
        let grown = circle_radius(TAP_SECONDS + CIRCLE_GROW_SECONDS);
        assert!((grown - CIRCLE_MAX_RADIUS).abs() < 1e-3, "{grown}");
        // Holding it forever does not grow it past the cap.
        assert!(circle_radius(60.0) <= CIRCLE_MAX_RADIUS + 1e-3);
    }

    #[test]
    fn the_circle_reaches_across_but_not_up() {
        let centre = Vec3::new(0.0, 0.0, 0.0);
        assert!(in_circle(Vec3::new(3.0, 0.0, 0.0), centre, 4.0));
        assert!(!in_circle(Vec3::new(9.0, 0.0, 0.0), centre, 4.0));
        // Somebody on the castle roof is not in a circle drawn on the lawn.
        assert!(!in_circle(
            Vec3::new(0.0, RECRUIT_HEIGHT + 1.0, 0.0),
            centre,
            4.0
        ));
        assert!(in_circle(Vec3::new(0.0, 2.0, 0.0), centre, 4.0));
    }

    #[test]
    fn recruiting_calls_back_an_ally_already_sent_somewhere() {
        let mut squad = Squad::default();
        let ally = Entity::from_raw_u32(7).unwrap();
        squad.members.push(ally);
        assert_eq!(squad.send(Vec2::new(5.0, 5.0)), 1);
        assert_eq!(squad.sent.len(), 1);
        assert!(squad.members.is_empty());
        // Whistled again, he drops the order rather than walking on deaf.
        assert_eq!(squad.recruit(&[ally]), 1);
        assert!(squad.sent.is_empty());
        assert_eq!(squad.members, vec![ally]);
        // And whistling him twice does not enlist him twice.
        assert_eq!(squad.recruit(&[ally]), 0);
        assert_eq!(squad.members.len(), 1);
    }

    #[test]
    fn sending_spreads_the_squad_around_the_spot() {
        let mut squad = Squad::default();
        for raw in 0..5 {
            squad.members.push(Entity::from_raw_u32(raw).unwrap());
        }
        let target = Vec2::new(10.0, -4.0);
        assert_eq!(squad.send(target), 5);
        assert_eq!(squad.marching(), 5);
        let mut seen: Vec<Vec2> = Vec::new();
        for (_, spot, _) in &squad.sent {
            assert!(spot.distance(target) < SEND_SPACING * 3.0);
            assert!(
                seen.iter().all(|other| other.distance(*spot) > 0.5),
                "two allies were sent to the same place"
            );
            seen.push(*spot);
        }
    }

    /// Turning the camera does not send the squad running round the player.
    ///
    /// This is the formation jitter, and the numbers are the point: the anchor
    /// sat on a 3.3 m arm off the leader's back, so a half-turn on the spot --
    /// one flick of a mouse, moving the player nowhere -- threw it 6.6 m across
    /// and every Mario chased it. Walking, on the other hand, must still drag
    /// the group along behind.
    #[test]
    fn turning_on_the_spot_does_not_move_the_formation() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut squad = Squad::default();
        let allies: Vec<Entity> = (0..4)
            .map(|index| {
                world
                    .spawn((Ally::new(Vec3::ZERO, index as f32), Transform::default()))
                    .id()
            })
            .collect();
        squad.members.extend(allies.iter().copied());
        world.insert_resource(squad);
        let leader = world.spawn((Player, Transform::default())).id();

        let goals = |world: &mut World| -> Vec<Vec2> {
            world.run_system_once(update_goals).expect("no run");
            allies
                .iter()
                .map(|ally| {
                    world
                        .get::<Ally>(*ally)
                        .expect("gone")
                        .goal
                        .expect("no goal")
                        .0
                })
                .collect()
        };
        let facing_one_way = goals(&mut world);
        // A half turn, standing still.
        world.get_mut::<Transform>(leader).unwrap().rotation =
            Quat::from_rotation_y(std::f32::consts::PI);
        let facing_the_other = goals(&mut world);
        for (before, after) in facing_one_way.iter().zip(&facing_the_other) {
            assert!(
                before.distance(*after) < 1e-3,
                "turning on the spot moved a slot {:.2} m",
                before.distance(*after)
            );
        }

        // But walking does bring them along: ten metres forward and the group
        // is gathering ten metres further on, still trailing at arm's length.
        world.get_mut::<Transform>(leader).unwrap().translation = Vec3::new(0.0, 0.0, 10.0);
        let walked = goals(&mut world);
        for (before, after) in facing_one_way.iter().zip(&walked) {
            assert!(
                (after.y - before.y - 10.0).abs() < 1e-3,
                "the formation did not follow him: {before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn disbanding_clears_both_lists() {
        let mut squad = Squad::default();
        squad.members.push(Entity::from_raw_u32(1).unwrap());
        squad
            .sent
            .push((Entity::from_raw_u32(2).unwrap(), Vec2::ZERO, true));
        assert_eq!(squad.disband(), 2);
        assert!(squad.members.is_empty() && squad.sent.is_empty());
    }

    #[test]
    fn the_aim_lands_on_the_castle_lawn_in_front_of_the_player() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        // A camera behind and above him, looking down at the ground ahead.
        let origin = player + Vec3::new(0.0, 6.0, 9.0);
        let direction = (Vec3::new(player.x, player.y, player.z - 8.0) - origin).normalize();
        let aim = aim_point(&level, origin, direction, player);
        let flat = Vec2::new(aim.x - player.x, aim.z - player.z).length();
        assert!(
            (AIM_MIN_RANGE..=AIM_MAX_RANGE).contains(&flat),
            "aimed {flat} away"
        );
        // It is ahead of him, on the bearing he is looking down.
        assert!(aim.z < player.z, "the aim landed behind the player");
        // And it is on the ground rather than floating over it.
        let floor = level.floor_height(aim + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some_and(|height| (height - aim.y).abs() < 0.5),
            "the aim is not on the floor: {aim:?} over {floor:?}"
        );
    }

    #[test]
    fn aiming_over_the_moat_walks_the_target_back_to_solid_ground() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        // Nearly level with the horizon, so the ray runs out over the moat and
        // off the edge of the map without ever meeting ground.
        let origin = player + Vec3::new(0.0, 2.0, 6.0);
        let direction = Vec3::new(0.0, 0.02, -1.0).normalize();
        let aim = aim_point(&level, origin, direction, player);
        let floor = level.floor_height(aim + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some(),
            "the order was sent somewhere with no floor under it: {aim:?}"
        );
    }

    #[test]
    fn aiming_straight_down_puts_the_target_at_his_feet() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        let aim = aim_point(&level, player + Vec3::Y * 5.0, Vec3::NEG_Y, player);
        assert_eq!(aim, player);
    }

    /// A flat lawn with a pond cut into one half of it.
    ///
    /// Ground everywhere, water only where the box says, so a walker can stand
    /// on either side and the shoreline is a straight line at `z = 0`.
    fn lawn_with_a_pond() -> LevelData {
        let corners = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        LevelData::new(
            corners.to_vec(),
            vec![[0, 1, 2], [0, 2, 3]],
            vec![crate::level::WaterBox {
                min_x: -20.,
                min_z: 0.,
                max_x: 20.,
                max_z: 40.,
                // Above the floor, so anything inside the box is out of its
                // depth rather than paddling.
                surface_y: 3.0,
            }],
        )
    }

    #[test]
    fn a_dry_heading_is_taken_as_it_is() {
        let level = lawn_with_a_pond();
        // Walking away from the pond, over open lawn: no deflection at all, and
        // that is the case that has to stay free -- it is every step the squad
        // takes that is not near a shore.
        let away = Vec2::new(0.0, -1.0);
        let kept = skirt(&level, Vec3::new(0.0, 0.0, -5.0), away, 6.0);
        assert!((kept - away).length() < 1e-6, "{kept:?}");
    }

    #[test]
    fn a_mario_walks_round_a_pond_rather_than_into_it() {
        let level = lawn_with_a_pond();
        // Standing just short of the shore, pointed straight at the water.
        let from = Vec3::new(0.0, 0.0, -2.0);
        let into = Vec2::new(0.0, 1.0);
        let bent = skirt(&level, from, into, 8.0);
        assert!(
            (bent - into).length() > 1e-3,
            "walked straight in: {bent:?}"
        );
        // And what it picked is genuinely dry, which is the whole point -- a
        // deflection that still ends in the pond is not avoidance.
        let ahead = from + Vec3::new(bent.x, 0.0, bent.y) * 8.0;
        assert!(
            level
                .water_depth(ahead)
                .is_none_or(|depth| depth <= SWIMMING_DEPTH),
            "the detour is wet too: {ahead:?}"
        );
        // Still roughly the way it wanted to go, rather than a right-about
        // turn: `skirt` tries the smallest deflections first.
        assert!(bent.dot(into) > 0.0, "it turned round: {bent:?}");
    }

    #[test]
    fn something_ringed_by_water_goes_where_it_meant_to() {
        // **The give-up case, and it is the one that keeps this safe.** A Mario
        // already in the pond has no dry heading anywhere; refusing all of them
        // would pin it there for the rest of the session. It swims out instead.
        let level = lawn_with_a_pond();
        let middle = Vec3::new(0.0, 0.0, 20.0);
        let wanted = Vec2::new(0.0, 1.0);
        assert_eq!(skirt(&level, middle, wanted, 4.0), wanted);
    }

    /// An ally that walks into deep water swims in it rather than trudging
    /// along the bottom.
    #[test]
    fn an_ally_out_of_its_depth_swims_at_the_surface() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(lawn_with_a_pond());
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        // Dropped into the middle of the pond, at the bottom of it, with its
        // plan pointing across -- so it is walking rather than standing, which
        // is the case that used to drag it along the floor.
        let under = Vec3::new(0.0, 0.0, 20.0);
        let ally = world
            .spawn((
                {
                    let mut ally = Ally::new(under, 0.0);
                    ally.plan = crate::goap::Goal::Fetch {
                        ball: Entity::from_raw_u32(99).unwrap(),
                        at: Vec2::new(0.0, 38.0),
                        arrive: 1.0,
                    };
                    ally
                },
                Transform::from_translation(under),
            ))
            .id();
        for _ in 0..90 {
            // The plan is written by hand here rather than by `goap::plan`,
            // which would clear it -- there is no ball in this world. So the
            // walk step is run on its own.
            world
                .run_system_once(move_allies)
                .expect("move_allies could not run");
        }
        let state = world.get::<Ally>(ally).unwrap();
        assert!(state.swimming, "it never noticed it was in the water");
        assert_eq!(
            state.state.motion,
            crate::player::Motion::Swim,
            "it is playing a walk cycle underwater"
        );
        let at = world.get::<Transform>(ally).unwrap().translation;
        // Riding the surface rather than the floor: the pond is three metres
        // deep and it floats just under the top of it.
        let depth = 3.0 - at.y;
        assert!(
            (depth - crate::player::SWIM_FLOAT_DEPTH).abs() < 0.4,
            "floating {depth} m down rather than {}",
            crate::player::SWIM_FLOAT_DEPTH
        );
        // And it did swim somewhere rather than treading water on the spot.
        assert!(at.z > under.z + 1.0, "it went nowhere: {at:?}");
    }

    /// One fixed tick of the squad, in the order the game runs it.
    ///
    /// `goap::plan` sits between the two, because that is where it sits in the
    /// real schedule and because a test that ran only the walk step would be
    /// exercising a Mario with no plan -- which stands still, whatever else is
    /// true about it. Deciding is its own system now; see [`crate::goap`].
    fn tick(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(update_goals)
            .expect("update_goals could not run");
        world
            .run_system_once(crate::goap::plan)
            .expect("goap::plan could not run");
        world
            .run_system_once(move_allies)
            .expect("move_allies could not run");
    }

    /// Everything a squad tick reads that is not the level or the tuning.
    fn squad_resources(world: &mut World) {
        world.insert_resource(Squad::default());
        world.insert_resource(Time::<Fixed>::from_hz(30.0));
        // The plan asks what the network is lit with, and the walk step makes a
        // splash when somebody goes in the water.
        world.init_resource::<crate::pylon::Network>();
        world.insert_resource(SoundQueue::default());
    }

    /// The follow behaviour end to end: recruit an ally, run the two fixed-step
    /// systems, and watch it walk to its slot behind the leader.
    #[test]
    fn a_recruited_ally_walks_to_its_slot_behind_the_leader() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let leader = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(leader)));
        // Standing well off to the side, with nothing to do.
        let start = leader + Vec3::new(9.0, 0.0, 0.0);
        let ally = world
            .spawn((Ally::new(start, 0.0), Transform::from_translation(start)))
            .id();

        // Unrecruited, it stays near where it was left rather than walking to
        // the leader.
        for _ in 0..30 {
            tick(&mut world);
        }
        let wandered = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            wandered.distance(start) < WANDER_RADIUS * 2.5,
            "an ally with no orders wandered off: {wandered:?}"
        );

        world.resource_mut::<Squad>().recruit(&[ally]);
        for _ in 0..90 {
            tick(&mut world);
        }
        let arrived = world.get::<Transform>(ally).unwrap().translation;
        let flat = Vec2::new(arrived.x - leader.x, arrived.z - leader.z).length();
        assert!(
            flat < FOLLOW_DISTANCE + FOLLOW_SPACING + FOLLOW_ARRIVE,
            "the ally never caught up: {flat} away"
        );
        // And it is standing on the ground rather than hovering over it.
        let level = world.resource::<LevelData>();
        let floor = level.floor_height(arrived + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some_and(|height| (height - arrived.y).abs() < 0.2),
            "the ally is off the floor at {arrived:?}"
        );
    }

    /// A Mario with nothing to do walks somewhere, stands about, and walks
    /// somewhere else.
    ///
    /// What it must not do is change its mind every few ticks. Every change of
    /// clip restarts it, so an ally alternating between walking and standing
    /// three times a second never plays more than the opening frames of a
    /// step: a field of Marios stuck mid-stride, which is exactly what the
    /// orbiting drift target this replaced produced. The count is what the
    /// test is really about -- eyes on the game see the symptom, and this is
    /// the number underneath it.
    #[test]
    fn an_idle_ally_ambles_rather_than_twitching() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let start = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(start)));
        let ally = world
            .spawn((Ally::new(start, 0.0), Transform::from_translation(start)))
            .id();

        // Ten seconds of having nowhere to be.
        let mut clips = Vec::new();
        let mut walked = 0;
        for _ in 0..300 {
            tick(&mut world);
            let ally = world.get::<Ally>(ally).unwrap();
            if ally.state.motion == crate::player::Motion::Run {
                walked += 1;
            }
            let (clip, _) = crate::animation::resolve(ActiveCharacter::Mario, &ally.state);
            if clips.last() != Some(&clip) {
                clips.push(clip);
            }
        }
        assert!(
            clips.len() <= 8,
            "the clip changed {} times in ten seconds, so the walk never plays: {clips:?}",
            clips.len()
        );
        // And it is an amble rather than a statue: it does spend time walking,
        // and time standing, and neither swallows the other.
        assert!(
            (30..270).contains(&walked),
            "walked on {walked} of 300 ticks, which is not an amble"
        );
    }

    /// Sent somewhere, an ally holds the spot rather than drifting off it.
    #[test]
    fn a_sent_ally_arrives_and_holds_its_ground() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let leader = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(leader)));
        let ally = world
            .spawn((Ally::new(leader, 0.0), Transform::from_translation(leader)))
            .id();
        world.resource_mut::<Squad>().recruit(&[ally]);
        // Somewhere across the lawn, still on the castle grounds.
        let target = Vec2::new(leader.x + 8.0, leader.z - 6.0);
        assert_eq!(world.resource_mut::<Squad>().send(target), 1);

        for _ in 0..150 {
            tick(&mut world);
        }
        let squad = world.resource::<Squad>();
        assert_eq!(squad.marching(), 0, "never reported arriving");
        let here = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(here.x, here.z).distance(target) <= SEND_ARRIVE + 0.2,
            "stopped {:?} short of the spot",
            Vec2::new(here.x, here.z).distance(target)
        );
        // Held: an order is not a suggestion, so it does not wander home.
        for _ in 0..90 {
            tick(&mut world);
        }
        let later = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(later.x, later.z).distance(target) <= SEND_ARRIVE + 0.2,
            "the ally wandered off the spot it was sent to"
        );
    }
}
