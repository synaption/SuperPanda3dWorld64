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
use bevy::prelude::*;

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
}

#[derive(Component)]
pub struct Enemy {
    /// What it is, which is also its collision cylinder and its model: kept as
    /// the one fact rather than as a copy of each thing derived from it.
    pub kind: Kind,
    pub origin: Vec3,
    pub phase: f32,
    pub animation: Handle<AnimationClip>,
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
                origin: position,
                phase,
                animation: assets.load(format!("{}#Animation0", kind.model())),
            },
            WorldAssetRoot(assets.load(format!("{}#Scene0", kind.model()))),
            Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
            // Parts of both of these are flat quads the original turns to face
            // the camera every frame.
            crate::billboard::BillboardActor,
        ))
        .id()
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
        &'static Enemy,
        &'static mut Transform,
        &'static mut Visibility,
    ),
    (Without<Player>, Without<crate::pipe::Launched>),
>;

pub fn update(
    fixed_time: Res<Time<Fixed>>,
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
    let elapsed = fixed_time.elapsed_secs();
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
            if !(tick + entity.index().index()).is_multiple_of(stride) {
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
        if here.y > transform.translation.y + height
            || here.y + PLAYER_HEIGHT < transform.translation.y
        {
            continue;
        }
        if controller.velocity.y < 0.0 && here.y > transform.translation.y + height * STOMP_MARGIN {
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
                    origin: Vec3::ZERO,
                    phase: 0.0,
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
        assert!(
            world.get_entity(enemy).is_ok(),
            "the enemy died on contact"
        );
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

    /// Standing on a roof directly above one is not touching it.
    #[test]
    fn an_enemy_a_storey_below_is_out_of_reach() {
        let (mut world, enemy) = world(4.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok());
        assert_eq!(controller(&mut world).1, 3, "hurt from a storey up");
    }
}
