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

/// Downward speed a landing needs before it is loud enough to be heard. Below
/// this the player is settling onto a slope, not landing on it.
const LANDING_IMPACT: f32 = 2.0;

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
    Skate,
    Fly,
    Swim,
    Attack,
}

#[derive(Component)]
pub struct Controller {
    pub velocity: Vec3,
    pub grounded: bool,
    pub motion: Motion,
    pub attack_left: f32,
    pub invulnerable_left: f32,
    pub health: u8,
    pub swimming: bool,
    /// Distance run since the last footfall, and seconds since the last swim
    /// stroke. Both only drive audio cadence.
    step_phase: f32,
    stroke_phase: f32,
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
            swimming: false,
            step_phase: 0.0,
            stroke_phase: 0.0,
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
    let (mut transform, mut previous, mut ctrl) = player.single_mut();
    previous.translation = transform.translation;
    previous.rotation = transform.rotation;
    let cam = camera.single();
    // Latched edges are consumed here rather than polled, so each press acts
    // on exactly one fixed step however many steps this frame runs.
    let jump_pressed = InputState::take(&mut input_state.jump);
    let attack_pressed = InputState::take(&mut input_state.attack);
    let input = input_state.move_axis;
    let forward = (cam.forward() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let right = (cam.right() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let wish = forward * input.y + right * input.x;
    let water_level = level.water_level(transform.translation.x, transform.translation.z);
    let was_swimming = ctrl.swimming;
    ctrl.swimming = water_level.is_some_and(|surface| transform.translation.y < surface - 0.15);
    if ctrl.swimming != was_swimming {
        sounds.push(Sfx::Splash);
    }
    let boost = input_state.boost && state.active == ActiveCharacter::Hero;
    let skate = boost && ctrl.grounded;
    let speed = if ctrl.swimming {
        if state.active == ActiveCharacter::Mario {
            tuning.mario_swim
        } else {
            tuning.hero_wade
        }
    } else if skate {
        tuning.skate_speed
    } else if state.active == ActiveCharacter::Hero {
        tuning.hero_speed
    } else {
        tuning.mario_speed
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

    if ctrl.swimming {
        let surface = water_level.unwrap()
            - if state.active == ActiveCharacter::Mario {
                0.45
            } else {
                0.25
            };
        let buoyancy = ((surface - transform.translation.y) * 2.0).clamp(-2.0, 3.0);
        ctrl.velocity.y += (buoyancy - ctrl.velocity.y) * 0.16;
        if jump_pressed {
            ctrl.velocity.y = (ctrl.velocity.y + 3.8).min(6.0);
            sounds.push(Sfx::Stroke);
        }
        ctrl.grounded = false;
    } else if jump_pressed && ctrl.grounded {
        ctrl.velocity.y = if skate { 9.0 } else { tuning.jump_velocity };
        ctrl.grounded = false;
        sounds.push(Sfx::Jump);
    }
    if ctrl.swimming {
        // Water supplies buoyancy above; gravity and the Hero booster do not
        // operate until the capsule leaves the water box.
    } else if boost && !ctrl.grounded {
        ctrl.velocity.y = (ctrl.velocity.y + tuning.jet_thrust).min(tuning.jet_rise);
    } else if !ctrl.grounded {
        ctrl.velocity.y -= 1.2;
    }

    if attack_pressed {
        ctrl.attack_left = 0.55;
        ctrl.motion = Motion::Attack;
        sounds.push(Sfx::Attack);
    }
    ctrl.attack_left = (ctrl.attack_left - FIXED_DT).max(0.0);
    transform.translation.y += ctrl.velocity.y * FIXED_DT;
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
    if !ctrl.swimming && ctrl.velocity.y > 0.0 {
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
    if ctrl.swimming {
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
    if ctrl.swimming {
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

    if ctrl.attack_left <= 0.0 {
        ctrl.motion = if ctrl.grounded {
            if skate {
                Motion::Skate
            } else if horizontal.length() > 0.25 {
                Motion::Run
            } else {
                Motion::Idle
            }
        } else if ctrl.swimming {
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
    let (root, previous) = player.single();
    let alpha = fixed_time.overstep_percentage().clamp(0.0, 1.0);
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
    use bevy::{core_pipeline::core_3d::Camera3d, ecs::system::RunSystemOnce};

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
            world.run_system_once(movement);
        }
    }

    /// The player's position, and the parts of its controller the tests read.
    fn player(world: &mut World) -> (Vec3, Vec3, bool, Motion) {
        let mut query = world.query_filtered::<(&Transform, &Controller), With<Player>>();
        let (transform, controller) = query.single(world);
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
