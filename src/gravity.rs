//! Which way is down.
//!
//! Every other module in this port used to answer that question with the
//! letter `Y`, and on a flat level that is the right answer: the castle's
//! collision is filed by `(x, z)` and its floors are the surfaces whose normal
//! leans towards `+Y`. A planet has no such axis. Standing on the far side of
//! one, `+Y` is *down*, and a jump made along it is a jump into the ground.
//!
//! So down becomes a resource rather than a constant. There are exactly two
//! shapes of it, because there are exactly two shapes of level: a flat one
//! where down is the same everywhere, and a round one where down is towards a
//! point. Everything that falls, stands, or turns to face where it is going
//! asks this rather than reaching for [`Vec3::Y`].
//!
//! It is deliberately not a force and not a physics engine. It hands back a
//! direction and a rate, and the movement code steps velocity by it exactly the
//! way it always did -- the change is which direction that step is taken along.

use bevy::prelude::*;

/// How fast anything not held up falls, in metres a second squared.
///
/// `app/main.py` stepped `-1.2` units of speed onto a body every frame at
/// 30 Hz, which is where this comes from; [`crate::pipe`]'s `LAUNCH_GRAVITY` is
/// the same number arrived at the same way, and the two are meant to stay
/// equal.
pub const FALL: f32 = 36.0;

/// Where down is, and how hard.
///
/// A resource rather than a component: every body in a level falls the same
/// way, and a level that wanted two gravities at once would want two centres,
/// which is a different feature and not this one.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub enum Gravity {
    /// A flat level. Down is `-Y` wherever you are standing.
    Down { accel: f32 },
    /// A planet. Down is towards `centre`, so "up" turns under your feet as you
    /// walk and the horizon closes rather than running out.
    Radial { centre: Vec3, accel: f32 },
}

impl Default for Gravity {
    fn default() -> Self {
        Self::Down { accel: FALL }
    }
}

impl Gravity {
    /// Gravity towards the middle of a planet centred at `centre`, at the same
    /// strength a flat level pulls with.
    ///
    /// The named constructor rather than the variant, because "make gravity
    /// point at the centre of the planet" is the thing being asked for and it
    /// should read that way at the call site.
    pub fn towards(centre: Vec3) -> Self {
        Self::Radial {
            centre,
            accel: FALL,
        }
    }

    /// Which way is up at `at`. Always unit length.
    ///
    /// The fallback matters more than it looks: exactly at a planet's centre
    /// there is no radial direction at all, and a zero vector handed to the
    /// movement code is a character with no floor, no facing and no jump.
    /// `+Y` is as good an answer as any there and is at least an answer.
    pub fn up(&self, at: Vec3) -> Vec3 {
        match *self {
            Gravity::Down { .. } => Vec3::Y,
            Gravity::Radial { centre, .. } => (at - centre).normalize_or(Vec3::Y),
        }
    }

    /// How fast the pull accelerates a body, in metres a second squared.
    pub fn accel(&self) -> f32 {
        match *self {
            Gravity::Down { accel } | Gravity::Radial { accel, .. } => accel,
        }
    }

    /// Splits a vector into how much of it runs along up at `at`, and what is
    /// left lying flat against the ground.
    ///
    /// Every piece of movement code that used to read `.y` and then treat
    /// `.x`/`.z` as the other half wants exactly this, and doing it by hand in
    /// each of them is how the two halves end up disagreeing about which is
    /// which.
    pub fn split(&self, vector: Vec3, at: Vec3) -> (f32, Vec3) {
        let up = self.up(at);
        let rise = vector.dot(up);
        (rise, vector - up * rise)
    }
}

/// Rewrites the component of `vector` that runs along `up`, leaving the rest
/// of it alone.
///
/// The counterpart to [`Gravity::split`]: `velocity.y = 0.0` on a flat level
/// is this with a zero, and on a planet it is the only way to say the same
/// thing without accidentally cancelling the run as well as the fall.
pub fn set_rise(vector: &mut Vec3, up: Vec3, rise: f32) {
    *vector += up * (rise - vector.dot(up));
}

/// Turns `direction` into the nearest direction lying flat against the ground
/// at `up`, or zero when it was pointing straight up or straight down.
pub fn flatten(direction: Vec3, up: Vec3) -> Vec3 {
    (direction - up * direction.dot(up)).normalize_or_zero()
}

/// How much of the remaining gap to close this step, for something easing
/// towards where the local down says it should be at `rate` per second.
///
/// Three things follow the local down and none of them should follow it
/// exactly: the body standing upright, the camera's idea of which way up is,
/// and the feet settling onto the floor. Reading the answer straight off the
/// ground puts every wobble in it on the screen in the frame it happens, which
/// is what makes a curved surface feel jerky to walk on -- there is nothing
/// between the geometry and the pixels.
///
/// A first-order ease is the thing between them. It keeps `exp(-rate * t)` of
/// its error after `t` seconds whatever the step length was, so `rate` is a
/// time constant of `1 / rate` seconds and says the same thing at 30 Hz, at
/// 60, and at 240. A plain "fraction per frame" does not: the same number
/// settles in half the wall-clock time at twice the frame rate, which is the
/// bug `camera::blend` exists to undo for the factors that were
/// already written that way.
pub fn settle(rate: f32, delta: f32) -> f32 {
    1.0 - (-rate * delta).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_gravity_is_the_one_the_port_already_had() {
        let gravity = Gravity::default();
        assert_eq!(gravity.up(Vec3::new(90.0, -4.0, 12.0)), Vec3::Y);
        assert_eq!(gravity.accel(), FALL);
    }

    /// The whole point of the exercise: on the far side of a planet, up is the
    /// opposite of what it is on the near side, and neither of them is `+Y`
    /// anywhere but the pole.
    #[test]
    fn a_planet_turns_up_around_underneath_you() {
        let gravity = Gravity::towards(Vec3::ZERO);
        assert_eq!(gravity.up(Vec3::new(0.0, 300.0, 0.0)), Vec3::Y);
        assert_eq!(gravity.up(Vec3::new(0.0, -300.0, 0.0)), Vec3::NEG_Y);
        assert_eq!(gravity.up(Vec3::new(300.0, 0.0, 0.0)), Vec3::X);
        // And up is away from the middle at every one of them, which is the
        // same statement as the pull being towards it.
        for at in [Vec3::Y * 300.0, Vec3::NEG_Y * 300.0, Vec3::X * 300.0] {
            assert!((gravity.up(at) - at.normalize()).length() < 1e-5, "{at}");
        }
    }

    #[test]
    fn the_centre_of_a_planet_still_has_an_up() {
        let gravity = Gravity::towards(Vec3::ZERO);
        assert_eq!(gravity.up(Vec3::ZERO), Vec3::Y, "a zero up is no up at all");
    }

    #[test]
    fn splitting_a_velocity_puts_it_back_together_again() {
        let gravity = Gravity::towards(Vec3::ZERO);
        let at = Vec3::new(200.0, 200.0, 100.0);
        let velocity = Vec3::new(3.0, -7.0, 2.5);
        let (rise, flat) = gravity.split(velocity, at);
        let up = gravity.up(at);
        assert!((flat.dot(up)).abs() < 1e-4, "the flat half still climbs");
        assert!((flat + up * rise - velocity).length() < 1e-4);
    }

    #[test]
    fn setting_the_rise_leaves_the_run_alone() {
        let up = Vec3::new(1.0, 1.0, 0.0).normalize();
        let mut velocity = Vec3::new(4.0, 0.0, 2.0);
        let before = velocity - up * velocity.dot(up);
        set_rise(&mut velocity, up, 0.0);
        assert!(velocity.dot(up).abs() < 1e-5);
        assert!((velocity - before).length() < 1e-5);
    }

    #[test]
    fn flattening_drops_the_part_that_points_at_the_sky() {
        let up = Vec3::Y;
        assert!((flatten(Vec3::new(0.0, 5.0, 3.0), up) - Vec3::Z).length() < 1e-5);
        assert_eq!(flatten(Vec3::Y, up), Vec3::ZERO);
    }

    /// The reason it is a rate and not a fraction: a second of easing has to be
    /// a second of easing whatever the frame rate is. Going fullscreen changes
    /// the frame rate.
    #[test]
    fn a_second_of_easing_is_a_second_at_any_step_length() {
        for rate in [4.0_f32, 8.0, 18.0] {
            let left = |steps: u32| {
                let delta = 1.0 / steps as f32;
                (0..steps).fold(1.0_f32, |gap, _| gap * (1.0 - settle(rate, delta)))
            };
            let reference = (-rate).exp();
            for steps in [30, 60, 144, 240] {
                let there = left(steps);
                assert!(
                    (there - reference).abs() < 1e-4,
                    "{rate}/s over {steps} steps left {there}, not {reference}"
                );
            }
        }
    }

    /// A rate of `r` has a time constant of `1 / r` seconds: after that long,
    /// `1 / e` of the gap is still open. That is what lets the tuning numbers
    /// be read as "about an eighth of a second" rather than guessed at.
    #[test]
    fn a_rate_is_the_reciprocal_of_its_time_constant() {
        let gap = 1.0 - settle(8.0, 1.0 / 8.0);
        assert!((gap - std::f32::consts::E.recip()).abs() < 1e-6, "{gap}");
    }

    #[test]
    fn easing_nowhere_in_no_time_moves_nothing() {
        assert_eq!(settle(9.0, 0.0), 0.0);
        assert_eq!(settle(0.0, 1.0), 0.0);
    }
}
