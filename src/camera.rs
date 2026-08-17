use crate::{
    console::GameTuning,
    input::InputState,
    level::LevelData,
    player::{Player, RenderPose},
    GameState,
};
use bevy::prelude::*;

#[derive(Component)]
pub struct FollowCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

/// Pitch range, in radians. Short of straight up and straight down: the boom
/// passing through either pole gives the player no ground to read.
const PITCH_LIMITS: (f32, f32) = (-0.75, 0.85);

/// The look stick's shape. Squaring the deflection keeps the first half of the
/// stick's travel slow enough to aim with while the far end still whips the
/// view around, which a linear stick cannot do at any single sensitivity.
fn stick_curve(deflection: f32) -> f32 {
    deflection * deflection.abs()
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<InputState>,
    mut cameras: Query<(&mut Transform, &mut FollowCamera), Without<Player>>,
    player: Res<RenderPose>,
    level: Res<LevelData>,
    mut state: ResMut<GameState>,
    tuning: Res<GameTuning>,
) {
    let Ok((mut camera, mut follow)) = cameras.single_mut() else {
        return;
    };
    follow.yaw -= input.look_mouse.x * tuning.mouse_sens;
    follow.pitch = (follow.pitch - input.look_mouse.y * tuning.mouse_sens * 0.8333)
        .clamp(PITCH_LIMITS.0, PITCH_LIMITS.1);
    // The stick is a rate rather than a displacement, so it is scaled by the
    // frame's length where the mouse is not.
    let stick = time.delta_secs() * tuning.pad_look;
    follow.yaw -= stick_curve(input.look_stick.x) * stick;
    follow.pitch = (follow.pitch + stick_curve(input.look_stick.y) * stick * 0.8333)
        .clamp(PITCH_LIMITS.0, PITCH_LIMITS.1);
    if keys.pressed(KeyCode::KeyQ) {
        follow.yaw += 0.035;
    }
    if keys.pressed(KeyCode::KeyE) {
        follow.yaw -= 0.035;
    }
    if InputState::take(&mut input.recenter) {
        let forward = player.rotation * Vec3::NEG_Z;
        follow.yaw = forward.x.atan2(forward.z);
    }
    state.aiming = input.aim;
    let desired_distance = if state.aiming {
        tuning.cam_aim_distance
    } else {
        tuning.cam_distance
    };
    follow.distance += (desired_distance - follow.distance) * 0.16;
    let focus = player.translation + Vec3::Y * tuning.cam_height;
    let orbit = Quat::from_rotation_y(follow.yaw) * Quat::from_rotation_x(follow.pitch);
    let mut wanted = focus + orbit * Vec3::new(0.0, 0.8, follow.distance);
    if let Some(hit) = level.segment_hit(focus, wanted) {
        // Leave a small gap so the near plane never sits inside the wall.
        wanted = hit + (focus - hit).normalize_or_zero() * 0.3;
    }
    camera.translation = camera.translation.lerp(wanted, tuning.cam_smooth);
    camera.look_at(focus, Vec3::Y);
}
