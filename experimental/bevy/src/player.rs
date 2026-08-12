use crate::{level::LevelData, ActiveCharacter, GameState};
use bevy::prelude::*;

pub const FIXED_DT: f32 = 1.0 / 30.0;

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
    Attack,
}

#[derive(Component)]
pub struct Controller {
    pub velocity: Vec3,
    pub grounded: bool,
    pub motion: Motion,
    pub attack_left: f32,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: false,
            motion: Motion::Fall,
            attack_left: 0.0,
        }
    }
}

pub fn movement(
    keys: Res<Input<KeyCode>>,
    level: Res<LevelData>,
    state: Res<GameState>,
    mut player: Query<(&mut Transform, &mut PreviousPose, &mut Controller), With<Player>>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
) {
    let (mut transform, mut previous, mut ctrl) = player.single_mut();
    previous.translation = transform.translation;
    previous.rotation = transform.rotation;
    let cam = camera.single();
    let mut input = Vec2::ZERO;
    if keys.pressed(KeyCode::W) || keys.pressed(KeyCode::Up) {
        input.y += 1.0;
    }
    if keys.pressed(KeyCode::S) || keys.pressed(KeyCode::Down) {
        input.y -= 1.0;
    }
    if keys.pressed(KeyCode::D) || keys.pressed(KeyCode::Right) {
        input.x += 1.0;
    }
    if keys.pressed(KeyCode::A) || keys.pressed(KeyCode::Left) {
        input.x -= 1.0;
    }
    input = input.clamp_length_max(1.0);
    let forward = (cam.forward() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let right = (cam.right() * Vec3::new(1.0, 0.0, 1.0)).normalize_or_zero();
    let wish = forward * input.y + right * input.x;
    let boost = keys.pressed(KeyCode::V) && state.active == ActiveCharacter::Hero;
    let skate = boost && ctrl.grounded;
    let speed = if skate {
        16.8
    } else if state.active == ActiveCharacter::Hero {
        11.4
    } else {
        9.6
    };
    let accel = if skate { 9.0 } else { 22.0 };
    let target = wish * speed;
    let horizontal = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z);
    let next = if wish.length_squared() > 0.0 {
        horizontal.lerp(target, (accel * FIXED_DT).min(1.0))
    } else {
        horizontal.lerp(
            Vec3::ZERO,
            (if skate { 0.35 } else { 10.0 } * FIXED_DT).min(1.0),
        )
    };
    ctrl.velocity.x = next.x;
    ctrl.velocity.z = next.z;

    if keys.just_pressed(KeyCode::Space) && ctrl.grounded {
        ctrl.velocity.y = if skate { 9.0 } else { 12.6 };
        ctrl.grounded = false;
    }
    if boost && !ctrl.grounded {
        ctrl.velocity.y = (ctrl.velocity.y + 2.4).min(6.0);
    } else if !ctrl.grounded {
        ctrl.velocity.y -= 1.2;
    }

    if keys.just_pressed(KeyCode::ShiftLeft) {
        ctrl.attack_left = 0.55;
        ctrl.motion = Motion::Attack;
    }
    ctrl.attack_left = (ctrl.attack_left - FIXED_DT).max(0.0);
    transform.translation += ctrl.velocity * FIXED_DT;
    let floor = level.floor_height(transform.translation + Vec3::Y * 0.75);
    if let Some(height) = floor {
        let separation = transform.translation.y - height;
        let walking_on_floor = ctrl.grounded && separation <= 0.75;
        let landed = transform.translation.y <= height && ctrl.velocity.y <= 0.0;
        if walking_on_floor || landed {
            transform.translation.y = height;
            ctrl.velocity.y = 0.0;
            ctrl.grounded = true;
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
    if wish.length_squared() > 0.01 {
        let yaw = wish.x.atan2(wish.z);
        transform.rotation = transform.rotation.slerp(Quat::from_rotation_y(yaw), 0.28);
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
        } else if boost {
            Motion::Fly
        } else if ctrl.velocity.y > 0.0 {
            Motion::Jump
        } else {
            Motion::Fall
        };
    }
}

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
