//! Turning the Hero's upper body to face where he is aiming.
//!
//! This is the procedural layer `docs/aim.md` specifies and its "As Built"
//! section records as **not ported**: the design, the rig work and the tuned
//! numbers all survived the move from Panda3D, and the code that drove them
//! did not. The numbers below are lifted from that section verbatim, and they
//! are on console sliders for the reason the doc gives -- every one of them was
//! arrived at by moving a slider, not by reasoning, so the next person to
//! disagree with one needs the slider more than they need the constant.
//!
//! What it drives is a single bone. `tools/aim_rig.py` inserts `AIM_TORSO`
//! between the skeleton root and every joint above the hips, carrying no
//! keyframes of its own, so a rotation written here rides on top of whatever
//! clip is playing rather than fighting it. The thighs, pelvis, belt and sash
//! stay behind, which is what lets the chest turn while the legs run.
//!
//! The bone's rest rotation is identity and its parent is the scene root, so
//! its local axes *are* model space: +Y up, +Z forward, the same convention
//! `player::movement` turns the body in and `billboard::facing` aims quads in.
//! That is why the rotation below is composed in plain world-ish terms instead
//! of through some bone-local correction.
//!
//! Only the twist is here. `docs/aim.md` also wants the rotation distributed up
//! the spine, authored aim-offset poses and left-hand IK, and all three need
//! the Rigify DEF bones parented properly in `TheHero.blend` first -- see the
//! "As Built" note on why the exported skeleton is flat. This is the doc's own
//! "simple first implementation can use only AIM_TORSO".

use crate::{
    console::GameTuning,
    level::LevelData,
    player::{Controller, Player, FIXED_DT},
    weapon::Loadout,
};
use bevy::{
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    prelude::*,
    transform::TransformSystems,
};

/// The name `tools/aim_rig.py` gives the pivot it inserts.
///
/// By name, because that is the only thing that survives a glTF export -- the
/// same reason `billboard::claim` and the animation clips are looked up by
/// name.
const PIVOT: &str = "AIM_TORSO";

/// How far the aim ray is followed looking for something to converge on.
///
/// Only used to turn the camera's direction into a *point*, so that a shot
/// leaving the muzzle -- which is off to the side of the camera, in the Hero's
/// hand -- travels toward what the crosshair is on rather than off parallel to
/// it. Past this the ray has hit nothing and the point is simply far away,
/// which converges close enough that the difference is invisible.
const CONVERGE: f32 = 400.0;

/// Where the player is aiming, and how much of it the torso has taken up.
///
/// One resource rather than a component, because there is one player and every
/// reader of this wants the same answer: the weapon that fires along it, the
/// bone that turns toward it, and the feet that come round when it runs out of
/// twist.
#[derive(Resource, Debug, Clone, Copy)]
pub struct Aim {
    /// Unit vector the shot travels along, in world space.
    pub direction: Vec3,
    /// Where the camera's ray lands: the level surface it hits, or a point far
    /// down it if it hits nothing. Shots are aimed at this rather than along
    /// [`Self::direction`] so they converge on the crosshair.
    pub point: Vec3,
    /// The eye the ray was cast from, kept so a shot can tell whether the
    /// muzzle is already past what the camera was looking at.
    pub eye: Vec3,
    /// Smoothed torso twist, in radians, relative to the way the body faces.
    pub yaw: f32,
    /// Smoothed torso pitch, in radians. Positive is aiming up.
    pub pitch: f32,
    /// Yaw the torso was not allowed to take up, for the feet to come round
    /// by. Signed the same way as [`Self::yaw`].
    pub excess: f32,
    /// Spring velocities behind the two smoothed angles.
    yaw_rate: f32,
    pitch_rate: f32,
}

impl Default for Aim {
    fn default() -> Self {
        Self {
            direction: Vec3::Z,
            point: Vec3::ZERO,
            eye: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            excess: 0.0,
            yaw_rate: 0.0,
            pitch_rate: 0.0,
        }
    }
}

impl Aim {
    /// An aim looking `direction` and converging on `point`.
    ///
    /// The spring state stays private -- it is this module's working, and
    /// nothing outside has any business setting a torso's angular velocity --
    /// so this is how a caller builds one to ask a question of. Test support:
    /// the running game only ever gets its aim from [`drive`].
    #[cfg(test)]
    pub fn at(direction: Vec3, point: Vec3) -> Self {
        Self {
            direction,
            point,
            ..Self::default()
        }
    }
}

/// The pivot bone, claimed from the name the exporter gave it.
#[derive(Component)]
pub struct AimTorso;

/// Tags the pivot as the Hero's scene arrives.
pub fn claim(mut commands: Commands, arrivals: Query<(Entity, &Name), Added<Name>>) {
    for (entity, name) in &arrivals {
        if name.as_str() == PIVOT {
            commands.entity(entity).insert(AimTorso);
        }
    }
}

/// Wraps an angle into -pi..pi.
///
/// Every comparison here is between two headings, and without this a turn
/// across the back of the character reads as a turn the long way round: 179
/// degrees to -179 is two degrees of movement and looks like 358.
pub fn wrap(angle: f32) -> f32 {
    let turn = std::f32::consts::TAU;
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(turn);
    wrapped - std::f32::consts::PI
}

/// The heading of a direction, in the same terms `player::movement` turns the
/// body in.
pub fn heading(direction: Vec3) -> f32 {
    direction.x.atan2(direction.z)
}

/// One step of a critically damped spring, toward `target`.
///
/// Critically damped is what `docs/aim.md` asks for and it is the right choice
/// rather than an arbitrary one: an underdamped torso overshoots the target and
/// swings back, which on a gun reads as the shot wandering off the crosshair
/// after the stick has stopped moving. `response` is the time it takes to
/// substantially arrive, so it is a number that can be reasoned about on a
/// slider -- the doc's 0.12 s.
///
/// The rational approximation of the exponential is the standard one from Game
/// Programming Gems 4; it is stable at any step size, which matters because
/// this runs at the render rate and a stalled frame must not fling the torso.
pub fn spring(value: f32, rate: f32, target: f32, response: f32, dt: f32) -> (f32, f32) {
    if response <= 0.0 || dt <= 0.0 {
        return (target, 0.0);
    }
    let omega = 2.0 / response;
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = value - target;
    let temp = (rate + omega * change) * dt;
    (
        target + (change + temp) * decay,
        (rate - omega * temp) * decay,
    )
}

/// Reads the camera into [`Aim`], and turns the pivot toward it.
///
/// Runs per rendered frame rather than per fixed step because it is a pose:
/// the body it sits on is interpolated between fixed steps by
/// `player::sync_visual`, and a torso that snapped thirty times a second on top
/// of a body that glides is exactly the judder that system exists to avoid.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    time: Res<Time>,
    tuning: Res<GameTuning>,
    loadout: Res<Loadout>,
    level: Res<LevelData>,
    mut aim: ResMut<Aim>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    player: Query<&Transform, With<Player>>,
    mut pivot: Query<&mut Transform, (With<AimTorso>, Without<Player>)>,
) {
    let Ok(view) = camera.single() else {
        return;
    };
    let eye = view.translation();
    // The camera's own forward, which is where the crosshair is pointing --
    // `camera::update` finishes by looking at the player, so this ray runs from
    // the eye through him and out the far side.
    let direction = Vec3::from(view.forward()).normalize_or(Vec3::Z);
    aim.eye = eye;
    aim.direction = direction;
    aim.point = match level.surface_hit(eye, eye + direction * CONVERGE) {
        Some((hit, _)) => hit,
        None => eye + direction * CONVERGE,
    };

    let dt = time.delta_secs();
    // A holstered gun does not twist anything. The sword's clips carry their
    // own swing and the doc's melee layer is not ported, so with the blade out
    // this unwinds to neutral and leaves the animation alone.
    let armed = loadout.equipped.is_ranged();
    let (mut wanted_yaw, mut wanted_pitch) = (0.0, 0.0);
    if armed {
        if let Ok(body) = player.single() {
            let facing = body.rotation * Vec3::Z;
            wanted_yaw = wrap(heading(direction) - heading(facing));
            // A fraction of the shot's elevation rather than all of it: the
            // chest leans into a high shot, the arm does the rest. The doc's
            // 55%, and the reason the pivot alone does not look like a turret.
            wanted_pitch = direction.y.clamp(-1.0, 1.0).asin() * tuning.torso_pitch;
        }
    }
    let limit = tuning.torso_limit.to_radians();
    let clamped_yaw = wanted_yaw.clamp(-limit, limit);
    let clamped_pitch = wanted_pitch.clamp(
        -tuning.torso_pitch_down.to_radians(),
        tuning.torso_pitch_up.to_radians(),
    );
    (aim.yaw, aim.yaw_rate) = spring(
        aim.yaw,
        aim.yaw_rate,
        clamped_yaw,
        tuning.torso_response,
        dt,
    );
    (aim.pitch, aim.pitch_rate) = spring(
        aim.pitch,
        aim.pitch_rate,
        clamped_pitch,
        tuning.torso_response,
        dt,
    );
    // What the twist could not cover is what the feet owe. Recorded here and
    // spent in `turn_body`, which runs on the fixed step the body moves on.
    aim.excess = if armed { wanted_yaw - clamped_yaw } else { 0.0 };

    for mut transform in &mut pivot {
        // Positive pitch aims up, and a positive rotation about +X tips the
        // model's +Z forward *down*, so the pitch goes on negated.
        transform.rotation = Quat::from_rotation_y(aim.yaw) * Quat::from_rotation_x(-aim.pitch);
    }
}

/// Brings the feet round when the twist runs out, and settles him square when
/// he is standing still.
///
/// Two limits rather than one, which is the doc's point: sixty degrees is how
/// far the torso *can* go, twenty is how far it is comfortable staying. Past
/// sixty the body turns because it must; between twenty and sixty it turns only
/// when he is not otherwise busy, so a player strafing while aiming behind
/// himself keeps the twist and a player stood still slowly squares up to what
/// he is looking at.
///
/// Fixed step, and after `player::movement`, because the rotation it writes is
/// the simulation's own -- `sync_visual` interpolates it, and a body turned in
/// the render schedule would be turned again from the stale value next tick.
pub fn turn_body(
    tuning: Res<GameTuning>,
    loadout: Res<Loadout>,
    aim: Res<Aim>,
    mut player: Query<(&mut Transform, &Controller), With<Player>>,
) {
    if !loadout.equipped.is_ranged() {
        return;
    }
    let Ok((mut transform, ctrl)) = player.single_mut() else {
        return;
    };
    let moving = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z).length() > 0.25;
    // Standing still he settles back to the comfortable twist; running, only
    // the hard limit moves him, because the legs already have a direction and
    // the doc is explicit that locomotion owns them.
    let owed = if moving {
        aim.excess
    } else {
        let comfort = tuning.torso_comfort.to_radians();
        let total = aim.excess + aim.yaw;
        total - total.clamp(-comfort, comfort)
    };
    if owed.abs() < 1e-4 {
        return;
    }
    let step = tuning.torso_turn_rate.to_radians() * FIXED_DT;
    let turn = owed.clamp(-step, step);
    transform.rotation = Quat::from_rotation_y(heading(transform.rotation * Vec3::Z) + turn);
}

/// Claiming and driving, in the window where a pose may be written.
///
/// After the animation player, or the clip overwrites the pivot a moment later
/// -- `AIM_TORSO` carries no curves of its own, so today nothing would, but a
/// gun clip added later will and this must not have to be rediscovered. Before
/// the transforms are propagated, or the turn is a frame late and, worse,
/// everything hanging off the torso -- the arm, the hand, the gun in it -- is
/// drawn from a transform that never saw it. Exactly the reasoning in
/// `billboard::systems`, and the same two constraints.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (claim, drive)
        .chain()
        .after(bevy::animation::animate_targets)
        .before(TransformSystems::Propagate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_takes_the_short_way_round() {
        assert!((wrap(0.0)).abs() < 1e-6);
        // Just past the back, which is the case that goes wrong untreated.
        let short = wrap(179f32.to_radians() - (-179f32).to_radians());
        assert!(
            short.abs() < 3f32.to_radians(),
            "{} degrees the long way",
            short.to_degrees()
        );
        // Three half-turns is a half-turn, and -pi and pi are the same
        // heading -- so it is the magnitude that is pinned here, not the sign.
        assert!((wrap(std::f32::consts::PI * 3.0).abs() - std::f32::consts::PI).abs() < 1e-4);
        for angle in [-9.0, -3.0, 0.4, 3.0, 9.0, 100.0] {
            let wrapped = wrap(angle);
            assert!(
                wrapped.abs() <= std::f32::consts::PI + 1e-5,
                "{angle} wrapped to {wrapped}, outside -pi..pi"
            );
            // Same heading, just named once round.
            assert!((wrapped.sin() - angle.sin()).abs() < 1e-4);
            assert!((wrapped.cos() - angle.cos()).abs() < 1e-4);
        }
    }

    #[test]
    fn heading_matches_the_way_the_body_turns() {
        // `player::movement` turns to `wish.x.atan2(wish.z)`; a body rotated by
        // that heading must end up facing the direction it came from.
        for direction in [Vec3::Z, Vec3::X, Vec3::NEG_Z, Vec3::new(1.0, 0.0, -1.0)] {
            let direction = direction.normalize();
            let facing = Quat::from_rotation_y(heading(direction)) * Vec3::Z;
            assert!(
                facing.dot(direction) > 0.999,
                "{direction:?} came back as {facing:?}"
            );
        }
    }

    /// Critically damped: it arrives, and it does not come back past the
    /// target on the way. An overshoot here is the shot wandering off the
    /// crosshair after the stick has stopped.
    #[test]
    fn the_spring_settles_without_overshooting() {
        let (mut value, mut rate) = (0.0f32, 0.0f32);
        let target = 1.0;
        for _ in 0..240 {
            (value, rate) = spring(value, rate, target, 0.12, 1.0 / 60.0);
            assert!(value <= target + 1e-4, "overshot to {value}");
        }
        assert!((value - target).abs() < 1e-3, "settled at {value}");
    }

    /// And it substantially arrives within the response time it was given,
    /// which is the whole reason that number is a number a person can tune.
    #[test]
    fn the_spring_arrives_in_about_its_response_time() {
        let (mut value, mut rate) = (0.0f32, 0.0f32);
        let dt = 1.0 / 60.0;
        for _ in 0..(0.12 / dt) as usize {
            (value, rate) = spring(value, rate, 1.0, 0.12, dt);
        }
        assert!(value > 0.55, "only reached {value} in one response time");
    }

    /// A huge frame must not fling the torso past the target.
    #[test]
    fn the_spring_is_stable_across_a_stalled_frame() {
        let (value, _) = spring(0.0, 0.0, 1.0, 0.12, 5.0);
        assert!((0.0..=1.0 + 1e-4).contains(&value), "flung to {value}");
    }
}
