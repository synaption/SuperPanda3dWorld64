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

/// How far above a planet's surface its pull keeps its full strength, and how
/// much further it takes to die away to nothing.
///
/// The numbers make the space between two planets genuinely weightless: the
/// system's worlds sit some five hundred metres apart surface to surface, so a
/// pull that is gone two hundred and forty metres up leaves a coasting zone in
/// the middle wider than either planet's grip.
///
/// A fade rather than an edge, because a body crossing a hard boundary at
/// speed picks up its whole weight in one tick, which reads as hitting
/// something invisible. `experimental/ow` faces the same choice and goes the
/// other way -- its pull turns *constant* beyond the falloff distance, because
/// its planets are meant to tug at you from across the system. These are not:
/// "leave the gravity of the planets and fly between them" is the feature, and
/// leaving means there is somewhere the gravity is not.
pub const GRAVITY_RANGE: f32 = 120.0;
pub const GRAVITY_FADE: f32 = 120.0;

/// Where down is, and how hard.
///
/// A resource rather than a component: every body in a level falls the same
/// way. What a level with two planets wants is not two gravities at once but
/// one answer built from two centres -- see [`Gravity::System`] -- because no
/// body is ever under both at full strength and the movement code still asks
/// one question.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub enum Gravity {
    /// A flat level. Down is `-Y` wherever you are standing.
    Down { accel: f32 },
    /// A planet. Down is towards `centre`, so "up" turns under your feet as you
    /// walk and the horizon closes rather than running out.
    Radial { centre: Vec3, accel: f32 },
    /// A solar system: planets, and the space between them. Down is towards
    /// the *nearest* centre -- the one change that keeps every surface level,
    /// which `experimental/ow`'s README works out the hard way: summing every
    /// body leaves the field tens of degrees off vertical and slides you off
    /// the world. The pull is full strength to [`GRAVITY_RANGE`] above the
    /// surface, gone [`GRAVITY_FADE`] later, and zero across the middle.
    ///
    /// One radius for every centre, because the system is the same planet
    /// twice. The day it is not, the radius moves into the array beside its
    /// centre.
    System {
        centres: [Vec3; 2],
        radius: f32,
        accel: f32,
    },
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

    /// Gravity for a pair of planets of the same `radius`: each pulls at the
    /// strength a flat level does, and between them there is none at all.
    pub fn binary(first: Vec3, second: Vec3, radius: f32) -> Self {
        Self::System {
            centres: [first, second],
            radius,
            accel: FALL,
        }
    }

    /// Moves a system's worlds without touching what they weigh. Called every
    /// tick by [`crate::orbit::advance`], because in the solar system the
    /// planets genuinely go somewhere; on any other gravity it is a no-op, so
    /// the caller does not have to know what kind of level is up.
    pub fn place_system(&mut self, at: [Vec3; 2]) {
        if let Gravity::System { centres, .. } = self {
            *centres = at;
        }
    }

    /// The centre this point answers to: the nearest one. With one radius for
    /// every centre, nearest centre and nearest surface are the same test.
    fn nearest(centres: &[Vec3; 2], at: Vec3) -> Vec3 {
        if (at - centres[0]).length_squared() <= (at - centres[1]).length_squared() {
            centres[0]
        } else {
            centres[1]
        }
    }

    /// Which way is up at `at`. Always unit length.
    ///
    /// The fallback matters more than it looks: exactly at a planet's centre
    /// there is no radial direction at all, and a zero vector handed to the
    /// movement code is a character with no floor, no facing and no jump.
    /// `+Y` is as good an answer as any there and is at least an answer.
    ///
    /// A system keeps answering out in the weightless middle, and that is
    /// deliberate: nothing *falls* along the answer there -- [`Self::strength`]
    /// is zero -- but the body still stands along it and the camera still
    /// rights itself by it, so a flyer arrives at the far planet the right way
    /// up instead of at whatever angle the last surface left him.
    pub fn up(&self, at: Vec3) -> Vec3 {
        match *self {
            Gravity::Down { .. } => Vec3::Y,
            Gravity::Radial { centre, .. } => (at - centre).normalize_or(Vec3::Y),
            Gravity::System { ref centres, .. } => {
                (at - Self::nearest(centres, at)).normalize_or(Vec3::Y)
            }
        }
    }

    /// How fast the pull accelerates a body, in metres a second squared,
    /// ignoring where the body is. The number the level was tuned with:
    /// everything that launches, lobs or lets fall near the ground wants this.
    /// The thing actually falling wants [`Self::strength`].
    pub fn accel(&self) -> f32 {
        match *self {
            Gravity::Down { accel }
            | Gravity::Radial { accel, .. }
            | Gravity::System { accel, .. } => accel,
        }
    }

    /// The pull at `at`, in metres a second squared: [`Self::accel`] with the
    /// altitude counted. On a flat level and a lone planet the two are the
    /// same number everywhere; in a system the pull fades with height and the
    /// middle is weightless.
    pub fn strength(&self, at: Vec3) -> f32 {
        match *self {
            Gravity::Down { accel } | Gravity::Radial { accel, .. } => accel,
            Gravity::System {
                ref centres,
                radius,
                accel,
            } => {
                let altitude = (at - Self::nearest(centres, at)).length() - radius;
                let faded = ((altitude - GRAVITY_RANGE) / GRAVITY_FADE).clamp(0.0, 1.0);
                accel * (1.0 - faded)
            }
        }
    }

    /// Whether a body at `at` is out from under every planet's pull: the coast
    /// between worlds, where the movement code flies instead of falling.
    pub fn weightless(&self, at: Vec3) -> bool {
        self.strength(at) <= 0.0
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

    /// The one change that makes a two-planet system walkable: each surface
    /// answers to its own centre and no other. A summed field -- what
    /// `experimental/ow` ports and then has to switch off -- would lean every
    /// up towards the other world.
    #[test]
    fn each_planet_in_a_system_owns_its_own_down() {
        let gravity = Gravity::binary(Vec3::ZERO, Vec3::X * 1100.0, 300.0);
        assert_eq!(gravity.up(Vec3::new(0.0, 300.0, 0.0)), Vec3::Y);
        assert_eq!(gravity.up(Vec3::new(1100.0, 300.0, 0.0)), Vec3::Y);
        // On the facing sides, up points at the *other* planet's sky, not away
        // from some blended middle.
        assert_eq!(gravity.up(Vec3::X * 300.0), Vec3::X);
        assert_eq!(gravity.up(Vec3::X * 800.0), Vec3::NEG_X);
    }

    /// Leaving is the feature: full weight on the ground, nothing at all in
    /// the middle, and a fade rather than an edge between the two.
    #[test]
    fn the_space_between_the_planets_is_weightless() {
        let gravity = Gravity::binary(Vec3::ZERO, Vec3::X * 1100.0, 300.0);
        assert_eq!(gravity.strength(Vec3::X * 300.0), FALL);
        assert_eq!(gravity.strength(Vec3::X * (300.0 + GRAVITY_RANGE)), FALL);
        let far = 300.0 + GRAVITY_RANGE + GRAVITY_FADE;
        assert_eq!(gravity.strength(Vec3::X * far), 0.0);
        assert!(gravity.weightless(Vec3::X * 550.0), "the middle still pulls");
        assert!(!gravity.weightless(Vec3::X * 310.0));
        // Halfway through the fade is half the pull: continuous, not a cliff.
        let half = gravity.strength(Vec3::X * (300.0 + GRAVITY_RANGE + GRAVITY_FADE * 0.5));
        assert!((half - FALL * 0.5).abs() < 1e-3, "{half}");
        // And the flat level is untouched by any of this.
        assert_eq!(Gravity::default().strength(Vec3::Y * 5000.0), FALL);
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
