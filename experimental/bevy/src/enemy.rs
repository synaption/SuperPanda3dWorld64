use crate::{
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    level::LevelData,
    player::{Controller, Player},
};
use bevy::prelude::*;

#[derive(Component)]
pub struct Enemy {
    pub origin: Vec3,
    pub phase: f32,
    pub animation: Handle<AnimationClip>,
}

#[derive(Component)]
pub struct WarpPipe {
    pub timer: Timer,
}

/// Connects the AnimationPlayer created inside a GLB scene to its enemy root.
#[derive(Component)]
pub struct EnemyAnimationRoot(pub Entity);

pub fn update(
    fixed_time: Res<Time<Fixed>>,
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    mut enemies: Query<(Entity, &Enemy, &mut Transform, &mut Visibility), Without<Player>>,
    tuning: Res<GameTuning>,
    mut fixed_tick: Local<u32>,
) {
    let player = player.single().translation;
    let elapsed = fixed_time.elapsed_seconds();
    *fixed_tick = fixed_tick.wrapping_add(1);
    let tick = *fixed_tick;
    enemies
        .par_iter_mut()
        .for_each(|(entity, enemy, mut transform, mut visibility)| {
            let to_player = player - transform.translation;
            let distance_squared = to_player.length_squared();
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
            if (tick + entity.index()) % stride != 0 {
                return;
            }
            // A reduced-rate step covers the skipped fixed ticks, so far actors
            // retain the same average movement and animation-independent AI time.
            let dt = crate::player::FIXED_DT * stride as f32;
            if distance_squared < 12.0 * 12.0 {
                let dir = Vec3::new(to_player.x, 0.0, to_player.z).normalize_or_zero();
                transform.translation += dir * dt * tuning.enemy_speed;
                transform.rotation = Quat::from_rotation_y(dir.x.atan2(dir.z));
            } else {
                let a = elapsed * 0.35 + enemy.phase;
                let target = enemy.origin + Vec3::new(a.sin(), 0.0, a.cos()) * 2.0;
                transform.translation = transform.translation.lerp(target, dt);
            }
            if let Some(floor) = level.floor_height(transform.translation + Vec3::Y * 2.0) {
                transform.translation.y = floor;
            }
        });
}

/// Resolves the lightweight combat rules shared by both playable characters:
/// attacks defeat nearby enemies in front of the player, falling onto an
/// enemy stomps it, and touching one from the side knocks the player away.
#[allow(clippy::type_complexity)]
pub fn combat(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut player: Query<(&Transform, &mut Controller), With<Player>>,
    enemies: Query<(Entity, &Transform), (With<Enemy>, Without<Player>)>,
) {
    let (player_transform, mut controller) = player.single_mut();
    controller.invulnerable_left =
        (controller.invulnerable_left - crate::player::FIXED_DT).max(0.0);
    let facing = player_transform.rotation * Vec3::Z;
    for (entity, enemy) in &enemies {
        let offset = enemy.translation - player_transform.translation;
        let horizontal = Vec3::new(offset.x, 0.0, offset.z);
        let distance_squared = horizontal.length_squared();
        let attacking = controller.attack_left > 0.0
            && distance_squared < 2.2 * 2.2
            && facing.dot(horizontal.normalize_or_zero()) > -0.15;
        let stomping = distance_squared < 0.75 * 0.75
            && offset.y < -0.35
            && offset.y > -1.8
            && controller.velocity.y < -1.0;
        if attacking || stomping {
            commands.entity(entity).despawn_recursive();
            sounds.push(Sfx::Defeat);
            if stomping {
                controller.velocity.y = 8.5;
                controller.grounded = false;
            }
        } else if distance_squared < 0.75 * 0.75
            && offset.y.abs() < 1.3
            && controller.invulnerable_left <= 0.0
        {
            let away = (-horizontal).normalize_or_zero();
            controller.velocity = away * 7.0 + Vec3::Y * 6.0;
            controller.grounded = false;
            controller.invulnerable_left = 1.0;
            controller.health = controller.health.saturating_sub(1);
            sounds.push(Sfx::Hurt);
        }
    }
}

/// Active pipes replenish a small enemy population when the player is close.
/// A global cap keeps unattended pipes from growing the world indefinitely.
pub fn spawn_from_pipes(
    mut commands: Commands,
    fixed_time: Res<Time<Fixed>>,
    assets: Res<AssetServer>,
    player: Query<&Transform, With<Player>>,
    enemies: Query<(), With<Enemy>>,
    mut pipes: Query<(&Transform, &mut WarpPipe), Without<Player>>,
    tuning: Res<GameTuning>,
) {
    if enemies.iter().len() >= tuning.enemy_limit.round() as usize {
        return;
    }
    let player_position = player.single().translation;
    for (transform, mut pipe) in &mut pipes {
        pipe.timer
            .set_duration(std::time::Duration::from_secs_f32(tuning.enemy_rate));
        if transform.translation.distance(player_position) > 28.0 {
            continue;
        }
        pipe.timer.tick(fixed_time.delta());
        if !pipe.timer.just_finished() {
            continue;
        }
        let position = transform.translation + Vec3::Y * 0.4;
        commands.spawn((
            Enemy {
                origin: position,
                phase: fixed_time.elapsed_seconds(),
                animation: assets.load("actors/goomba.glb#Animation0"),
            },
            SceneBundle {
                scene: assets.load("actors/goomba.glb#Scene0"),
                transform: Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
                ..default()
            },
        ));
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
            player.pause();
        } else {
            player.resume();
        }
    }
}
