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
const SEND_ARRIVE: f32 = 1.4;

/// The angle between one slot in a cluster and the next. The golden angle is
/// what keeps a spiral from lining its points up into spokes, which is the
/// same reason a sunflower uses it: any simpler step leaves the allies in rows
/// with gaps between them.
pub(crate) const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// How near an ally has to be before it counts as standing on its slot.
const ALLY_RADIUS: f32 = 0.4;

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
}

impl Ally {
    /// A new Mario standing where it was put, about to amble.
    pub fn new(home: Vec3, phase: f32) -> Self {
        let mut ally = Self {
            goal: None,
            home,
            stroll: Vec2::new(home.x, home.z),
            rest_left: 0.0,
            phase,
            velocity: Vec3::ZERO,
            state: AnimationState::default(),
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

/// Puts one Mario in the field.
///
/// Shared by the console's population count and by the Mario warp pipe, so the
/// two cannot produce subtly different Marios -- the same reason `enemy::spawn`
/// is shared between the level's placements and the enemy pipes.
pub fn spawn_ally(commands: &mut Commands, assets: &AssetServer, home: Vec3, phase: f32) -> Entity {
    commands
        .spawn((
            Ally::new(home, phase),
            // Allies animate off the same tables the playable Mario does.
            ActiveCharacter::Mario,
            // And stand on the ground the same way, so they get the same disc
            // under them as the player.
            crate::shadow::ShadowCaster::new(crate::player::PLAYER_RADIUS),
            WorldAssetRoot(assets.load("mario/mario.glb#Scene0")),
            Transform::from_translation(home).with_scale(Vec3::splat(0.00667)),
        ))
        .id()
}

/// The Marios the console's population count answers for: the field's standing
/// crowd, with the warp pipe's own brood left out of it.
type StandingCrowd<'w, 's> =
    Query<'w, 's, (Entity, &'static Transform), (With<Ally>, Without<crate::pipe::Brood>)>;

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
    let wanted = tuning.ally_count.round() as usize;
    let live: Vec<_> = allies.iter().collect();
    if live.len() == wanted {
        return;
    }
    if live.len() > wanted {
        for (entity, _) in live.iter().skip(wanted) {
            squad.members.retain(|member| member != entity);
            squad.sent.retain(|(sent, _, _)| sent != entity);
            commands.entity(*entity).despawn();
        }
        return;
    }
    let Ok(leader) = player.single() else {
        return;
    };
    // New arrivals stand around the leader in the same cluster the squad uses
    // to follow him, so a crowd summoned from the console is not a pile.
    for index in live.len()..wanted {
        let offset = slot(index, FOLLOW_SPACING * 1.5);
        let x = leader.translation.x + offset.x;
        let z = leader.translation.z + offset.y;
        let y = level
            .floor_height(Vec3::new(x, leader.translation.y + PLAYER_HEIGHT, z))
            .unwrap_or(leader.translation.y);
        let home = Vec3::new(x, y, z);
        spawn_ally(&mut commands, &assets, home, index as f32 * GOLDEN_ANGLE);
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
    // through him, and so the slots stay put while he turns.
    let behind = leader.rotation * Vec3::Z;
    let anchor = Vec2::new(
        leader.translation.x - behind.x * FOLLOW_DISTANCE,
        leader.translation.z - behind.z * FOLLOW_DISTANCE,
    );
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

/// Walks each ally toward its goal, or lets it amble where it stands.
pub fn move_allies(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    squad: Res<Squad>,
    // One still in the air out of the Mario pipe is flown by `pipe::fly`, and
    // walking it toward a goal at the same time would drag it out of its arc.
    mut allies: Query<(Entity, &mut Ally, &mut Transform), Without<crate::pipe::Launched>>,
) {
    let dt = crate::player::FIXED_DT;
    for (entity, mut ally, mut transform) in &mut allies {
        let ordered = squad.members.contains(&entity)
            || squad.sent.iter().any(|(sent, _, _)| *sent == entity);
        // A goal outlives the order it came from by exactly one tick, which is
        // how being released is noticed: it stands where the order left it and
        // ambles from there, rather than walking back to wherever it was
        // recruited.
        if !ordered && ally.goal.take().is_some() {
            ally.home = transform.translation;
            ally.amble_somewhere_else();
        }
        // Standing about between ambles. An ally that has nowhere to be is
        // still for seconds at a time, which is what lets the idle actually
        // play.
        if ally.rest_left > 0.0 && ally.goal.is_none() {
            ally.rest_left -= dt;
            ally.stand(dt);
            continue;
        }
        let here = Vec2::new(transform.translation.x, transform.translation.z);
        let (target, arrive) = match ally.goal {
            Some((target, arrive)) => (target, arrive),
            // Nowhere to be: amble to the spot picked last time it arrived, so
            // an idle field of Marios is not a field of statues.
            None => (ally.stroll, WANDER_ARRIVE),
        };
        let to_target = target - here;
        let distance = to_target.length();
        if distance <= arrive {
            if ally.goal.is_none() {
                // Arrived nowhere in particular: stand about a while, then
                // amble somewhere else.
                ally.amble_somewhere_else();
            }
            ally.stand(dt);
            continue;
        }
        // Ease off over the last stride so arriving is not a hard stop.
        let pace = if ally.goal.is_some() {
            tuning.ally_speed
        } else {
            AMBLE_SPEED
        };
        let speed = pace * (distance / arrive.max(0.001)).min(1.0);
        let step = to_target / distance * speed * dt;
        transform.translation.x += step.x;
        transform.translation.z += step.y;
        if let Some(height) = level.floor_height(transform.translation + Vec3::Y * PLAYER_HEIGHT) {
            transform.translation.y = height;
        }
        transform.rotation = Quat::from_rotation_y(step.x.atan2(step.y));
        ally.velocity = Vec3::new(step.x / dt, 0.0, step.y / dt);
        ally.state.motion = crate::player::Motion::Run;
        ally.state.speed = speed;
        ally.state.still_for = 0.0;
    }
}

/// Plays each ally's own clip, off the same tables the player uses.
pub fn animate_allies(
    animations: Res<CharacterAnimations>,
    allies: Query<&Ally>,
    mut players: Query<(&AllyAnimationRoot, &mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    if !animations.ready(ActiveCharacter::Mario) {
        return;
    }
    for (root, mut player, mut transitions) in &mut players {
        let Ok(ally) = allies.get(root.0) else {
            continue;
        };
        let (name, rate) = crate::animation::resolve(ActiveCharacter::Mario, &ally.state);
        let Some(clip) = animations.named(ActiveCharacter::Mario, name) else {
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
fn ring_mesh() -> Mesh {
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

    #[test]
    fn disbanding_clears_both_lists() {
        let mut squad = Squad::default();
        squad.members.push(Entity::from_raw_u32(1).unwrap());
        squad.sent.push((Entity::from_raw_u32(2).unwrap(), Vec2::ZERO, true));
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

    /// The follow behaviour end to end: recruit an ally, run the two fixed-step
    /// systems, and watch it walk to its slot behind the leader.
    #[test]
    fn a_recruited_ally_walks_to_its_slot_behind_the_leader() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        world.insert_resource(Squad::default());
        world.insert_resource(Time::<Fixed>::from_hz(30.0));
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
            world.run_system_once(update_goals).expect("update_goals could not run");
            world.run_system_once(move_allies).expect("move_allies could not run");
        }
        let wandered = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            wandered.distance(start) < WANDER_RADIUS * 2.5,
            "an ally with no orders wandered off: {wandered:?}"
        );

        world.resource_mut::<Squad>().recruit(&[ally]);
        for _ in 0..90 {
            world.run_system_once(update_goals).expect("update_goals could not run");
            world.run_system_once(move_allies).expect("move_allies could not run");
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
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        world.insert_resource(Squad::default());
        let start = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(start)));
        let ally = world
            .spawn((Ally::new(start, 0.0), Transform::from_translation(start)))
            .id();

        // Ten seconds of having nowhere to be.
        let mut clips = Vec::new();
        let mut walked = 0;
        for _ in 0..300 {
            world.run_system_once(move_allies).expect("move_allies could not run");
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
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        world.insert_resource(Squad::default());
        world.insert_resource(Time::<Fixed>::from_hz(30.0));
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
            world.run_system_once(update_goals).expect("update_goals could not run");
            world.run_system_once(move_allies).expect("move_allies could not run");
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
            world.run_system_once(update_goals).expect("update_goals could not run");
            world.run_system_once(move_allies).expect("move_allies could not run");
        }
        let later = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(later.x, later.z).distance(target) <= SEND_ARRIVE + 0.2,
            "the ally wandered off the spot it was sent to"
        );
    }
}
