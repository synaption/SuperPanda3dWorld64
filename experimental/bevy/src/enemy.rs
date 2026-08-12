use crate::{level::LevelData, player::Player};
use bevy::prelude::*;

#[derive(Component)]
pub struct Enemy {
    pub origin: Vec3,
    pub phase: f32,
    pub animation: Handle<AnimationClip>,
}

pub fn update(
    time: Res<Time>,
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    mut enemies: Query<(&Enemy, &mut Transform), Without<Player>>,
) {
    let player = player.single().translation;
    for (enemy, mut transform) in &mut enemies {
        let to_player = player - transform.translation;
        if to_player.length() < 12.0 {
            let dir = Vec3::new(to_player.x, 0.0, to_player.z).normalize_or_zero();
            transform.translation += dir * time.delta_seconds() * 1.8;
            transform.rotation = Quat::from_rotation_y(dir.x.atan2(dir.z));
        } else {
            let a = time.elapsed_seconds() * 0.35 + enemy.phase;
            let target = enemy.origin + Vec3::new(a.sin(), 0.0, a.cos()) * 2.0;
            transform.translation = transform.translation.lerp(target, time.delta_seconds());
        }
        if let Some(floor) = level.floor_height(transform.translation + Vec3::Y * 2.0) {
            transform.translation.y = floor;
        }
    }
}
