//! Warp pipes, and the arc the things they throw fly on.
//!
//! Ported from `WarpPipe` and `Object.coast` in `sm64py/objects.py`. A pipe
//! does not place something beside itself: it throws it up out of the barrel,
//! and the thing flies a ballistic arc with its own behaviour suspended until
//! it lands. That suspension is the whole trick -- every behaviour in the game
//! writes its own speed each tick, so a launch handed straight to a goomba's
//! walk is gone within a tick or two and it lands back on the pipe it came out
//! of.
//!
//! Every constant here is the original's, converted once: the port's world
//! scale is 1/100 and its clock is 30 Hz, so a speed of *n* SM64 units a frame
//! is `n * 0.3` world units a second and an acceleration of *n* units a frame
//! squared is `n * 9` world units a second squared.

use crate::{
    console::GameTuning,
    enemy::{self, Enemy, Kind},
    level::LevelData,
    player::{Motion, FIXED_DT},
    squad::{self, Ally, GOLDEN_ANGLE},
};
use bevy::prelude::*;

/// Take-off speed: 60 units a frame. Gravity takes 4 a frame back, so the
/// launch peaks `v*v/8` = 450 units up -- clearing the pipe's own 205-unit rim
/// by as much again -- and stays up for `v/2` frames, a full second.
const LAUNCH_RISE: f32 = 18.0;

/// Carried outwards for the whole of that second, so it lands about 600 units
/// out: four pipe-widths, and far enough that a brood of five comes down spread
/// around the pipe rather than in a stack on top of it.
const LAUNCH_SPEED: f32 = 6.0;

/// `OBJECT_GRAVITY`, which is -4 units a frame squared.
const LAUNCH_GRAVITY: f32 = 36.0;

/// `LAUNCH_MAX_TICKS`, 120 ticks. A backstop rather than a rule: something
/// thrown off the edge of the map would otherwise fly for ever.
const LAUNCH_MAX_SECONDS: f32 = 4.0;

/// How far below the pipe a landing spot may be and still count as somewhere to
/// throw something, and how many headings are tried before giving up. Thrown
/// this far, direction matters: a pipe standing a few hundred units from where
/// the ground drops into the moat otherwise puts a share of its brood in the
/// water.
const LANDING_DROP: f32 = 4.0;
const LANDING_TRIES: usize = 8;

/// How high above a candidate spot the floor is probed from. The probe looks
/// downward from this height, so it has to start above whatever it is meant to
/// find.
const PROBE_HEIGHT: f32 = 2.0;

/// How long the Mario pipe waits between one and the next.
///
/// Its own number rather than the `enemy_rate` the other two read. The Mario
/// pipe produces company rather than enemies, and a slider labelled "enemy"
/// dragging it too would be a surprise -- which is exactly the split
/// `PipeTuning` in `app/main.py` draws.
pub const MARIO_INTERVAL: f32 = 12.0;

/// What a pipe produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spawn {
    Enemy(Kind),
    Mario,
}

impl Spawn {
    /// Whether the enemy sliders speak for a pipe that produces this.
    fn is_enemy(self) -> bool {
        matches!(self, Self::Enemy(_))
    }
}

/// A pipe that things climb out of, up to a population it then holds.
///
/// The count is of its own brood rather than of the field: the level already
/// has enemies placed by hand, and a pipe that counted those would stop short
/// of its own quota or never fire at all. So each pipe is responsible for
/// exactly what it produced, and for replacing it when it dies.
///
/// The countdown only runs while there is room. At the cap the timer holds
/// where it stands, so a kill is what starts the clock again rather than
/// restarting it -- the difference between "one along every so often" and a
/// replacement appearing the instant something dies.
#[derive(Component)]
pub struct WarpPipe {
    /// What comes out.
    pub spawns: Spawn,
    /// The countdown to the next one.
    pub timer: Timer,
    /// What this pipe has produced and is still alive. Entities rather than a
    /// count, so something despawned elsewhere drops out of the quota by
    /// itself.
    pub brood: Vec<Entity>,
    /// Advanced every time a heading is tried, so no two throws from a pipe --
    /// or from two pipes -- go the same way, with no random number generator
    /// anywhere and a whole run reproducible in a test.
    phase: f32,
}

impl WarpPipe {
    pub fn new(spawns: Spawn, interval: f32, phase: f32) -> Self {
        Self {
            spawns,
            timer: Timer::from_seconds(interval, TimerMode::Repeating),
            brood: Vec::new(),
            phase,
        }
    }
}

/// Anything a pipe produced. Its brood is its own business: the console's
/// `ally_count` reconciles the field's standing crowd of Marios and leaves the
/// pipe's to the pipe, the same way the pipes leave the hand-placed enemies to
/// the level.
#[derive(Component)]
pub struct Brood;

/// In the air on the arc a pipe threw it, with its own behaviour suspended.
#[derive(Component)]
pub struct Launched {
    pub velocity: Vec3,
    /// Seconds of flight left before the backstop cuts it short.
    pub left: f32,
}

impl Launched {
    pub fn new(velocity: Vec3) -> Self {
        Self {
            velocity,
            left: LAUNCH_MAX_SECONDS,
        }
    }
}

/// How far out the arc puts something down: airborne for twice the time it
/// takes gravity to cancel the launch, carried outwards throughout.
pub fn reach() -> f32 {
    LAUNCH_SPEED * 2.0 * LAUNCH_RISE / LAUNCH_GRAVITY
}

/// The velocity a throw on this heading leaves the pipe with.
pub fn launch_velocity(yaw: f32) -> Vec3 {
    Vec3::new(
        yaw.sin() * LAUNCH_SPEED,
        LAUNCH_RISE,
        yaw.cos() * LAUNCH_SPEED,
    )
}

/// Picks a heading with something to land on at the end of it.
///
/// Tries headings a golden angle apart until one has floor under it that is not
/// far below the pipe, and throws on the last one tried if none has: ringed by
/// drops, apparently, and throwing it anyway beats not firing at all.
fn landing_yaw(level: &LevelData, from: Vec3, phase: &mut f32) -> f32 {
    let reach = reach();
    let mut yaw = *phase;
    for _ in 0..LANDING_TRIES {
        yaw = *phase;
        *phase += GOLDEN_ANGLE;
        let spot = from + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * reach;
        if let Some(height) = level.floor_height(spot + Vec3::Y * PROBE_HEIGHT) {
            if height > from.y - LANDING_DROP {
                return yaw;
            }
        }
    }
    yaw
}

/// Runs every pipe's countdown and throws one out when it comes up.
#[allow(clippy::too_many_arguments)]
pub fn fire(
    mut commands: Commands,
    fixed_time: Res<Time<Fixed>>,
    assets: Res<AssetServer>,
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    // No filter and no component access: this is only asked whether an entity
    // is still alive, so it conflicts with nothing else in the system.
    alive: Query<Entity>,
    enemies: Query<(), With<Enemy>>,
    mut pipes: Query<(&Transform, &mut WarpPipe)>,
) {
    let quota = tuning.pipe_brood.round() as usize;
    let enemy_limit = tuning.enemy_limit.round() as usize;
    // Commands are deferred, so the query cannot see enemies born earlier in
    // this loop. Keep the count locally as well or two ready pipes can both
    // consume the final slot and take the field one over its advertised cap.
    let mut enemy_count = enemies.iter().len();
    for (transform, mut pipe) in &mut pipes {
        // Whatever has died since the last tick stops counting against the
        // quota, which is what starts the clock again.
        pipe.brood.retain(|member| alive.contains(*member));
        if pipe.spawns.is_enemy() {
            // Enemy pipes answer to the field-wide limit. Giving them the
            // per-pipe brood quota too made enemy_limit inert above the ten
            // enemies the two default pipes could produce between them.
            if enemy_count >= enemy_limit {
                continue;
            }
            // A rate cut also pulls in a countdown already running: a pipe part
            // way through a long wait would otherwise ignore the new number
            // until the old one had elapsed, which reads as a slider that does
            // nothing for half a minute.
            pipe.timer
                .set_duration(std::time::Duration::from_secs_f32(tuning.enemy_rate));
        } else if pipe.brood.len() >= quota {
            // The Mario pipe is not part of enemy_limit; pipe_brood remains
            // its own replacement quota.
            continue;
        }
        pipe.timer.tick(fixed_time.delta());
        if !pipe.timer.just_finished() {
            continue;
        }
        // Thrown from the pipe's feet rather than from its lip: the launch
        // carries it up through the barrel and out of the top, which is what
        // the pop is. It starts hidden inside instead of appearing in mid-air
        // above.
        let mouth = transform.translation;
        let phase = pipe.phase;
        let yaw = landing_yaw(&level, mouth, &mut pipe.phase);
        let born = match pipe.spawns {
            Spawn::Enemy(kind) => {
                enemy_count += 1;
                enemy::spawn(&mut commands, &assets, kind, mouth, phase)
            }
            Spawn::Mario => squad::spawn_ally(&mut commands, &assets, mouth, phase),
        };
        commands
            .entity(born)
            .insert((Brood, Launched::new(launch_velocity(yaw))));
        pipe.brood.push(born);
    }
}

/// Flies everything currently on a launch arc.
///
/// Resolved here rather than in the behaviours it belongs to, because an arc
/// wants its landing detected the tick it happens -- that is what ends the
/// launch. Both the enemy step and the ally step skip anything with `Launched`
/// on it, so for the second or so it is in the air nothing else moves it.
pub fn fly(
    mut commands: Commands,
    level: Res<LevelData>,
    mut flying: Query<(Entity, &mut Launched, &mut Transform, Option<&mut Ally>)>,
) {
    for (entity, mut launch, mut transform, ally) in &mut flying {
        launch.left -= FIXED_DT;
        let step = launch.velocity * FIXED_DT;
        transform.translation += step;
        // Face the way it is going, so it does not fly out backwards.
        let heading = Vec3::new(launch.velocity.x, 0.0, launch.velocity.z);
        if heading.length_squared() > 1e-6 {
            transform.rotation = Quat::from_rotation_y(heading.x.atan2(heading.z));
        }
        let floor = level.floor_height(transform.translation + Vec3::Y * PROBE_HEIGHT);
        let landed = floor.is_some_and(|height| transform.translation.y <= height);
        if let Some(height) = floor.filter(|_| landed) {
            transform.translation.y = height;
        }
        if let Some(mut ally) = ally {
            // Mario has clips for being in the air; the enemies have one clip
            // each and keep it.
            ally.velocity = launch.velocity;
            ally.state.motion = if launch.velocity.y > 0.0 {
                Motion::Jump
            } else {
                Motion::Fall
            };
            ally.state.speed = launch.velocity.length();
            ally.state.still_for = 0.0;
        }
        if landed || launch.left <= 0.0 {
            commands.entity(entity).remove::<Launched>();
            continue;
        }
        launch.velocity.y -= LAUNCH_GRAVITY * FIXED_DT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One flat floor, wider than any arc here can reach.
    fn ground(height: f32) -> LevelData {
        let corners = [
            Vec3::new(-200., height, -200.),
            Vec3::new(200., height, -200.),
            Vec3::new(200., height, 200.),
            Vec3::new(-200., height, 200.),
        ];
        LevelData::new(corners.to_vec(), vec![[0, 1, 2], [0, 2, 3]], Vec::new())
    }

    /// Floor on one side of the origin and open air on the other: a cliff to
    /// throw things off, standing in for the moat.
    fn cliff() -> LevelData {
        let corners = [
            Vec3::new(-200., 0., 0.),
            Vec3::new(200., 0., 0.),
            Vec3::new(200., 0., 200.),
            Vec3::new(-200., 0., 200.),
        ];
        LevelData::new(corners.to_vec(), vec![[0, 1, 2], [0, 2, 3]], Vec::new())
    }

    /// Nothing anywhere to land on.
    fn void() -> LevelData {
        LevelData::new(Vec::new(), Vec::new(), Vec::new())
    }

    /// The arc has to clear the pipe it comes out of. A pipe is 205 units tall
    /// -- 2.05 here -- and something that does not get over the rim has not
    /// come out of the top of anything.
    #[test]
    fn a_launch_clears_the_rim_of_the_pipe() {
        let mut world = World::new();
        world.insert_resource(ground(0.0));
        let thrown = world
            .spawn((
                Launched::new(launch_velocity(0.0)),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();
        let mut apex: f32 = 0.0;
        for _ in 0..120 {
            world.run_system_once(fly).expect("fly could not run");
            let height = world.get::<Transform>(thrown).unwrap().translation.y;
            apex = apex.max(height);
            if world.get::<Launched>(thrown).is_none() {
                break;
            }
        }
        assert!(
            apex > 2.05,
            "the throw peaked {apex:.2} up, which does not clear the pipe"
        );
    }

    /// It comes down again, on the ground, and the launch ends there. A throw
    /// that never lands is an enemy hovering over the field for ever.
    #[test]
    fn a_launch_lands_and_hands_the_thing_back() {
        let mut world = World::new();
        world.insert_resource(ground(0.0));
        let thrown = world
            .spawn((
                Launched::new(launch_velocity(0.0)),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();
        for _ in 0..120 {
            world.run_system_once(fly).expect("fly could not run");
            if world.get::<Launched>(thrown).is_none() {
                break;
            }
        }
        assert!(
            world.get::<Launched>(thrown).is_none(),
            "still in the air after four seconds"
        );
        let landed = world.get::<Transform>(thrown).unwrap().translation;
        assert!(
            (landed.y - 0.0).abs() < 0.2,
            "it came to rest at {:.2}, not on the floor",
            landed.y
        );
        // Thrown along +Z at the launch speed for the second it is up.
        let out = Vec2::new(landed.x, landed.z).length();
        assert!(
            (out - reach()).abs() < 0.5,
            "it landed {out:.2} out, not the {:.2} the arc reaches",
            reach()
        );
    }

    /// The backstop. Thrown off the edge of the map there is no floor to land
    /// on, and without it the thing falls for ever with its behaviour switched
    /// off.
    #[test]
    fn a_throw_into_nothing_still_ends() {
        let mut world = World::new();
        world.insert_resource(void());
        let thrown = world
            .spawn((
                Launched::new(launch_velocity(0.0)),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ))
            .id();
        for _ in 0..(LAUNCH_MAX_SECONDS / FIXED_DT) as usize + 2 {
            world.run_system_once(fly).expect("fly could not run");
        }
        assert!(world.get::<Launched>(thrown).is_none());
    }

    /// Successive throws go different ways. Five of them out of one pipe on one
    /// heading is a stack, not a brood.
    #[test]
    fn successive_throws_are_spread_around_the_pipe() {
        let level = ground(0.0);
        let mut phase = 0.0;
        let mut headings = Vec::new();
        for _ in 0..5 {
            headings.push(landing_yaw(&level, Vec3::ZERO, &mut phase));
        }
        for (index, &yaw) in headings.iter().enumerate() {
            for &other in &headings[index + 1..] {
                let apart = (yaw - other).abs() % std::f32::consts::TAU;
                let apart = apart.min(std::f32::consts::TAU - apart);
                assert!(
                    apart > 0.5,
                    "two of the five throws went {apart:.2} radians apart"
                );
            }
        }
    }

    /// A heading over a drop is refused while there is a better one. The pipe
    /// by the moat otherwise throws a share of its brood into the water.
    #[test]
    fn a_heading_over_a_drop_is_passed_over() {
        let level = cliff();
        let mut phase = 0.0;
        for _ in 0..12 {
            let yaw = landing_yaw(&level, Vec3::ZERO, &mut phase);
            let spot = Vec3::new(yaw.sin(), 0.0, yaw.cos()) * reach();
            assert!(
                level
                    .floor_height(spot + Vec3::Y * PROBE_HEIGHT)
                    .is_some_and(|height| height > -LANDING_DROP),
                "thrown to {:.2}, {:.2}, where there is no floor",
                spot.x,
                spot.z
            );
        }
    }
}
