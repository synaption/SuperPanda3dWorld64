use crate::{
    console::GameTuning,
    gravity::Gravity,
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
        let forward = follow.frame.inverse() * (player.rotation * Vec3::NEG_Z);
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
    let focus = player.translation + up * tuning.cam_height;
    let orbit =
        follow.frame * Quat::from_rotation_y(follow.yaw) * Quat::from_rotation_x(follow.pitch);
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
    camera.look_at(focus, up);
}

#[cfg(test)]
mod tests {
    use super::*;

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
