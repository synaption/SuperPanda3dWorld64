use crate::{
    console::GameTuning,
    gravity::{self, Gravity},
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
    /// How much of the boom the level currently leaves free, from 0 at the
    /// player's head to 1 at the full [`Self::distance`]. Kept between frames
    /// because it is eased rather than applied raw -- see [`update`].
    pub clearance: f32,
    /// The frame [`Self::yaw`] and [`Self::pitch`] are measured in.
    ///
    /// Identity on a flat level, where the orbit is about world `+Y` and always
    /// was. On a planet it is carried along as the player walks: each frame it
    /// is turned by the smallest rotation that takes its own up onto the local
    /// one, which is parallel transport and is the only part of this that is
    /// not obvious.
    ///
    /// The obvious thing -- rebuilding the frame from scratch out of the local
    /// up every frame -- does not work. `Quat::from_rotation_arc(Vec3::Y, up)`
    /// has no answer at the antipode of `+Y` and an arbitrary one near it, so a
    /// player walking towards the planet's south pole would find the view
    /// spinning faster and faster and then flipping over. Turning by the small
    /// step from *last* frame's up to this one never asks that question,
    /// because between two frames the up has barely moved.
    pub frame: Quat,
    /// The frame the view is actually built in, chasing [`Self::frame`].
    ///
    /// Two frames rather than one, which is how the Outer Wilds prototype in
    /// `experimental/ow` is arranged and the reason walking a planet there does
    /// not feel jerky: what the ground says and what the camera does are kept
    /// apart, with a rate between them. `frame` is the ground's answer, exact
    /// and updated every frame; `view` lags it by [`UP_ALIGN_RATE`].
    ///
    /// It is the *frame* that lags and not the whole orbit. Yaw and pitch are
    /// the player's own input and stay immediate -- easing those would be input
    /// latency, and the prototype avoids it the other way round, by running its
    /// camera filter four times faster on foot than in flight.
    pub view: Quat,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.2,
            distance: 9.5,
            // Starts fully extended: the first frame eases in from wherever the
            // level actually leaves room, rather than out from the player's
            // head.
            clearance: 1.0,
            frame: Quat::IDENTITY,
            view: Quat::IDENTITY,
        }
    }
}

/// Pitch range, in radians. Short of straight up and straight down: the boom
/// passing through either pole gives the player no ground to read.
const PITCH_LIMITS: (f32, f32) = (-0.75, 0.85);

/// The gap left between the camera and a wall it has been pulled in front of,
/// so the near plane never ends up inside the geometry.
const WALL_GAP: f32 = 0.3;

/// The rate the boom is allowed back out at once a wall stops blocking it,
/// as a fraction of the remaining gap per sixtieth of a second.
const REOPEN_RATE: f32 = 0.08;

/// How fast the view's idea of up chases the ground's, per second.
///
/// A ninth of a second of lag. The prototype in `experimental/ow` uses 20 on
/// foot and 2.25 in flight; this sits nearer the walking end because there is
/// no flight mode here to serve, and because it is not the only filter in the
/// chain -- the body's own levelling in [`crate::player`] runs slower again,
/// and what reaches the screen is the two of them in series.
const UP_ALIGN_RATE: f32 = 9.0;

/// Past this much of an angle between the two ups, the gap is not something to
/// ease across.
///
/// Walking never opens one: on a planet a few hundred metres across, running
/// flat out turns up by a couple of degrees a second, and the rate above holds
/// the error well under one. What does open one is arriving somewhere else
/// entirely -- a respawn, a warp pipe, the far side of the planet -- and easing
/// across that is a second of the horizon rolling over for no reason.
const UP_SNAP: f32 = 0.5;

/// Reshapes a per-frame blend factor to cover `delta` seconds instead.
///
/// `camera.lerp(wanted, k)` run once a frame is not a rate: it is `k` per
/// *frame*, so the same `k` settles in half the wall-clock time at twice the
/// frame rate. That is fine while the frame rate never changes and stops being
/// fine the moment it does -- going fullscreen was enough to make a camera
/// tuned at 60 snap noticeably harder, without a line of the camera changing.
///
/// Each frame keeps `1 - k` of the distance still to cover, so `n` frames keep
/// `(1 - k)^n`. Feeding in how many sixtieths this frame actually lasted makes
/// the same number mean what it meant at 60fps and hold there at any rate.
fn blend(per_frame: f32, delta: f32) -> f32 {
    1.0 - (1.0 - per_frame.clamp(0.0, 1.0)).powf(delta * 60.0)
}

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
    gravity: Res<Gravity>,
    mut state: ResMut<GameState>,
    tuning: Res<GameTuning>,
) {
    let Ok((mut camera, mut follow)) = cameras.single_mut() else {
        return;
    };
    // Carried onto this frame's up before anything is measured in it. On the
    // castle `up` is `+Y`, the correction is the identity, and every line below
    // is the line that was there before.
    let up = gravity.up(player.translation);
    follow.frame = Quat::from_rotation_arc(follow.frame * Vec3::Y, up) * follow.frame;
    // And the view eases onto the frame. That is the whole of the lag, and
    // every axis below is measured in `view` rather than `frame`, so the camera
    // answers the ground through a filter while still answering the mouse
    // directly.
    let carried = follow.view * Vec3::Y;
    follow.view = if carried.angle_between(up) > UP_SNAP {
        follow.frame
    } else {
        let rate = gravity::settle(UP_ALIGN_RATE, time.delta_secs());
        follow.view.slerp(follow.frame, rate)
    };
    // Roll is the axis this matters most on. Taking the camera's up straight
    // off the ground puts every wobble in the surface onto the horizon, and a
    // tilting horizon is the most legible motion there is on a screen -- far
    // more so than the same wobble in pitch. Which is why `up` is not read
    // again below this line.
    let view_up = follow.view * Vec3::Y;
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
        // Behind the player means behind him *in the camera's own frame*, which
        // is what taking the facing back out of that frame asks.
        let forward = follow.view.inverse() * (player.rotation * Vec3::NEG_Z);
        follow.yaw = forward.x.atan2(forward.z);
    }
    state.aiming = input.aim;
    let desired_distance = if state.aiming {
        tuning.cam_aim_distance
    } else {
        tuning.cam_distance
    };
    let smoothing = blend(tuning.cam_smooth, time.delta_secs());
    follow.distance += (desired_distance - follow.distance) * blend(0.16, time.delta_secs());
    let focus = player.translation + view_up * tuning.cam_height;
    let orbit =
        follow.view * Quat::from_rotation_y(follow.yaw) * Quat::from_rotation_x(follow.pitch);
    let boom = orbit * Vec3::new(0.0, 0.8, follow.distance);

    // How much of the boom is free this frame, measured along the boom rather
    // than as a position, so it can be compared with the last frame's answer.
    let reach = match level.segment_hit(focus, focus + boom) {
        Some(hit) => ((hit - focus).length() - WALL_GAP).max(0.0) / boom.length().max(f32::EPSILON),
        None => 1.0,
    };
    // The two directions are deliberately not symmetric. Coming in has to be
    // immediate: the wall is between the player and the camera *now*, and any
    // easing there is a frame spent looking at the inside of it. Going back out
    // has no such deadline, and easing it is the whole point -- a wall the boom
    // clears and re-catches from one frame to the next, which is what a pillar
    // or a doorway does as the player walks past, otherwise throws the camera
    // its whole length out and back. That is the jump: not the frame rate, the
    // position, exactly as far as the boom is long.
    follow.clearance = if reach < follow.clearance {
        reach
    } else {
        follow.clearance + (reach - follow.clearance) * blend(REOPEN_RATE, time.delta_secs())
    };
    let wanted = focus + boom * follow.clearance;
    camera.translation = camera.translation.lerp(wanted, smoothing);
    camera.look_at(focus, view_up);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::LevelData;
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    const PLANET: f32 = 300.0;

    /// Everything [`update`] reads, with no renderer and no window: a bare
    /// planet with no collision in it, so the boom is never cut short and the
    /// only thing under test is which way the view thinks up is.
    fn planet_world(at: Vec3) -> World {
        let mut world = World::new();
        world.insert_resource(Gravity::towards(Vec3::ZERO));
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, PLANET));
        world.insert_resource(RenderPose {
            translation: at,
            rotation: Quat::IDENTITY,
        });
        world.insert_resource(InputState::default());
        world.insert_resource(GameState::default());
        world.insert_resource(GameTuning::default());
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.init_resource::<Time>();
        let aligned = Quat::from_rotation_arc(Vec3::Y, at.normalize());
        world.spawn((
            Transform::default(),
            FollowCamera {
                frame: aligned,
                view: aligned,
                ..default()
            },
        ));
        world
    }

    /// Advances the clock as well as the systems: the lag is a rate, so a
    /// frame of no elapsed time is a frame in which nothing eases.
    fn frames(world: &mut World, count: usize) {
        for _ in 0..count {
            world
                .resource_mut::<Time>()
                .advance_by(Duration::from_nanos(16_666_667));
            world.run_system_once(update).expect("update could not run");
        }
    }

    fn view_up(world: &mut World) -> Vec3 {
        let mut query = world.query::<&FollowCamera>();
        let follow = query.single(world).unwrap();
        follow.view * Vec3::Y
    }

    /// The point of the whole exercise: the ground moves and the view arrives
    /// afterwards. A step of walking is a fraction of the gap, not the gap.
    #[test]
    fn the_view_follows_the_ground_late_and_then_arrives() {
        let mut world = planet_world(Vec3::X * PLANET);
        // A sixth of a radian round the planet -- more than any single frame of
        // running covers, and well inside the distance that counts as walking.
        let moved = (Vec3::X * 0.986 + Vec3::Z * 0.166).normalize() * PLANET;
        world.resource_mut::<RenderPose>().translation = moved;
        let wanted = moved.normalize();
        frames(&mut world, 1);
        let after_one = view_up(&mut world).angle_between(wanted);
        assert!(after_one > 0.1, "arrived in a single frame");
        assert!(
            after_one < 0.166,
            "did not set off at all: {after_one} rad left of 0.166"
        );
        frames(&mut world, 30);
        assert!(
            view_up(&mut world).angle_between(wanted) < 0.01,
            "half a second later the view still has not caught up"
        );
    }

    /// And a gap that is not walking is not eased across. Respawning on the far
    /// side of a planet would otherwise spend a second rolling the horizon.
    #[test]
    fn arriving_somewhere_else_entirely_snaps_the_view() {
        let mut world = planet_world(Vec3::X * PLANET);
        world.resource_mut::<RenderPose>().translation = Vec3::NEG_Z * PLANET;
        frames(&mut world, 1);
        assert!(
            view_up(&mut world).angle_between(Vec3::NEG_Z) < 1e-4,
            "eased across a teleport"
        );
    }

    /// The castle is not paying for any of this. Up never moves there, so the
    /// frame, the view and the horizon are `+Y` on every frame as before.
    #[test]
    fn a_flat_level_keeps_the_view_upright_throughout() {
        let mut world = planet_world(Vec3::Y * PLANET);
        world.insert_resource(Gravity::default());
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, PLANET));
        world.resource_mut::<RenderPose>().translation = Vec3::new(-13.28, 3.0, 46.64);
        for _ in 0..60 {
            frames(&mut world, 1);
            let up = view_up(&mut world);
            assert!((up - Vec3::Y).length() < 1e-5, "the horizon tilted: {up}");
        }
    }

    /// One sixtieth of a second is the rate the tuning numbers were chosen at,
    /// so the reshaped factor has to leave them alone there.
    #[test]
    fn a_sixtieth_of_a_second_is_the_factor_unchanged() {
        for per_frame in [0.01, 0.16, 0.24, 0.5, 1.0] {
            let reshaped = blend(per_frame, 1.0 / 60.0);
            assert!(
                (reshaped - per_frame).abs() < 1e-5,
                "{per_frame} became {reshaped}"
            );
        }
    }

    /// The bug this exists to kill: the same camera settling different amounts
    /// in the same second because the frame rate changed under it. Going
    /// fullscreen is enough to do that.
    #[test]
    fn a_second_of_smoothing_is_a_second_at_any_frame_rate() {
        let settle = |rate: f32| {
            let mut remaining: f32 = 1.0;
            for _ in 0..rate as u32 {
                remaining *= 1.0 - blend(0.24, 1.0 / rate);
            }
            remaining
        };
        let at_60 = settle(60.0);
        for rate in [30.0, 75.0, 144.0, 240.0] {
            let there = settle(rate);
            assert!(
                (there - at_60).abs() < 1e-3,
                "a second at {rate}fps left {there} where 60fps left {at_60}"
            );
        }
        // And it is genuinely smoothing, not a factor that rounds to nothing.
        assert!(at_60 < 0.01, "a second at 0.24 should have all but arrived");
    }

    /// A wall cuts the boom short at once and lets it back out gradually. The
    /// asymmetry is what stops a pillar the player walks past from throwing the
    /// camera its whole length out and back between two frames.
    #[test]
    fn the_boom_gives_way_faster_than_it_returns() {
        let step = |clearance: f32, reach: f32| {
            if reach < clearance {
                reach
            } else {
                clearance + (reach - clearance) * blend(REOPEN_RATE, 1.0 / 60.0)
            }
        };
        assert_eq!(step(1.0, 0.3), 0.3, "a wall arriving takes effect at once");
        let reopened = step(0.3, 1.0);
        assert!(
            reopened > 0.3 && reopened < 0.4,
            "a wall clearing should ease out, not snap: {reopened}"
        );
    }
}
