use crate::{
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    input::InputState,
    level::LevelData,
    ActiveCharacter, GameState,
};
use bevy::prelude::*;

pub const FIXED_DT: f32 = 1.0 / 30.0;

/// The player's collision capsule: radius, and height from feet to crown.
pub const PLAYER_RADIUS: f32 = 0.42;
pub const PLAYER_HEIGHT: f32 = 1.75;

/// How far above the feet a ceiling search starts. Anything nearer is the
/// floor being stood on rather than something to bump a head on.
const CEILING_CLEARANCE: f32 = 0.6;

/// Ground distance between footfalls, and seconds between swim strokes.
const STEP_DISTANCE: f32 = 3.0;
const STROKE_SECONDS: f32 = 0.6;

/// How far under the surface counts as being in the water at all. Above this
/// the feet are wet and nothing else changes.
const SUBMERGED_DEPTH: f32 = 0.15;

/// How far under the surface each character floats once they are in deep
/// water: Mario swimming, the Hero held up by it.
const SWIM_FLOAT_DEPTH: f32 = 0.45;
const WADE_FLOAT_DEPTH: f32 = 0.6;

/// How fast the surface pulls the wading Hero back to that depth, and how
/// fast whatever vertical speed he ran off the bank with bleeds away. The
/// original approaches 8 and 4 SM64 units per frame at 30 Hz, which at this
/// port's scale of 1/100 is 2.4 and 1.2 units a second.
const WADE_RISE: f32 = 2.4;
const WADE_SETTLE: f32 = 1.2;

/// Downward speed a landing needs before it is loud enough to be heard. Below
/// this the player is settling onto a slope, not landing on it.
const LANDING_IMPACT: f32 = 2.0;

/// How long the landing pose is held, and how long after a swing the next one
/// still counts as the second half of a combo.
const LAND_SECONDS: f32 = 0.25;
const COMBO_WINDOW: f32 = 1.2;

#[derive(Component)]
pub struct Player;

#[derive(Component, Clone, Copy)]
pub struct PreviousPose {
    translation: Vec3,
    rotation: Quat,
}

impl PreviousPose {
    pub fn new(transform: &Transform) -> Self {
        Self {
            translation: transform.translation,
            rotation: transform.rotation,
        }
    }
}

#[derive(Resource, Clone, Copy)]
pub struct RenderPose {
    pub translation: Vec3,
    pub rotation: Quat,
}

#[derive(Component)]
pub struct PlayerVisual;

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    Idle,
    Run,
    Jump,
    Fall,
    /// The moment of touching down, held briefly so the landing clip is seen
    /// rather than skipped straight past into the run.
    Land,
    Skate,
    Fly,
    Swim,
    Attack,
}

/// What the water is doing to whoever is standing in it.
///
/// Deep water is not one behaviour, because the two characters do not have the
/// same clips. Ported from `sm64py/hero/constants.py`, which says it plainly:
/// the Hero has no swimming animation, so rather than draw a walk cycle
/// underwater he is held at the surface and slowed to a wade. It is the one
/// place his move set is short of Mario's, and it is a gap in the source
/// animation rather than something the controller could paper over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submersion {
    /// Dry, or wet only to the ankles.
    Dry,
    /// Deep water, walked through upright and slowly.
    Wading,
    /// Deep water, swum properly. Mario only.
    Swimming,
}

impl Submersion {
    /// In deep water, however it is being handled.
    pub fn in_water(self) -> bool {
        self != Self::Dry
    }

    pub fn swimming(self) -> bool {
        self == Self::Swimming
    }
}

/// Which of the three a character is in.
///
/// Split out from the controller so the rule is checked without a level, a
/// water box or a renderer -- it is the part that decides whether the Hero
/// swims, which he must never do.
pub fn submersion(character: ActiveCharacter, y: f32, water_level: Option<f32>) -> Submersion {
    if !water_level.is_some_and(|surface| y < surface - SUBMERGED_DEPTH) {
        return Submersion::Dry;
    }
    match character {
        ActiveCharacter::Hero => Submersion::Wading,
        ActiveCharacter::Mario => Submersion::Swimming,
    }
}

/// Moves `value` toward `target` by at most `step`, the way SM64's
/// `approach_f32` does: a linear pull rather than a spring, so it settles
/// without overshooting and ringing.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

#[derive(Component)]
pub struct Controller {
    pub velocity: Vec3,
    pub grounded: bool,
    pub motion: Motion,
    pub attack_left: f32,
    pub invulnerable_left: f32,
    pub health: u8,
    pub submersion: Submersion,
    /// Distance run since the last footfall, and seconds since the last swim
    /// stroke. Both only drive audio cadence.
    step_phase: f32,
    stroke_phase: f32,
    /// Seconds the landing clip still owns the body.
    pub land_left: f32,
    /// Which swing of the combo the next attack is, and how long since the
    /// last one, so a held button reads as a combo and a pause resets it.
    pub combo: u8,
    since_attack: f32,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: false,
            motion: Motion::Fall,
            attack_left: 0.0,
            invulnerable_left: 0.0,
            health: 3,
            submersion: Submersion::Dry,
            step_phase: 0.0,
            stroke_phase: 0.0,
            land_left: 0.0,
            combo: 0,
            since_attack: COMBO_WINDOW,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn movement(
    mut input_state: ResMut<InputState>,
    level: Res<LevelData>,
    state: Res<GameState>,
    tuning: Res<GameTuning>,
    mut sounds: ResMut<SoundQueue>,
    mut player: Query<(&mut Transform, &mut PreviousPose, &mut Controller), With<Player>>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
) {
    let Ok((mut transform, mut previous, mut ctrl)) = player.single_mut() else {
        return;
    };
    previous.translation = transform.translation;
    previous.rotation = transform.rotation;
    let Ok(cam) = camera.single() else {
        return;
    };
    // Latched edges are consumed here rather than polled, so each press acts
    // on exactly one fixed step however many steps this frame runs.
    let jump_pressed = InputState::take(&mut input_state.jump);
    let attack_pressed = InputState::take(&mut input_state.attack);
    let input = input_state.move_axis;
    // `forward`/`right` hand back a `Direction3d` rather than a `Vec3` now: a
    // vector the type system knows is unit length. Flattening one onto the
    // ground plane is exactly the operation that stops it being unit length,
    // so it is taken back to a plain `Vec3` first and renormalised after.
    let forward = (Vec3::from(cam.forward()) * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let right = (Vec3::from(cam.right()) * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let wish = forward * input.y + right * input.x;
    let water_level = level.water_level(transform.translation.x, transform.translation.z);
    let was_wet = ctrl.submersion.in_water();
    ctrl.submersion = submersion(state.active, transform.translation.y, water_level);
    if ctrl.submersion.in_water() != was_wet {
        sounds.push(Sfx::Splash);
    }
    let boost = input_state.boost && state.active == ActiveCharacter::Hero;
    let skate = boost && ctrl.grounded && !ctrl.submersion.in_water();
    let speed = match ctrl.submersion {
        Submersion::Swimming => tuning.mario_swim,
        Submersion::Wading => tuning.hero_wade,
        Submersion::Dry if skate => tuning.skate_speed,
        Submersion::Dry if state.active == ActiveCharacter::Hero => tuning.hero_speed,
        Submersion::Dry => tuning.mario_speed,
    };
    let accel = if skate {
        tuning.skate_accel
    } else {
        tuning.walk_accel
    };
    let target = wish * speed;
    let horizontal = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z);
    let next = if wish.length_squared() > 0.0 {
        horizontal.lerp(target, (accel * FIXED_DT).min(1.0))
    } else {
        horizontal.lerp(
            Vec3::ZERO,
            (if skate { 0.35 } else { tuning.decel } * FIXED_DT).min(1.0),
        )
    };
    ctrl.velocity.x = next.x;
    ctrl.velocity.z = next.z;

    match ctrl.submersion {
        Submersion::Swimming => {
            let surface = water_level.unwrap() - SWIM_FLOAT_DEPTH;
            let buoyancy = ((surface - transform.translation.y) * 2.0).clamp(-2.0, 3.0);
            ctrl.velocity.y += (buoyancy - ctrl.velocity.y) * 0.16;
            if jump_pressed {
                ctrl.velocity.y = (ctrl.velocity.y + 3.8).min(6.0);
                sounds.push(Sfx::Stroke);
            }
            ctrl.grounded = false;
        }
        // No stroke and no jump: a wade has neither. Whatever vertical speed
        // he entered the water with bleeds off, and the surface takes over
        // from gravity below.
        Submersion::Wading => {
            ctrl.velocity.y = approach(ctrl.velocity.y, 0.0, WADE_SETTLE * FIXED_DT);
        }
        Submersion::Dry => {
            if jump_pressed && ctrl.grounded {
                ctrl.velocity.y = if skate { 9.0 } else { tuning.jump_velocity };
                ctrl.grounded = false;
                sounds.push(Sfx::Jump);
            }
        }
    }
    // Water supplies its own vertical motion either way -- buoyancy for a
    // swimmer, the pull toward the surface below for a wader -- so gravity and
    // the Hero's booster do not operate until the capsule leaves the box.
    if !ctrl.submersion.in_water() {
        if boost && !ctrl.grounded {
            ctrl.velocity.y = (ctrl.velocity.y + tuning.jet_thrust).min(tuning.jet_rise);
        } else if !ctrl.grounded {
            ctrl.velocity.y -= 1.2;
        }
    }

    ctrl.since_attack += FIXED_DT;
    if attack_pressed {
        // A swing soon after the last one is the second half of a combo; a
        // pause starts over at the first.
        ctrl.combo = u8::from(ctrl.since_attack < COMBO_WINDOW && ctrl.combo == 0);
        ctrl.since_attack = 0.0;
        ctrl.attack_left = 0.55;
        ctrl.motion = Motion::Attack;
        sounds.push(Sfx::Attack);
    }
    ctrl.attack_left = (ctrl.attack_left - FIXED_DT).max(0.0);
    transform.translation.y += ctrl.velocity.y * FIXED_DT;
    if ctrl.submersion == Submersion::Wading {
        // Held at the surface rather than sinking. Approached instead of
        // snapped, so running off a bank into deep water does not pop him up
        // to it; and only a pull, so a bottom shallower than the float depth
        // still wins below and he walks along it.
        let float_line = water_level.unwrap() - WADE_FLOAT_DEPTH;
        transform.translation.y =
            approach(transform.translation.y, float_line, WADE_RISE * FIXED_DT);
    }
    let horizontal_step = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z) * FIXED_DT;
    let wanted = transform.translation + horizontal_step;
    let corrected = level.resolve_walls(wanted, PLAYER_RADIUS, PLAYER_HEIGHT);
    // Cancel velocity into a wall while retaining tangential motion. This
    // produces natural sliding and avoids accumulating speed against walls.
    let correction = corrected - wanted;
    if correction.length_squared() > 1e-8 {
        let normal = Vec3::new(correction.x, 0.0, correction.z).normalize_or_zero();
        let into_wall = ctrl.velocity.dot(normal);
        if into_wall < 0.0 {
            ctrl.velocity -= normal * into_wall;
        }
    }
    transform.translation = corrected;
    // A head bump stops the rise but not the run: the horizontal step above
    // has already been resolved against the walls, and a low arch should slow
    // a jump under it rather than stopping the player dead.
    if !ctrl.submersion.swimming() && ctrl.velocity.y > 0.0 {
        if let Some(ceiling) = level.ceiling_height(transform.translation, CEILING_CLEARANCE) {
            if transform.translation.y + PLAYER_HEIGHT > ceiling {
                transform.translation.y = ceiling - PLAYER_HEIGHT;
                ctrl.velocity.y = 0.0;
            }
        }
    }
    let was_grounded = ctrl.grounded;
    let impact = ctrl.velocity.y;
    let floor = level.floor_height(transform.translation + Vec3::Y * 0.75);
    // A wader keeps the ordinary floor logic: in water shallower than the
    // float depth the bottom is under his feet and he walks along it, which is
    // the whole difference between wading and swimming.
    if ctrl.submersion.swimming() {
        ctrl.grounded = false;
    } else if let Some(height) = floor {
        let separation = transform.translation.y - height;
        let walking_on_floor = ctrl.grounded && separation <= 0.75;
        let landed = transform.translation.y <= height && ctrl.velocity.y <= 0.0;
        if walking_on_floor || landed {
            transform.translation.y = height;
            ctrl.velocity.y = 0.0;
            ctrl.grounded = true;
            if !was_grounded && impact < -LANDING_IMPACT {
                sounds.push(Sfx::Land);
                ctrl.land_left = LAND_SECONDS;
            }
        } else if ctrl.grounded {
            ctrl.grounded = false;
        }
    } else {
        // Leaving a ledge must enter the air state. Keeping the previous
        // grounded flag here was the source of the visibly floating player.
        ctrl.grounded = false;
    }
    if transform.translation.y < -20.0 {
        transform.translation = Vec3::new(-13.28, 3.0, 46.64);
        ctrl.velocity = Vec3::ZERO;
        previous.translation = transform.translation;
        previous.rotation = transform.rotation;
    }
    if ctrl.health == 0 {
        transform.translation = Vec3::new(-13.28, 3.0, 46.64);
        ctrl.velocity = Vec3::ZERO;
        ctrl.health = 3;
        ctrl.invulnerable_left = 1.5;
        previous.translation = transform.translation;
        previous.rotation = transform.rotation;
    }
    if wish.length_squared() > 0.01 {
        let yaw = wish.x.atan2(wish.z);
        transform.rotation = transform.rotation.slerp(Quat::from_rotation_y(yaw), 0.28);
    }
    // Footfalls are paced by ground covered rather than by time, so they stay
    // in step with the run whatever the speed is tuned to; strokes are paced
    // by time, because swimming has no stride to measure.
    let ground_speed = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z).length();
    if ctrl.submersion.swimming() {
        ctrl.step_phase = 0.0;
        ctrl.stroke_phase += FIXED_DT;
        if ctrl.stroke_phase >= STROKE_SECONDS && ground_speed > 0.25 {
            ctrl.stroke_phase = 0.0;
            sounds.push(Sfx::Stroke);
        }
    } else {
        ctrl.stroke_phase = 0.0;
        // Skates roll rather than step, so only a run is footed.
        if ctrl.grounded && !skate && ground_speed > 0.25 {
            ctrl.step_phase += ground_speed * FIXED_DT;
            if ctrl.step_phase >= STEP_DISTANCE {
                ctrl.step_phase = 0.0;
                sounds.push(Sfx::Step);
            }
        } else {
            ctrl.step_phase = 0.0;
        }
    }

    ctrl.land_left = (ctrl.land_left - FIXED_DT).max(0.0);
    if !ctrl.grounded {
        ctrl.land_left = 0.0;
    }
    if ctrl.attack_left <= 0.0 {
        // A wade is drawn as a walk, standing up, whether or not the bottom is
        // under his feet -- `sync_graphics` in `sm64py/hero/state.py` makes
        // the same point, and it is why this is tested before `grounded`
        // rather than after it.
        ctrl.motion = if ctrl.submersion == Submersion::Wading {
            if horizontal.length() > 0.25 {
                Motion::Run
            } else {
                Motion::Idle
            }
        } else if ctrl.grounded {
            if skate {
                Motion::Skate
            } else if ctrl.land_left > 0.0 {
                Motion::Land
            } else if horizontal.length() > 0.25 {
                Motion::Run
            } else {
                Motion::Idle
            }
        } else if ctrl.submersion.swimming() {
            Motion::Swim
        } else if boost {
            Motion::Fly
        } else if ctrl.velocity.y > 0.0 {
            Motion::Jump
        } else {
            Motion::Fall
        };
    }
}

#[allow(clippy::type_complexity)]
pub fn sync_visual(
    fixed_time: Res<Time<Fixed>>,
    player: Query<(&Transform, &PreviousPose), (With<Player>, Without<PlayerVisual>)>,
    mut render_pose: ResMut<RenderPose>,
    mut visuals: Query<(&ActiveCharacter, &mut Transform), With<PlayerVisual>>,
) {
    let Ok((root, previous)) = player.single() else {
        return;
    };
    let alpha = fixed_time.overstep_fraction().clamp(0.0, 1.0);
    render_pose.translation = previous.translation.lerp(root.translation, alpha);
    render_pose.rotation = previous.rotation.slerp(root.rotation, alpha);
    for (kind, mut visual) in &mut visuals {
        visual.translation = render_pose.translation;
        visual.rotation = render_pose.rotation;
        visual.scale = match kind {
            ActiveCharacter::Hero => Vec3::splat(0.81),
            ActiveCharacter::Mario => Vec3::splat(0.00667),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audio::Sfx, level};
    use bevy::{camera::Camera3d, ecs::system::RunSystemOnce};

    /// The castle's Hero spawn, on the lawn in front of the gate.
    const SPAWN: Vec3 = Vec3::new(-13.28, 3.0, 46.64);

    /// A world holding just what `movement` reads: the real castle collision,
    /// a player, and a camera to take the movement basis from. No renderer and
    /// no window, so this runs in CI and under WSL without opening anything.
    fn world() -> World {
        let (collision, _) = level::load();
        world_with(collision, SPAWN)
    }

    fn world_with(collision: level::LevelData, spawn_at: Vec3) -> World {
        let mut world = World::new();
        world.insert_resource(collision);
        world.insert_resource(GameState::default());
        world.insert_resource(GameTuning::default());
        world.insert_resource(InputState::default());
        world.insert_resource(SoundQueue::default());
        let spawn = Transform::from_translation(spawn_at);
        world.spawn((
            Player,
            PreviousPose::new(&spawn),
            Controller::default(),
            spawn,
        ));
        // Facing -Z, so "forward" input runs toward the castle.
        world.spawn((Camera3d::default(), Transform::from_xyz(-13.0, 6.0, 56.0)));
        world
    }

    fn tick(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world.run_system_once(movement).expect("movement could not run");
        }
    }

    /// The player's position, and the parts of its controller the tests read.
    fn player(world: &mut World) -> (Vec3, Vec3, bool, Motion) {
        let mut query = world.query_filtered::<(&Transform, &Controller), With<Player>>();
        let (transform, controller) = query.single(world).unwrap();
        (
            transform.translation,
            controller.velocity,
            controller.grounded,
            controller.motion,
        )
    }

    fn sounds(world: &mut World) -> Vec<Sfx> {
        world.resource_mut::<SoundQueue>().drain()
    }

    #[test]
    fn the_player_settles_onto_the_lawn_and_stays_there() {
        let mut world = world();
        tick(&mut world, 30);
        let (position, _, grounded, _) = player(&mut world);
        assert!(grounded, "never found the floor");
        assert!(
            (position.y - SPAWN.y).abs() < 1.0,
            "settled at {} rather than near the spawn height",
            position.y
        );
        assert!(
            !sounds(&mut world).contains(&Sfx::Step),
            "stood still, stepped anyway"
        );
    }

    #[test]
    fn a_latched_jump_fires_once_and_lands_with_a_thud() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().jump = true;
        tick(&mut world, 1);
        let (_, velocity, grounded, _) = player(&mut world);
        assert!(velocity.y > 0.0, "the jump did not take off");
        assert!(!grounded);
        assert_eq!(sounds(&mut world), vec![Sfx::Jump]);
        // The edge was consumed, so the held-down key cannot jump again from
        // mid-air on the next tick.
        assert!(!world.resource::<InputState>().jump);

        // Falling back down raises exactly one landing.
        tick(&mut world, 60);
        let (_, _, grounded, _) = player(&mut world);
        assert!(grounded, "never came back down");
        let heard = sounds(&mut world);
        assert_eq!(
            heard.iter().filter(|sfx| **sfx == Sfx::Land).count(),
            1,
            "{heard:?}"
        );
    }

    #[test]
    fn running_covers_ground_and_paces_footfalls_by_distance() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        tick(&mut world, 60);
        let (position, _, _, motion) = player(&mut world);
        let travelled = (position - SPAWN).length();
        assert!(travelled > 5.0, "only travelled {travelled}");
        assert_eq!(motion, Motion::Run);

        let steps = sounds(&mut world)
            .iter()
            .filter(|sfx| **sfx == Sfx::Step)
            .count();
        // Two seconds at the Hero's speed covers roughly 20 units, which is
        // several strides and nowhere near one per tick.
        assert!(
            (2..=12).contains(&steps),
            "{steps} footfalls in two seconds"
        );
    }

    #[test]
    fn an_attack_is_one_swing_per_press() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().attack = true;
        tick(&mut world, 10);
        let heard = sounds(&mut world);
        assert_eq!(
            heard.iter().filter(|sfx| **sfx == Sfx::Attack).count(),
            1,
            "{heard:?}"
        );
    }

    /// A floor and a roof over it, each a quad of two triangles, centred on
    /// the origin and large enough that the player cannot run out from under
    /// the roof during the test.
    fn room(roof: f32) -> level::LevelData {
        let quad = |y: f32| {
            [
                Vec3::new(-20., y, -20.),
                Vec3::new(20., y, -20.),
                Vec3::new(20., y, 20.),
                Vec3::new(-20., y, 20.),
            ]
        };
        let mut vertices = quad(0.0).to_vec();
        vertices.extend(quad(roof));
        let faces = vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
        level::LevelData::new(vertices, faces, Vec::new())
    }

    /// A flat bottom with a body of water over it, both wider than a test can
    /// walk out of. The surface is at zero and the bottom `depth` below it.
    fn pool(depth: f32) -> level::LevelData {
        let corners = [
            Vec3::new(-40., -depth, -40.),
            Vec3::new(40., -depth, -40.),
            Vec3::new(40., -depth, 40.),
            Vec3::new(-40., -depth, 40.),
        ];
        level::LevelData::new(
            corners.to_vec(),
            vec![[0, 1, 2], [0, 2, 3]],
            vec![level::WaterBox {
                min_x: -40.,
                min_z: -40.,
                max_x: 40.,
                max_z: 40.,
                surface_y: 0.0,
            }],
        )
    }

    fn submersion_of(world: &mut World) -> Submersion {
        let mut query = world.query_filtered::<&Controller, With<Player>>();
        query.single(world).unwrap().submersion
    }

    /// Distance covered over a second of running forward.
    fn ground_covered(world: &mut World) -> f32 {
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        let (from, ..) = player(world);
        tick(world, 30);
        let (to, ..) = player(world);
        Vec2::new(to.x - from.x, to.z - from.z).length()
    }

    /// The Hero has no swimming clips, so he does not swim: deep water holds
    /// him at the surface and slows him to a wade, drawn upright. This is
    /// `act_wading` in `sm64py/hero/actions.py`, and it is the one place his
    /// move set is deliberately short of Mario's.
    #[test]
    fn the_hero_wades_rather_than_swimming() {
        let mut world = world_with(pool(6.0), Vec3::ZERO);
        let mut deepest = 0.0f32;
        for _ in 0..90 {
            tick(&mut world, 1);
            let (position, _, _, motion) = player(&mut world);
            assert_ne!(motion, Motion::Swim, "the Hero is swimming");
            deepest = deepest.min(position.y);
        }
        assert_eq!(submersion_of(&mut world), Submersion::Wading);
        let (position, _, _, _) = player(&mut world);
        assert!(
            (position.y + WADE_FLOAT_DEPTH).abs() < 0.2,
            "settled at {} rather than at the surface",
            position.y
        );
        assert!(
            deepest > -2.0,
            "he sank to {deepest} on the way in rather than being held up"
        );
        // And he walks through it, which is the whole point of the clip he
        // does have.
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        tick(&mut world, 10);
        assert_eq!(player(&mut world).3, Motion::Run);
    }

    /// The same water, the other character. Without this the test above would
    /// pass just as well if nothing in the game could swim at all.
    #[test]
    fn mario_still_swims_in_the_same_water() {
        let mut world = world_with(pool(6.0), Vec3::ZERO);
        world.resource_mut::<GameState>().active = ActiveCharacter::Mario;
        tick(&mut world, 90);
        assert_eq!(submersion_of(&mut world), Submersion::Swimming);
        assert_eq!(player(&mut world).3, Motion::Swim);
    }

    /// Water shallower than he floats in is simply slow ground: the bottom is
    /// under his feet and he walks along it.
    #[test]
    fn the_hero_walks_the_bottom_of_shallow_water() {
        let bottom = WADE_FLOAT_DEPTH - 0.2;
        let mut world = world_with(pool(bottom), Vec3::ZERO);
        tick(&mut world, 60);
        let (position, _, grounded, _) = player(&mut world);
        assert_eq!(submersion_of(&mut world), Submersion::Wading);
        assert!(grounded, "floating over a bottom he could stand on");
        assert!(
            (position.y + bottom).abs() < 0.05,
            "standing at {} rather than on the bottom at {}",
            position.y,
            -bottom
        );
    }

    /// "Slower under water" is the requirement, so it is measured rather than
    /// assumed: the same second of running, in water and out of it.
    #[test]
    fn wading_is_slower_than_walking() {
        let mut dry = world_with(room(50.0), Vec3::ZERO);
        tick(&mut dry, 30);
        let on_land = ground_covered(&mut dry);

        let mut wet = world_with(pool(6.0), Vec3::ZERO);
        tick(&mut wet, 60);
        let in_water = ground_covered(&mut wet);

        assert!(
            in_water < on_land * 0.75,
            "wading covered {in_water} to walking's {on_land}, which is not \
             slower in any way a player would notice"
        );
        assert!(in_water > 1.0, "wading is not movement at all: {in_water}");
    }

    #[test]
    fn a_ceiling_stops_a_jump_without_stopping_the_run() {
        // Head room of a third of a metre: enough to stand in, not enough to
        // jump in.
        let roof = PLAYER_HEIGHT + 0.35;
        let mut world = world_with(room(roof), Vec3::new(0., 0., 0.));
        tick(&mut world, 10);

        world.resource_mut::<InputState>().jump = true;
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        let mut highest: f32 = 0.0;
        for _ in 0..20 {
            tick(&mut world, 1);
            let (position, _, _, _) = player(&mut world);
            highest = highest.max(position.y);
            assert!(
                position.y + PLAYER_HEIGHT <= roof + 1e-3,
                "the head passed through the roof at {}",
                position.y
            );
        }
        // It did leave the ground -- the assertion above must be catching a
        // real jump rather than a player that never rose at all.
        assert!(highest > 0.05, "the jump never happened: {highest}");
        // And the bump stopped the rise without stopping the run.
        let (position, _, _, _) = player(&mut world);
        assert!(position.z.abs() > 2.0, "the head bump halted the run");
    }
}
