use crate::{
    player::{Player, RenderPose},
    GameState,
};
use bevy::{input::mouse::MouseMotion, prelude::*};

#[derive(Component)]
pub struct FollowCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

pub fn update(
    mut mouse: EventReader<MouseMotion>,
    buttons: Res<Input<MouseButton>>,
    keys: Res<Input<KeyCode>>,
    mut cameras: Query<(&mut Transform, &mut FollowCamera), Without<Player>>,
    player: Res<RenderPose>,
    mut state: ResMut<GameState>,
) {
    let (mut camera, mut follow) = cameras.single_mut();
    for delta in mouse.read() {
        follow.yaw -= delta.delta.x * 0.003;
        follow.pitch = (follow.pitch - delta.delta.y * 0.0025).clamp(-0.75, 0.85);
    }
    if keys.pressed(KeyCode::Q) {
        follow.yaw += 0.035;
    }
    if keys.pressed(KeyCode::E) {
        follow.yaw -= 0.035;
    }
    if keys.just_pressed(KeyCode::R) {
        let forward = player.rotation * Vec3::NEG_Z;
        follow.yaw = forward.x.atan2(forward.z);
    }
    state.aiming = buttons.pressed(MouseButton::Right) || keys.pressed(KeyCode::F);
    let desired_distance = if state.aiming { 5.7 } else { 9.5 };
    follow.distance += (desired_distance - follow.distance) * 0.16;
    let focus = player.translation + Vec3::Y * 1.35;
    let orbit = Quat::from_rotation_y(follow.yaw) * Quat::from_rotation_x(follow.pitch);
    let wanted = focus + orbit * Vec3::new(0.0, 0.8, follow.distance);
    camera.translation = camera.translation.lerp(wanted, 0.24);
    camera.look_at(focus, Vec3::Y);
}
