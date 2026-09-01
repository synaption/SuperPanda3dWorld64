//! A day and a night over the castle grounds.
//!
//! Everything the game draws is lit out of one small resource,
//! [`N64Lighting`], and until now every number in it was a constant. A sun that
//! moves is that resource written every few frames, which is the cheap half of
//! this module: [`crate::n64::relight`] already existed to carry a changed
//! light out to a world full of surfaces, and named the case "moving the sun"
//! before there was one.
//!
//! It gained a term on the way. Three of the four -- key, ambient, direction --
//! only reach a surface this renderer lights, and the castle is not one: its
//! shading was baked into its vertex colours before the game ran, as were the
//! impostor sheets. Both were made under a noon sun, so
//! [`N64Lighting::daylight`] is how much of that light is left, and it is what
//! stops a night falling on the actors alone while they walk about on grass
//! that is still noon-bright.
//!
//! The expensive half is that a moving sun has to be *visible*, and the game
//! had no sky at all. The horizon was one flat fog colour, `water::SKY_COLOUR`,
//! painted over the far distance by the camera's fog and behind everything by
//! the clear colour. So there are four pieces of geometry here, each a shell
//! about the camera, drawn from nearest out:
//!
//!   * the **sun** and the **moon**, quads turned to face the camera, on
//!     opposite ends of the same line through the sky;
//!   * the **stars**, four hundred quads scattered over a sphere that turns
//!     with the sun, faded in as the sun goes down;
//!   * the **dome**, a sphere whose vertex colours are rewritten as the light
//!     changes: a gradient from the horizon up to the zenith, with a warm band
//!     burnt into it along the sun's own bearing.
//!
//! All four are drawn six hundred-odd units out, which is three times past
//! where the camera's haze becomes total, so all four are marked
//! [`N64Uniform::beyond_the_fog`] -- see that field for why a fogged sky is a
//! flat grey screen. What keeps the seam invisible instead is the other
//! direction: the fog and the clear colour are repainted every frame with the
//! dome's own horizon colour, so the world fades into exactly the sky it is
//! standing under. That is also what makes a sunset reach the ground -- the
//! whole far half of the level goes pink because the fog it is dissolving into
//! is pink.
//!
//! **Two skies, and one level with none.** The shells are put up once and
//! follow the camera for the rest of the run. Over the castle the sky is a
//! clock: up is `+Y` everywhere and the sun runs round [`ORBIT_AXIS`] once a
//! day. Over the orbiting planet the same four shells are hung in a different
//! frame ([`Overhead`]): up is whichever way the core is not, so the dome is
//! turned to put its horizon on the player's own horizon, and the sun is where
//! [`Sky::orbit_sun`] says -- the planet's spin carrying it round the poles
//! once a day, and the planet's orbit drifting it through the star field
//! across the year, with the axial tilt swinging noon up and down the sky as
//! the seasons turn. The plain planet level keeps the fixed noon it always
//! had: [`advance`] puts the constant light back and hides all four shells
//! there.

use crate::{
    console::GameTuning,
    gravity::Gravity,
    n64::{N64Lighting, N64Material, N64Uniform, Shading},
    water::{self, CameraMedium},
    world::LevelId,
};
use bevy::{
    asset::RenderAssetUsages,
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    mesh::{Indices, PrimitiveTopology},
    pbr::DistanceFog,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// How far out each shell is drawn.
///
/// Ordered rather than arbitrary, and the order is the whole reason there are
/// three numbers: the dome is opaque and writes depth, so anything meant to be
/// seen against it has to be inside it. All three are well under the camera's
/// far plane of 1000, which is the only other constraint -- a sky past that is
/// a sky clipped away.
const DOME_RADIUS: f32 = 620.0;
const STAR_RADIUS: f32 = 560.0;
const BODY_RADIUS: f32 = 500.0;

/// How finely the dome is divided: rings from pole to pole, segments around.
///
/// Sized by the sharpest thing painted onto it rather than by how round it
/// looks. The vertical gradient would be happy with a quarter of this -- it is
/// smooth over tens of degrees, and Gouraud interpolation of a smooth function
/// is the function. The glow round the sun is not: it falls off over about
/// fifteen degrees, and at eighteen rings by thirty-two the sky either side of
/// a sunrise came out as a visible hexagon. Six degrees a step is fine enough
/// that the halo lands on several vertices on the way down.
///
/// The cost of the finer mesh is 2,145 vertices of arithmetic a few times a
/// second, which is nothing, and 2,145 vertices of geometry every frame, which
/// is less than one of the enemies standing about on the lawn.
const DOME_RINGS: u32 = 32;
const DOME_SEGMENTS: u32 = 64;

/// How many stars, and how wide each one is drawn at [`STAR_RADIUS`].
///
/// A star is a quad rather than a texel of a sky texture, and the difference is
/// resolution: an equirectangular star map fine enough to give a *point* of
/// light would be four thousand pixels across, and one small enough to build in
/// a loop gives stars a degree and a half wide -- fifteen-pixel squares. A quad
/// is sized in world units instead, so a star is as small as it wants to be.
const STAR_COUNT: usize = 420;
const STAR_SIZE: (f32, f32) = (1.7, 4.4);

/// The seed the star field is scattered from.
///
/// Fixed, so the sky is the same sky every run. The same reasoning as the phase
/// offsets in `world::spawn_inhabitants`: a screenshot that has to be compared
/// against another screenshot cannot have the scenery reshuffled underneath it.
const STAR_SEED: u32 = 0x5EED_5217;

/// How wide the sun and the moon are drawn, at [`BODY_RADIUS`].
///
/// Both far larger than the half-degree the real ones subtend, which is eleven
/// pixels at this game's internal resolution and reads as a stuck bright texel.
/// These are about five degrees across, which is the size a sun is *remembered*
/// as being. Only part of each quad is the body and the rest is the halo round
/// it, and the two split it differently: the sun is a small core in a wide
/// bloom and the moon is a wide disc with a thin one, so the moon reads as the
/// larger object by more than the difference in these two numbers.
const SUN_SIZE: f32 = 44.0;
const MOON_SIZE: f32 = 58.0;

/// How much wider the sun is drawn sitting on the horizon than overhead.
///
/// The real one does this too, and for a reason this renderer has no way to
/// reproduce honestly -- refraction through a long slant of atmosphere -- so it
/// is simply asserted. Without it the sunset is a small hard dot in a very
/// large orange sky.
const SUN_SWELL: f32 = 0.7;

/// The axis the sun and the stars turn about, and where the sun starts.
///
/// The sun runs round a circle whose pole is [`ORBIT_AXIS`], starting from
/// [`SUNRISE`] due east and climbing. The axis is tilted off the vertical so
/// that noon lands short of the zenith: the sun's height at noon is the axis's
/// own `z`, and 0.87 of the way up is 60 degrees -- which is, not by accident,
/// exactly the elevation the game's fixed key light had before it started
/// moving. See [`RAMP`], whose last stop is the light this replaced.
///
/// The two are perpendicular, which is what makes the rotation give a unit
/// direction back rather than a shortened one.
const ORBIT_AXIS: Vec3 = Vec3::new(0.0, 0.494, 0.87);
const SUNRISE: Vec3 = Vec3::X;

/// Which hour of the clock the sun crosses [`SUNRISE`] on, and how long the
/// whole circuit is in hours. Six and twenty-four, so the numbers on the
/// console slider are the numbers on a clock.
const DAWN_HOUR: f32 = 6.0;
const HOURS: f32 = 24.0;

/// How long the orbiting planet's year is, in its own days, and how far its
/// spin axis leans out of its orbit.
///
/// Eight days to the year rather than hundreds, because an orbit is only
/// visible in what it changes -- the sun drifting through the constellations,
/// the seasons swinging noon up and down the sky -- and at 365 days to a
/// five-minute day a single season would take a working week of play to turn.
/// Eight puts a whole year, solstice to solstice and back, inside three
/// quarters of an hour.
///
/// The tilt is Earth's, and it is what makes the orbit *show*. With the axis
/// square to the orbital plane the sun would ride the same circle every day of
/// the year, and the only witness to the year would be the stars behind it.
const YEAR_DAYS: f32 = 8.0;
const AXIAL_TILT: f32 = 23.4_f32.to_radians();

/// How far the sun has to move before the world is relit and the dome
/// recoloured.
///
/// Not every frame, and this is the one number in the module that is a real
/// trade rather than a look. Relighting means rewriting the uniform of *every*
/// material in the game -- the castle alone has forty-five -- because this
/// renderer keeps its light in each material rather than in a bind group of its
/// own. At a five-minute day the sun crosses a quarter of a degree about five
/// times a second, and a quarter of a degree of sun is a change of at most one
/// step in an eight-bit colour channel: below what the frame buffer can show
/// and far below what a vertex-lit scene can. So the sky is continuous and the
/// *light* is not, and nothing can tell.
///
/// The sun and moon discs are exempt -- they are transforms, not uniforms, and
/// they move every frame. A disc that stepped would be obvious.
const RELIGHT_STEP: f32 = 0.25_f32.to_radians();

/// How tightly the warm glow gathers about the sun, and how far the same warmth
/// spreads along the horizon under it.
///
/// Two terms rather than one because a sunset is two things: a small fierce
/// halo round the disc, which is the first, and a wide band lying along the
/// horizon either side of it, which is the second. A single power of
/// `dot(direction, sun)` gives one or the other and never both.
///
/// Both are tighter than they look like they want to be, and the reason is the
/// disc. The sky at the sun's own bearing reaches [`Look::glow`] exactly, and
/// the disc is drawn over it with ordinary blending -- so a glow that spreads
/// until it is near white leaves the sun nothing to be brighter *than*, and it
/// reads as a hole punched in the sunset rather than as the thing causing it.
///
/// Neither is tight enough to be the bloom right at the sun's edge, and that is
/// on purpose too. This one is evaluated per *vertex* and interpolated across
/// several degrees of dome; anything sharper than the mesh facets. The sharp
/// half of the halo is painted into [`sun_disc`] instead, where it is a
/// texture and is as sharp as it likes.
const HALO_FOCUS: f32 = 10.0;
const BAND_FOCUS: f32 = 3.0;
const BAND_FLATNESS: f32 = 3.0;
const BAND_STRENGTH: f32 = 0.60;

/// How high the sun has to be for its disc to be drawn at all.
///
/// The level is eighty units across and the sun is five hundred out, so there
/// is no horizon between the two: past the edge of the castle grounds what is
/// behind the sun is the dome, and a sun that simply kept going would sit in
/// the middle of a midnight sky. So it is faded out across a hand's width of
/// elevation either side of the horizon instead, which reads as setting into
/// the haze -- helped by the disc reddening and swelling as it gets there.
const HORIZON_FADE: (f32, f32) = (-0.075, 0.03);

/// How much of the moon is shown in daylight.
///
/// Not zero: a moon in a blue sky is a real thing and a nice one. Not one
/// either, because at full strength it reads as a hole in the sky rather than
/// as the moon.
const DAYTIME_MOON: f32 = 0.22;

/// One entry in [`RAMP`]: what the whole sky looks like with the sun at a given
/// height.
///
/// Keyed on the sun's *elevation* -- the `y` of the unit vector pointing at it
/// -- rather than on the hour, because elevation is what the look actually
/// depends on. Twilight is not a time, it is the sun being just under the
/// horizon, and keying on it means the table stays right if the orbit is
/// re-tilted or the day made longer.
///
/// The colours are sRGB, the way every other colour in this game is written and
/// the way a person can read them. They are converted to linear on the way into
/// a vertex; the two light terms below are *not* colours in that sense but
/// multipliers against an already-linear texture sample, so those are left
/// alone. See [`Look::at`].
#[derive(Clone, Copy, Debug)]
struct Look {
    /// Straight overhead.
    zenith: Vec3,
    /// The horizon ring, away from the sun. Also the fog and the clear colour:
    /// what the world dissolves into is what is behind the world.
    ///
    /// Deliberately cool even at sunset. The warm belongs to the sun's own
    /// bearing and [`glow`](Self::glow) is what puts it there; painted into the
    /// base instead it goes all the way round, so the sky is pink in the east
    /// while the sun is setting in the west.
    horizon: Vec3,
    /// Under the horizon. Seen past the edge of the level and nowhere else,
    /// so it is a haze rather than a ground: what the far distance would look
    /// like if the far distance kept going.
    ground: Vec3,
    /// The warm the sky takes on along the sun's own bearing.
    glow: Vec3,
    /// The sun's disc.
    disc: Vec3,
    /// [`N64Lighting::key`] and [`N64Lighting::ambient`], as multipliers.
    key: Vec3,
    ambient: Vec3,
    /// How much of the star field shows through.
    stars: f32,
}

impl Look {
    fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            zenith: self.zenith.lerp(other.zenith, t),
            horizon: self.horizon.lerp(other.horizon, t),
            ground: self.ground.lerp(other.ground, t),
            glow: self.glow.lerp(other.glow, t),
            disc: self.disc.lerp(other.disc, t),
            key: self.key.lerp(other.key, t),
            ambient: self.ambient.lerp(other.ambient, t),
            stars: self.stars + (other.stars - self.stars) * t,
        }
    }

    /// The sky with the sun at `elevation`, which is `sun.y` and therefore
    /// runs -1 to 1. Off either end of the table is that end of the table:
    /// deeper than midnight is midnight and higher than noon is noon.
    fn at(elevation: f32) -> Self {
        let last = RAMP.len() - 1;
        if elevation <= RAMP[0].0 {
            return RAMP[0].1;
        }
        for pair in RAMP.windows(2) {
            let ((low, below), (high, above)) = (pair[0], pair[1]);
            if elevation <= high {
                return below.lerp(above, (elevation - low) / (high - low));
            }
        }
        RAMP[last].1
    }
}

/// The whole look of the sky, as a handful of stops the sun slides between.
///
/// The stops are crowded round the horizon and sparse away from it because
/// that is where the change is: the sky is very nearly the same at noon as at
/// four in the afternoon and completely different at six as at half past. The
/// last stop is the sky this game had before it had a sky -- its horizon is
/// `water::SKY_COLOUR` and its two light terms are the constants
/// [`N64Lighting::default`] shipped with -- so the middle of the day still
/// looks exactly the way the rest of the game was tuned against, and a test
/// below holds that.
const RAMP: [(f32, Look); 8] = [
    // Deep night. Dark enough that the stars are the brightest thing in the
    // sky, which is the whole point of it being dark.
    (
        -1.0,
        Look {
            zenith: Vec3::new(0.015, 0.025, 0.070),
            horizon: Vec3::new(0.050, 0.070, 0.135),
            ground: Vec3::new(0.020, 0.025, 0.050),
            glow: Vec3::new(0.050, 0.070, 0.135),
            disc: Vec3::new(1.000, 0.620, 0.350),
            key: Vec3::new(0.028, 0.034, 0.054),
            ambient: Vec3::new(0.026, 0.033, 0.058),
            stars: 1.0,
        },
    ),
    // The sun well under the horizon: the last of the blue gone, no colour
    // left in the west, and the stars nearly all out.
    (
        -0.22,
        Look {
            zenith: Vec3::new(0.050, 0.080, 0.185),
            horizon: Vec3::new(0.120, 0.125, 0.245),
            ground: Vec3::new(0.045, 0.050, 0.100),
            glow: Vec3::new(0.230, 0.140, 0.230),
            disc: Vec3::new(1.000, 0.620, 0.350),
            key: Vec3::new(0.038, 0.044, 0.066),
            ambient: Vec3::new(0.036, 0.040, 0.062),
            stars: 0.85,
        },
    ),
    // Civil twilight: the sun just under, the sky still lit from below, and
    // the brighter stars showing.
    (
        -0.055,
        Look {
            zenith: Vec3::new(0.120, 0.185, 0.360),
            horizon: Vec3::new(0.260, 0.270, 0.400),
            ground: Vec3::new(0.105, 0.105, 0.160),
            glow: Vec3::new(0.780, 0.340, 0.190),
            disc: Vec3::new(1.000, 0.640, 0.380),
            key: Vec3::new(0.085, 0.070, 0.075),
            ambient: Vec3::new(0.075, 0.075, 0.100),
            stars: 0.30,
        },
    ),
    // Sunrise and sunset proper, the sun sitting on the horizon. A quarter of
    // the light of noon: a sun at this angle is shining along the ground
    // rather than onto it, and most of what reaches a surface is sky.
    (
        0.05,
        Look {
            zenith: Vec3::new(0.200, 0.340, 0.610),
            horizon: Vec3::new(0.460, 0.500, 0.640),
            ground: Vec3::new(0.220, 0.205, 0.240),
            glow: Vec3::new(1.000, 0.560, 0.250),
            disc: Vec3::new(1.000, 0.720, 0.450),
            key: Vec3::new(0.290, 0.190, 0.110),
            ambient: Vec3::new(0.115, 0.105, 0.115),
            stars: 0.0,
        },
    ),
    // Ten degrees up, half an hour in: the sky has gone blue again and the
    // light is still low and yellow. This stop is what stops the ground
    // snapping to full daylight the moment the sun clears the horizon.
    (
        0.14,
        Look {
            zenith: Vec3::new(0.230, 0.415, 0.700),
            horizon: Vec3::new(0.520, 0.590, 0.760),
            ground: Vec3::new(0.290, 0.290, 0.330),
            glow: Vec3::new(1.000, 0.700, 0.420),
            disc: Vec3::new(1.000, 0.840, 0.620),
            key: Vec3::new(0.540, 0.440, 0.340),
            ambient: Vec3::new(0.180, 0.180, 0.210),
            stars: 0.0,
        },
    ),
    // An hour or so up: the warmth is off the sky and nearly off the light.
    (
        0.26,
        Look {
            zenith: Vec3::new(0.255, 0.475, 0.780),
            horizon: Vec3::new(0.560, 0.660, 0.850),
            ground: Vec3::new(0.340, 0.360, 0.400),
            glow: Vec3::new(0.980, 0.820, 0.580),
            disc: Vec3::new(1.000, 0.930, 0.780),
            key: Vec3::new(0.600, 0.600, 0.520),
            ambient: Vec3::new(0.270, 0.280, 0.330),
            stars: 0.0,
        },
    ),
    // Mid morning.
    (
        0.55,
        Look {
            zenith: Vec3::new(0.215, 0.455, 0.845),
            horizon: Vec3::new(0.400, 0.630, 0.870),
            ground: Vec3::new(0.400, 0.450, 0.510),
            glow: Vec3::new(0.860, 0.890, 0.950),
            disc: Vec3::new(1.000, 0.960, 0.860),
            key: Vec3::new(0.695, 0.645, 0.530),
            ambient: Vec3::new(0.405, 0.425, 0.485),
            stars: 0.0,
        },
    ),
    // Noon -- the sky this game had before it had one. Held by
    // `noon_is_the_light_the_game_was_tuned_with`.
    (
        0.87,
        Look {
            zenith: Vec3::new(0.190, 0.430, 0.840),
            horizon: Vec3::new(0.320, 0.600, 0.860),
            ground: Vec3::new(0.420, 0.480, 0.540),
            glow: Vec3::new(0.760, 0.850, 0.950),
            disc: Vec3::new(1.000, 0.980, 0.920),
            key: Vec3::new(0.680, 0.660, 0.580),
            ambient: Vec3::new(0.420, 0.440, 0.500),
            stars: 0.0,
        },
    ),
];

/// The clock, and what was last pushed out of it.
///
/// The hour itself lives here rather than in [`GameTuning`] because it is
/// state that advances and the tuning table is a table of settings -- but the
/// two are kept in step, so `sky_hour` on the console reads the clock and
/// writing it sets the clock. See [`advance`], which does the handshake.
#[derive(Resource, Debug)]
pub struct Sky {
    /// The hour of the day, 0 to 24.
    pub hours: f32,
    /// How far through the year the orbiting planet is, in days, 0 to
    /// [`YEAR_DAYS`]. Advanced by the same clock as `hours` while that level
    /// is up, and left where it was everywhere else: the year is that
    /// planet's year, and it does not pass while nobody is on it.
    pub year: f32,
    /// The hour last written into `GameTuning::sky_hour`, so a value that
    /// differs from it can only have come from the player.
    published: f32,
    /// Where the sun last stood in the local sky -- [`Overhead::apparent`] --
    /// when the world was last relit and the dome last recoloured. See
    /// [`RELIGHT_STEP`]. The apparent sun rather than the world one, because
    /// on the planet the sun can stand still in the world while the player
    /// walks the horizon out from under it, and elevation over *that* horizon
    /// is what every colour in the [`RAMP`] is keyed on.
    lit_from: Option<Vec3>,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            // Mid morning: the sun up and clearly to one side, which is what
            // every screenshot of this game has been taken in and what the
            // impostor sheets were baked under.
            hours: GameTuning::default().sky_hour,
            // The year opens at a solstice, so the first season a player sees
            // is a season and not the year's most average day.
            year: 0.0,
            // Agreeing with `hours` from the start, so the first frame reads
            // the slider as untouched and advances the clock rather than
            // taking a scrub nobody made.
            published: GameTuning::default().sky_hour,
            // Nothing has been lit yet, which is what makes the first frame
            // light the world whatever the step below would have said.
            lit_from: None,
        }
    }
}

impl Sky {
    /// Where the sun is over the castle, as a unit vector pointing at it from
    /// the ground.
    pub fn sun(&self) -> Vec3 {
        self.spin() * SUNRISE
    }

    /// The turn the whole celestial sphere has made since dawn. The sun is a
    /// point on it and the star field is the rest of it, which is what makes
    /// the stars wheel with the sun rather than merely fade in under it.
    fn spin(&self) -> Quat {
        let turned = (self.hours - DAWN_HOUR) / HOURS * std::f32::consts::TAU;
        Quat::from_axis_angle(ORBIT_AXIS.normalize(), turned)
    }

    /// Where the sun is from the orbiting planet, as a unit vector in the
    /// planet's own frame -- the frame its mesh, its collision and everyone
    /// standing on it all live in.
    ///
    /// Built from the outside in. In the orbit's frame the sun is a point on
    /// a circle the year moves round; the axial tilt then leans that whole
    /// frame off the planet's equator, which is where the seasons come from;
    /// and the day's spin carries it round the poles. The spin is about `Y`
    /// and not [`ORBIT_AXIS`]: the planet's poles are its mesh's poles, and
    /// the tilt has already been applied by the middle term.
    pub fn orbit_sun(&self) -> Vec3 {
        let along = self.year / YEAR_DAYS * std::f32::consts::TAU;
        let toward = Vec3::new(along.cos(), 0.0, along.sin());
        self.orbit_spin() * (Quat::from_rotation_z(-AXIAL_TILT) * toward)
    }

    /// The turn the day's spin has put on the orbiting planet's sky: about
    /// the poles, backwards -- the planet turns one way, so everything over
    /// it appears to go the other -- once every [`HOURS`].
    fn orbit_spin(&self) -> Quat {
        let turned = (self.hours - DAWN_HOUR) / HOURS * std::f32::consts::TAU;
        Quat::from_rotation_y(-turned)
    }

    /// The turn the star field has made over the orbiting planet. The tilt is
    /// in it and the year is not: the stars sit in the same frame as the sun,
    /// but they are infinitely far away, so the orbit that moves the sun
    /// round its circle moves them not at all. That difference is the orbit's
    /// one signature in the sky -- the sun drifts through the constellations
    /// and comes back round in exactly a year.
    fn orbit_stars(&self) -> Quat {
        self.orbit_spin() * Quat::from_rotation_z(-AXIAL_TILT)
    }
}

/// Which sky a level stands under.
///
/// The castle has the clock sky this module began as; the orbiting planet has
/// the same four shells hung in its own frame; and the plain planet has none,
/// keeping the fixed noon the rest of the game was tuned against. One place to
/// answer the question rather than `==` tests scattered through two systems,
/// so a fourth level has one line to add.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SkyKind {
    /// The castle's: up is `+Y` everywhere and the sun keeps the hours its
    /// slider is labelled with.
    Clock,
    /// The orbiting planet's: up is local, the spin is the day, and the orbit
    /// is the year.
    Orbit,
    /// No sky at all -- the constant noon light, and all four shells hidden.
    None,
}

fn sky_of(level: LevelId) -> SkyKind {
    match level {
        LevelId::Castle => SkyKind::Clock,
        LevelId::PlanetOrbit => SkyKind::Orbit,
        LevelId::Planet => SkyKind::None,
    }
}

/// Where the sky stands over one particular pair of feet: everything
/// [`advance`] and [`follow`] both need, computed from the same inputs in each
/// so the world's light and the picture of the sky can never disagree.
struct Overhead {
    /// The direction to the sun in the world's frame, which is what the
    /// meshes are lit from and where the disc is hung.
    sun: Vec3,
    /// The turn carrying the sky's own `+Y` onto the up under the camera.
    /// Identity over the castle; over the planet the dome wears it, which is
    /// what lays the dome's horizon along the player's own.
    frame: Quat,
    /// The sun seen through `frame`, whose `y` is its elevation over the
    /// player's horizon: the number the whole [`RAMP`] is keyed on. Over the
    /// castle it *is* `sun`.
    apparent: Vec3,
    /// The turn the star field has made.
    stars: Quat,
}

impl Overhead {
    /// The moon: opposite the sun, which makes it a full moon that rises as
    /// the sun sets -- the one phase worth having when there is only going to
    /// be one.
    fn moon(&self) -> Vec3 {
        -self.sun
    }
}

/// The sky over an `up`. [`SkyKind::None`] never gets here -- both callers
/// have returned before asking -- so the castle's answer stands in for it
/// harmlessly.
fn overhead(kind: SkyKind, sky: &Sky, up: Vec3) -> Overhead {
    match kind {
        SkyKind::Orbit => {
            let sun = sky.orbit_sun();
            let frame = Quat::from_rotation_arc(Vec3::Y, up);
            Overhead {
                sun,
                frame,
                apparent: frame.inverse() * sun,
                stars: sky.orbit_stars(),
            }
        }
        SkyKind::Clock | SkyKind::None => {
            let sun = sky.sun();
            Overhead {
                sun,
                frame: Quat::IDENTITY,
                apparent: sun,
                stars: sky.spin(),
            }
        }
    }
}

/// Which shell of the sky an entity is. One component with four values rather
/// than four marker components, because every one of them is moved by the same
/// system and three quarters of it would be `Option`s otherwise.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkyPart {
    Dome,
    Stars,
    Sun,
    Moon,
}

/// Puts the sky up. Called once from `main::setup`, beside the other two
/// `prepare`s, and never again: all four shells ride the camera, so there is
/// nothing in any of them that a change of level invalidates. What a change of
/// level does is turn them off -- see [`advance`].
pub fn prepare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<N64Material>,
) {
    // One white texel, so the dome's colour is entirely its vertices. The
    // shader multiplies the tint by a texture sample whatever the material
    // says, and a material with no texture at all leans on a fallback this
    // module would rather not be relying on the colour of.
    let white = images.add(texture(1, |_, _| [255, 255, 255, 255]));
    commands.spawn((
        SkyPart::Dome,
        Name::new("sky: dome"),
        Mesh3d(meshes.add(dome())),
        MeshMaterial3d(materials.add(shell(Some(white), AlphaMode::Opaque))),
        Transform::default(),
    ));
    commands.spawn((
        SkyPart::Stars,
        Name::new("sky: stars"),
        Mesh3d(meshes.add(stars())),
        MeshMaterial3d(materials.add(shell(Some(images.add(spark())), AlphaMode::Blend))),
        Transform::default(),
        Visibility::Hidden,
    ));
    for (part, texture, size) in [
        (SkyPart::Sun, images.add(sun_disc()), SUN_SIZE),
        (SkyPart::Moon, images.add(moon_disc()), MOON_SIZE),
    ] {
        commands.spawn((
            part,
            Name::new(format!("sky: {part:?}").to_lowercase()),
            // A unit quad in its own XY plane facing +Z, which is what
            // `looking_to` in [`follow`] then turns onto the camera.
            Mesh3d(meshes.add(Rectangle::new(1.0, 1.0).mesh().build())),
            MeshMaterial3d(materials.add(shell(Some(texture), AlphaMode::Blend))),
            Transform::from_scale(Vec3::splat(size)),
            Visibility::Hidden,
        ));
    }
}

/// The material every shell wears: luminous, unfogged, and drawn from both
/// sides.
///
/// **Luminous** because the sky is not a surface the sun falls on, it is where
/// the sun is. Lighting it would be shading the light, and *dimming* it -- what
/// [`N64Uniform::unlit`] would have got it, along with the castle -- would take
/// the colour out of the sunset and the stars out of the night. **Unfogged** for the reason
/// in [`N64Uniform::beyond_the_fog`]. **Double-sided** because the camera is
/// inside every one of these spheres, so what it sees is their inner face --
/// which is the back of every triangle. Nothing is paid for it: a sphere
/// centred on the camera covers each pixel exactly once however it is wound,
/// since every point on it is the same distance away.
fn shell(texture: Option<Handle<Image>>, alpha_mode: AlphaMode) -> N64Material {
    N64Material {
        uniform: N64Uniform::luminous(0.0).beyond_the_fog(),
        base_color_texture: texture,
        // Never read either: the shader adds a lamp's light to everything but
        // a luminous surface, and this is the one luminous surface there is.
        // The binding still has to be filled -- a material pointing at nothing
        // has no bind group and draws no pixels at all.
        lamps: crate::n64::LAMPLIGHT,
        alpha_mode,
        double_sided: true,
        // Never read: an unlit surface is drawn by the same pipeline whichever
        // way the player has the display option set, which is what
        // `N64MaterialKey` pins. Same reasoning as the impostor sheets.
        shading: Shading::Vertex,
    }
}

/// The dome: a sphere of [`DOME_RADIUS`], carrying positions and a colour per
/// vertex and nothing else.
///
/// No normals, because nothing lights it, and no UVs, because its one texel is
/// white wherever it is sampled. The colours are placeholders -- [`repaint`]
/// writes the real ones before the first frame is drawn.
fn dome() -> Mesh {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for ring in 0..=DOME_RINGS {
        // Pole to pole, so `ring` 0 is straight up and the last is straight
        // down.
        let polar = ring as f32 / DOME_RINGS as f32 * std::f32::consts::PI;
        let (sin_polar, cos_polar) = polar.sin_cos();
        for segment in 0..=DOME_SEGMENTS {
            let around = segment as f32 / DOME_SEGMENTS as f32 * std::f32::consts::TAU;
            let (sin_around, cos_around) = around.sin_cos();
            positions.push(
                [sin_polar * cos_around, cos_polar, sin_polar * sin_around]
                    .map(|axis| axis * DOME_RADIUS),
            );
        }
    }
    let stride = DOME_SEGMENTS + 1;
    for ring in 0..DOME_RINGS {
        for segment in 0..DOME_SEGMENTS {
            let corner = ring * stride + segment;
            indices.extend_from_slice(&[
                corner,
                corner + stride,
                corner + 1,
                corner + 1,
                corner + stride,
                corner + stride + 1,
            ]);
        }
    }
    let colours = vec![[1.0_f32; 4]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        // `repaint` rewrites the colours from the CPU as the light moves, so
        // the main-world copy has to stay -- the same reason `water::drift`
        // keeps its sheets.
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
    .with_inserted_indices(Indices::U32(indices))
}

/// The star field: [`STAR_COUNT`] quads scattered over a sphere of
/// [`STAR_RADIUS`], each lying flat against it and therefore square-on to a
/// camera at the middle.
///
/// One mesh rather than one entity per star, because four hundred entities is
/// four hundred draw calls to put a few hundred pixels on the screen. It never
/// changes afterwards: the field turns by turning the entity, and it fades by
/// the tint on its material.
fn stars() -> Mesh {
    let mut rng = Rng(STAR_SEED);
    let mut positions = Vec::with_capacity(STAR_COUNT * 4);
    let mut uvs = Vec::with_capacity(STAR_COUNT * 4);
    let mut colours = Vec::with_capacity(STAR_COUNT * 4);
    let mut indices = Vec::with_capacity(STAR_COUNT * 6);
    for star in 0..STAR_COUNT {
        // Uniform over the sphere rather than over the two angles: stepping
        // latitude evenly crowds the poles, and a night sky with two bald
        // patches in it is a sky nobody believes.
        let height = 1.0 - 2.0 * rng.next();
        let radius = (1.0 - height * height).max(0.0).sqrt();
        let around = rng.next() * std::f32::consts::TAU;
        let direction = Vec3::new(radius * around.cos(), height, radius * around.sin());
        let (right, up) = direction.any_orthonormal_pair();
        // Squared, so most stars are faint and a few are not. An evenly bright
        // field reads as noise rather than as stars.
        let bright = 0.30 + 0.70 * rng.next().powi(2);
        let size = STAR_SIZE.0 + (STAR_SIZE.1 - STAR_SIZE.0) * rng.next();
        // A little colour, warm one way and cool the other, about white.
        let warmth = rng.next() - 0.5;
        let tint = Vec3::new(1.0 + warmth * 0.20, 1.0, 1.0 - warmth * 0.22) * bright;
        let centre = direction * STAR_RADIUS;
        let base = (star * 4) as u32;
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let at = centre + right * ((u - 0.5) * size) + up * ((v - 0.5) * size);
            positions.push([at.x, at.y, at.z]);
            uvs.push([u, v]);
            colours.push([tint.x, tint.y, tint.z, 1.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
    .with_inserted_indices(Indices::U32(indices))
}

/// Whatever the sun and the moon and the stars are drawn with, generated rather
/// than shipped -- the same trade `shadow::disc` makes, and for the same
/// reason: a formula in the source is one fewer file for the Windows packaging
/// step to be told about and forget.
///
/// `texel` is handed the pixel's place in the square, running -1 to 1 on both
/// axes, so a disc is a test on its length.
///
/// Shared out of this module rather than copied into the next one that wants a
/// small generated picture: [`crate::nuclonium`] draws its glow with the same three
/// lines, and two identical loops writing RGBA into a `Vec` is exactly the
/// duplication this port spends its comments refusing.
pub(crate) fn texture(size: u32, mut texel: impl FnMut(f32, f32) -> [u8; 4]) -> Image {
    let across = size as f32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 + 0.5) / across * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / across * 2.0 - 1.0;
            pixels.extend_from_slice(&texel(u, v));
        }
    }
    Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        // Nothing reads any of these back once they are on the GPU.
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// The sun: a solid core with a halo falling away round it.
///
/// The core is white rather than yellow and the material tints it, which is
/// what lets the same texture be a white sun overhead and a red one on the
/// horizon without a second picture.
fn sun_disc() -> Image {
    texture(96, |u, v| {
        let distance = Vec2::new(u, v).length();
        let core = 1.0 - smooth_step(0.20, 0.30, distance);
        // The bloom right at the sun's edge, which the dome is too coarse to
        // carry -- see `HALO_FOCUS`.
        let halo = (1.0 - smooth_step(0.16, 1.0, distance)).powi(3) * 0.62;
        let alpha = (core + halo).clamp(0.0, 1.0);
        [255, 255, 255, (alpha * 255.0).round() as u8]
    })
}

/// The moon: a disc with a hard limb, a few darker patches on it, and a much
/// fainter halo than the sun's.
///
/// The patches are three circles rather than a picture of the real thing. What
/// they are for is that a plain white disc at this size reads as a hole punched
/// in the sky; anything at all breaking it up reads as a moon.
fn moon_disc() -> Image {
    const MARE: [(Vec2, f32, f32); 3] = [
        (Vec2::new(-0.10, 0.08), 0.13, 0.24),
        (Vec2::new(0.12, -0.06), 0.09, 0.18),
        (Vec2::new(0.02, 0.20), 0.07, 0.14),
    ];
    texture(96, |u, v| {
        let at = Vec2::new(u, v);
        let distance = at.length();
        // A hard edge, softened by about a texel so the limb is a curve rather
        // than a staircase.
        let body = 1.0 - smooth_step(0.36, 0.385, distance);
        let halo = (1.0 - smooth_step(0.36, 0.95, distance)).powi(3) * 0.30;
        let mut shade = 1.0_f32;
        for (centre, inner, outer) in MARE {
            shade -= 0.22 * (1.0 - smooth_step(inner, outer, (at - centre).length()));
        }
        let shade = shade.clamp(0.55, 1.0);
        let grey = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        [
            grey(shade),
            grey(shade * 0.99),
            grey(shade * 0.94),
            grey((body + halo).clamp(0.0, 1.0)),
        ]
    })
}

/// One star: a soft point, brightest at the middle. Small, because it is drawn
/// three or four pixels across and anything more detailed than this is detail
/// nobody will ever see.
fn spark() -> Image {
    texture(16, |u, v| {
        let alpha = (1.0 - smooth_step(0.0, 0.5, Vec2::new(u, v).length())).powi(2);
        [255, 255, 255, (alpha * 255.0).round() as u8]
    })
}

/// A deterministic scatter, so the stars come out in the same places every run.
///
/// xorshift32. Nothing here needs a good generator -- it needs the *same*
/// generator on every machine the game is built for, which is what a crate
/// would not have guaranteed and eight lines do.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        // The top 24 bits, which is every bit an `f32` can hold anyway.
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn smooth_step(from: f32, to: f32, at: f32) -> f32 {
    let t = ((at - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// An sRGB triple, the way [`RAMP`] writes them, as the linear one a vertex
/// colour and a material tint are both actually in.
fn linear(srgb: Vec3) -> Vec3 {
    let converted = LinearRgba::from(Color::srgb(srgb.x, srgb.y, srgb.z));
    Vec3::new(converted.red, converted.green, converted.blue)
}

/// The colour of the sky in the direction `at`, with the sun where `sun` says.
///
/// The one function the dome, the fog and the clear colour all go through, so
/// that the haze the world dissolves into is the same colour as the sky above
/// it by construction rather than by two tables being kept in step. Given the
/// horizon direction it returns exactly [`Look::horizon`], which is what the
/// fog asks it for.
///
/// In sRGB, like the table it reads.
fn sky_colour(at: Vec3, sun: Vec3, look: &Look) -> Vec3 {
    let up = at.y.clamp(-1.0, 1.0);
    // The gradient. The power is what keeps the pale band tight against the
    // horizon instead of washing halfway up the sky: linear in `up`, the
    // horizon colour is still half the sky at forty-five degrees.
    let base = if up >= 0.0 {
        look.horizon.lerp(look.zenith, up.powf(0.45))
    } else {
        look.horizon.lerp(look.ground, (-up).powf(0.55))
    };
    // How much of the warm to lay over it: a tight halo about the sun itself,
    // plus a wide band lying along the horizon under it.
    let toward = at.dot(sun).max(0.0);
    let halo = toward.powf(HALO_FOCUS);
    let band = toward.powf(BAND_FOCUS) * (1.0 - up.abs()).max(0.0).powf(BAND_FLATNESS);
    let warmth = (halo + band * BAND_STRENGTH).clamp(0.0, 1.0);
    base.lerp(look.glow, warmth)
}

/// How much is left of the light every bake in this game was made under.
///
/// Derived rather than tabulated, because it is not a look -- it is a
/// bookkeeping fact about the other numbers. The castle's vertex colours and
/// the impostor sheets were both resolved under the last stop of [`RAMP`], so
/// what is honest to multiply them by now is how much light there is against
/// how much there was then. Half a key plus the ambient is the light on a
/// surface at forty-five degrees to the sun, which is the average surface.
///
/// Per channel, so it carries the colour of the hour and not only its
/// brightness: the castle goes blue at midnight and orange at sunset because
/// the ratio does, without a second table to keep in step with the first.
///
/// **This is a linear ratio and the screen is not.** What comes out of it at
/// midnight is about 0.06, and 0.06 of the light is not 6% of the picture --
/// encoded to sRGB it is a bit over a quarter, which is what a moonlit field
/// should look like and is bright enough to play in. The night stops of
/// [`RAMP`] were chosen by reading that number off the far end rather than by
/// eye: picked to look dark as *linear* multipliers they came out at half
/// brightness on the screen, which is a bright overcast afternoon.
fn daylight(look: &Look) -> Vec3 {
    let noon = RAMP[RAMP.len() - 1].1;
    (look.ambient + look.key * 0.5) / (noon.ambient + noon.key * 0.5)
}

/// How much of a celestial body is drawn, given how high it is. See
/// [`HORIZON_FADE`].
fn above_horizon(elevation: f32) -> f32 {
    smooth_step(HORIZON_FADE.0, HORIZON_FADE.1, elevation)
}

/// Runs the clock, and pushes the sun out to everything that is not the sky
/// itself: the world's light, the camera's fog, and the colour behind
/// everything.
///
/// The clock and the console's `sky_hour` are the same number seen from two
/// sides. Every frame this reads the slider, and a value that is not the one it
/// wrote last frame can only have been typed or dragged -- so that is taken as
/// the player scrubbing to an hour and the clock jumps to it. Otherwise the
/// clock advances and the slider is written to follow. `day_length` at zero
/// holds it, which is how you park the game at a sunset and look at it.
///
/// The same clock runs both skies. On the castle it drives the sun round its
/// axis; on the orbiting planet it *is* the spin, so scrubbing the slider
/// there is turning the globe by hand. The year only advances while the clock
/// runs -- a scrub moves the day and not the season.
#[allow(clippy::too_many_arguments)]
pub fn advance(
    time: Res<Time>,
    level: Res<LevelId>,
    medium: Res<CameraMedium>,
    gravity: Option<Res<Gravity>>,
    mut tuning: ResMut<GameTuning>,
    mut sky: ResMut<Sky>,
    mut lighting: ResMut<N64Lighting>,
    mut clear: ResMut<ClearColor>,
    mut fog: Query<(&mut DistanceFog, &GlobalTransform), With<Camera3d>>,
) {
    let kind = sky_of(*level);
    if kind == SkyKind::None {
        // Not this level's sky. Everything the clock was driving goes back to
        // the fixed value the rest of the game was written against --
        // including the fog and the clear colour, or a player who steps onto
        // the planet at midnight arrives under a black sky that nothing there
        // will ever repaint.
        //
        // Once, on the frame the level changed, which is what taking
        // `lit_from` says: relighting is a rewrite of every material in the
        // game and there is nothing here for it to keep up with.
        if sky.lit_from.take().is_some() {
            let noon = RAMP[RAMP.len() - 1].1;
            lighting.key = noon.key;
            lighting.ambient = noon.ambient;
            lighting.daylight = Vec3::ONE;
            lighting.to_light = N64Lighting::default().to_light;
            if !medium.submerged() {
                clear.0 = water::SKY_COLOUR;
                if let Ok((mut fog, _)) = fog.single_mut() {
                    fog.color = water::SKY_COLOUR;
                }
            }
        }
        return;
    }

    // The handshake with the console slider.
    if (tuning.sky_hour - sky.published).abs() > 1e-4 {
        sky.hours = tuning.sky_hour;
    } else if tuning.day_length > 0.0 {
        let step = HOURS * time.delta_secs() / tuning.day_length;
        sky.hours += step;
        if kind == SkyKind::Orbit {
            sky.year = (sky.year + step / HOURS).rem_euclid(YEAR_DAYS);
        }
    }
    sky.hours = sky.hours.rem_euclid(HOURS);
    sky.published = sky.hours;
    tuning.sky_hour = sky.hours;

    // Which way is up under the camera: `+Y` on the castle, and off the
    // planet's core under anywhere the player has walked to. Missing pieces
    // fall back to `+Y` rather than skipping the frame -- a headless world
    // with no camera still wants its light run.
    let up = match &gravity {
        Some(gravity) => fog
            .single()
            .map(|(_, eye)| gravity.up(eye.translation()))
            .unwrap_or(Vec3::Y),
        None => Vec3::Y,
    };
    let view = overhead(kind, &sky, up);
    let look = Look::at(view.apparent.y);

    // The fog and the clear colour are the horizon, so the world fades into
    // the sky rather than into a colour that used to match it. Left alone
    // underwater: `water::camera_medium` owns both down there, and the whole
    // point of the underwater fog is that it is *not* the sky.
    if !medium.submerged() {
        let horizon = look.horizon;
        let colour = Color::srgb(horizon.x, horizon.y, horizon.z);
        clear.0 = colour;
        if let Ok((mut fog, _)) = fog.single_mut() {
            fog.color = colour;
        }
    }

    // The light, at a quarter of a degree of sun rather than every frame. See
    // `RELIGHT_STEP` for why that is not a shortcut. Measured on the
    // *apparent* sun: on the planet the world's sun can hold still while the
    // player walks the horizon round underneath it, and either motion is a
    // change of light.
    let moved = sky
        .lit_from
        .is_none_or(|last| last.dot(view.apparent).clamp(-1.0, 1.0).acos() >= RELIGHT_STEP);
    if !moved {
        return;
    }
    sky.lit_from = Some(view.apparent);
    // Where the key light comes from, once the sun is down: from the moon,
    // which is the only thing left up there. Crossfaded rather than switched,
    // because a key that jumps from one side of the sky to the other flips
    // which face of every wall in the level is the lit one, in one frame.
    let day = above_horizon(view.apparent.y);
    let night = above_horizon(-view.apparent.y) * (1.0 - day);
    let direction = (view.sun * day + view.moon() * night).normalize_or(up);
    lighting.to_light = direction;
    lighting.key = look.key;
    lighting.ambient = look.ambient;
    // What the castle and the impostor sheets are dimmed by. Neither of them
    // takes the two terms above at all -- their light was resolved before the
    // game ran -- so without this the sun sets on the actors alone.
    lighting.daylight = daylight(&look);
}

/// Moves the sky. Every shell is centred on the camera, the two discs are
/// turned to face it, the star field is turned with the sun, and the dome is
/// repainted when the light has moved far enough to be worth repainting.
///
/// Split from [`advance`] because they touch nothing in common: this one holds
/// the meshes and the materials and does not care what time it is beyond
/// asking, and that one holds the clock and the world's light and never looks
/// at a vertex.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn follow(
    level: Res<LevelId>,
    medium: Res<CameraMedium>,
    gravity: Option<Res<Gravity>>,
    sky: Res<Sky>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<N64Material>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut parts: Query<(
        &SkyPart,
        &mut Transform,
        &mut Visibility,
        &Mesh3d,
        &MeshMaterial3d<N64Material>,
    )>,
) {
    // Underwater the sky is behind a wall of fog that closes at forty metres,
    // and an unfogged dome six hundred metres out would be drawn straight
    // through it -- open sky seen from the bottom of the moat.
    let kind = sky_of(*level);
    let shown = kind != SkyKind::None && !medium.submerged();
    let Ok(camera) = camera.single() else {
        return;
    };
    let eye = camera.translation();
    // The same up, the same `overhead`, the same inputs as `advance` this
    // frame, so the picture and the light agree to the bit -- which the dome's
    // repaint gate below compares them on.
    let up = match &gravity {
        Some(gravity) => gravity.up(eye),
        None => Vec3::Y,
    };
    let view = overhead(kind, &sky, up);
    let look = Look::at(view.apparent.y);

    for (part, mut transform, mut visibility, mesh, material) in &mut parts {
        if !shown {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        match part {
            SkyPart::Dome => {
                visibility.set_if_neq(Visibility::Visible);
                transform.translation = eye;
                // The dome's pole is the player's up: identity on the castle,
                // and on the planet the turn that keeps its painted horizon
                // lying along the real one as he walks round the world.
                transform.rotation = view.frame;
                // Only when the light has moved: the same gate `advance` uses,
                // read off the same field, so the dome and the world's light
                // step together and never disagree about what time it is.
                if sky.lit_from == Some(view.apparent) {
                    if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
                        // The apparent sun, because the vertices being painted
                        // are in the dome's own frame.
                        repaint(&mut mesh, view.apparent, &look);
                    }
                }
            }
            SkyPart::Stars => {
                visibility.set_if_neq(match look.stars > 0.0 {
                    true => Visibility::Visible,
                    false => Visibility::Hidden,
                });
                transform.translation = eye;
                // The stars are the one thing that actually turns. The dome's
                // gradient is fixed to the horizon and the two discs are moved
                // rather than spun, so this is where "the sky rotates" lives.
                transform.rotation = view.stars;
                tint(&mut materials, material, Vec3::ONE, look.stars);
            }
            SkyPart::Sun => {
                let fade = above_horizon(view.apparent.y);
                visibility.set_if_neq(visible(fade));
                // Wider on the horizon than overhead. `SUN_SWELL`.
                let swell = 1.0 + SUN_SWELL * (1.0 - smooth_step(0.0, 0.30, view.apparent.y));
                *transform = body(eye, view.sun, SUN_SIZE * swell);
                tint(&mut materials, material, linear(look.disc), fade);
            }
            SkyPart::Moon => {
                // Up, and dimmed by daylight rather than hidden by it.
                let daylight = above_horizon(view.apparent.y);
                let fade =
                    above_horizon(-view.apparent.y) * (1.0 - daylight * (1.0 - DAYTIME_MOON));
                visibility.set_if_neq(visible(fade));
                *transform = body(eye, view.moon(), MOON_SIZE);
                tint(&mut materials, material, Vec3::ONE, fade);
            }
        }
    }
}

/// A body of `size` hung in the direction `at` from an eye at `eye`, turned
/// square-on to it.
///
/// `looking_to` points an entity's forward -- its local `-Z` -- along the
/// direction given, so handing it the direction of the body points the quad's
/// `+Z` face, which is the one the mesh was built on, straight back at the eye.
fn body(eye: Vec3, at: Vec3, size: f32) -> Transform {
    Transform::from_translation(eye + at * BODY_RADIUS)
        // The orbit is tilted, so nothing ever reaches the pole and this can
        // never be handed a direction parallel to its up vector.
        .looking_to(at, Vec3::Y)
        .with_scale(Vec3::splat(size))
}

/// Hidden rather than drawn at nothing, which is a quad's worth of blending to
/// add zero to every pixel. Written through `set_if_neq` at every call site,
/// so a sky that has not changed does not wake the visibility propagation once
/// a frame for the whole run.
fn visible(fade: f32) -> Visibility {
    match fade > 0.004 {
        true => Visibility::Visible,
        false => Visibility::Hidden,
    }
}

/// Writes a shell's tint, in linear RGB with `fade` for its alpha.
fn tint(
    materials: &mut Assets<N64Material>,
    handle: &MeshMaterial3d<N64Material>,
    colour: Vec3,
    fade: f32,
) {
    if let Some(mut material) = materials.get_mut(&handle.0) {
        material.uniform = material.uniform.tinted(colour.extend(fade));
    }
}

/// Rewrites the dome's colours for a sun at `sun`.
///
/// Every vertex is on a sphere about the origin, so its direction is its
/// position normalised -- which is why the mesh carries no normals: the one
/// thing a normal would have said is already in the position.
fn repaint(mesh: &mut Mesh, sun: Vec3, look: &Look) {
    let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return;
    };
    let colours: Vec<[f32; 4]> = positions
        .iter()
        .map(|position| {
            let direction = Vec3::from_array(*position).normalize_or(Vec3::Y);
            let colour = linear(sky_colour(direction, sun, look));
            [colour.x, colour.y, colour.z, 1.0]
        })
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
}

/// The clock first, then the geometry that shows it: [`follow`] reads
/// `Sky::lit_from` to decide whether to repaint, and [`advance`] is what writes
/// it. The other way round, the dome would be painted for a sun a frame behind
/// the one the world was lit by.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (advance, follow).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: f32) -> Sky {
        Sky {
            hours,
            ..Sky::default()
        }
    }

    /// The sun turns about an axis it starts square to, which is what makes
    /// every direction it is ever at a unit vector. Re-tilt the orbit without
    /// keeping that true and its path is a cone: the sun shortens as it climbs
    /// and every colour keyed on its elevation is read from the wrong place.
    #[test]
    fn the_orbit_is_a_circle() {
        assert!(
            ORBIT_AXIS.normalize().dot(SUNRISE).abs() < 1e-6,
            "the sun starts somewhere other than square to the axis it turns about"
        );
        for hour in 0..24 {
            let sun = at(hour as f32).sun();
            assert!(
                (sun.length() - 1.0).abs() < 1e-5,
                "the sun at {hour}:00 is {} long rather than one",
                sun.length()
            );
        }
    }

    /// The orbiting planet's sun is on the unit sphere whatever the hour and
    /// the season, for the same reason the castle's is: every colour in the
    /// [`RAMP`] is keyed on its elevation, and a sun that shortens is a sun
    /// read from the wrong row.
    #[test]
    fn the_orbiting_sun_is_a_unit_vector_all_year() {
        for day in 0..8 {
            for hour in 0..24 {
                let mut sky = at(hour as f32);
                sky.year = day as f32;
                let sun = sky.orbit_sun();
                assert!(
                    (sun.length() - 1.0).abs() < 1e-5,
                    "the sun on day {day} at {hour}:00 is {} long",
                    sun.length()
                );
            }
        }
    }

    /// The tilt is what turns the orbit into seasons: across the year the
    /// subsolar point swings from one tropic to the other and back. Without
    /// this the sun would ride the same circle every day and the year would
    /// be invisible from the ground.
    #[test]
    fn the_year_swings_the_sun_between_the_tropics() {
        let mut sky = at(12.0);
        let solstice = AXIAL_TILT.sin();
        sky.year = 0.0;
        assert!(
            (sky.orbit_sun().y + solstice).abs() < 1e-5,
            "the year does not open at a solstice: {}",
            sky.orbit_sun().y
        );
        sky.year = YEAR_DAYS / 2.0;
        assert!(
            (sky.orbit_sun().y - solstice).abs() < 1e-5,
            "half a year later the other pole does not have the sun"
        );
        sky.year = YEAR_DAYS / 4.0;
        assert!(
            sky.orbit_sun().y.abs() < 1e-5,
            "the equinox sun is off the equator by {}",
            sky.orbit_sun().y
        );
    }

    /// The orbit's one signature in the sky: measured against the stars the
    /// sun holds still across a day and drifts across the year. Break the
    /// shared spin between [`Sky::orbit_sun`] and [`Sky::orbit_stars`] and
    /// the constellations slide behind the sun in a single evening.
    #[test]
    fn the_sun_drifts_through_the_stars_across_the_year() {
        let against_the_stars = |sky: &Sky| sky.orbit_stars().inverse() * sky.orbit_sun();
        let mut sky = at(6.0);
        sky.year = 1.0;
        let morning = against_the_stars(&sky);
        sky.hours = 20.0;
        assert!(
            (against_the_stars(&sky) - morning).length() < 1e-5,
            "the sun moved against the stars over one day"
        );
        sky.year = 3.0;
        assert!(
            (against_the_stars(&sky) - morning).length() > 0.5,
            "a quarter of a year did not move the sun through the constellations"
        );
    }

    /// The whole point of the orbiting level, held at the resource level: the
    /// season changes what the ground gets. At the north pole -- which is
    /// where the fallback up of `+Y` stands -- the year opens in polar night,
    /// and half a year later the same midnight hour is under the midnight
    /// sun.
    #[test]
    fn the_seasons_reach_the_ground_on_the_orbiting_planet() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<GameTuning>()
            .init_resource::<Sky>()
            .init_resource::<N64Lighting>()
            .init_resource::<CameraMedium>()
            .init_resource::<ClearColor>()
            .insert_resource(LevelId::PlanetOrbit)
            .insert_resource(Gravity::towards(Vec3::ZERO))
            .add_systems(Update, advance);
        app.update();
        let winter = app.world().resource::<N64Lighting>().daylight;
        assert!(
            winter.max_element() < 0.2,
            "the pole's winter night still gets {winter:?} of the daylight"
        );
        // And the sky was painted for it rather than left at the fixed noon
        // the plain planet keeps.
        assert_ne!(
            app.world().resource::<ClearColor>().0,
            water::SKY_COLOUR,
            "the orbiting planet is under the castle's stand-in sky"
        );

        app.world_mut().resource_mut::<Sky>().year = YEAR_DAYS / 2.0;
        app.update();
        let summer = app.world().resource::<N64Lighting>().daylight;
        assert!(
            summer.min_element() > winter.max_element() * 2.0,
            "the midnight sun ({summer:?}) is no brighter than the polar night ({winter:?})"
        );
    }

    /// The clock has to agree with the words. Six is sunrise, noon is the top
    /// of the arc, six in the evening is sunset and midnight is the bottom --
    /// and a slider labelled in hours that does none of those is a slider
    /// nobody can use.
    #[test]
    fn the_sun_keeps_the_hours_its_slider_is_labelled_with() {
        let dawn = at(6.0).sun();
        assert!(
            dawn.y.abs() < 1e-5,
            "the sun is not on the horizon at six in the morning, it is at {}",
            dawn.y
        );
        assert!(dawn.x > 0.9, "the sun does not rise in the east");

        let noon = at(12.0).sun();
        assert!(
            noon.y > 0.85,
            "the sun is only {} of the way up at noon",
            noon.y
        );

        let dusk = at(18.0).sun();
        assert!(
            dusk.y.abs() < 1e-5,
            "the sun is not on the horizon at six in the evening, it is at {}",
            dusk.y
        );
        assert!(dusk.x < -0.9, "the sun does not set in the west");

        assert!(
            at(0.0).sun().y < -0.85,
            "the sun has not gone down at midnight"
        );
    }

    /// The one thing that makes the moon worth having: it is up when the sun is
    /// not, so the night has something in it.
    #[test]
    fn the_moon_is_up_at_night_and_the_sun_is_not() {
        let midnight = overhead(SkyKind::Clock, &at(0.0), Vec3::Y);
        assert!(midnight.sun.y < 0.0 && midnight.moon().y > 0.0);
        let noon = overhead(SkyKind::Clock, &at(12.0), Vec3::Y);
        assert!(noon.sun.y > 0.0 && noon.moon().y < 0.0);
    }

    /// The sky is only worth having if it changes, and it has to change in the
    /// direction the words say: dark at midnight, bright at noon, stars at one
    /// and none at the other.
    #[test]
    fn midnight_is_darker_than_noon() {
        let night = Look::at(at(0.0).sun().y);
        let day = Look::at(at(12.0).sun().y);
        assert!(
            night.key.length() < day.key.length() * 0.4,
            "the night is nearly as bright as the day"
        );
        assert!(night.ambient.length() < day.ambient.length());
        assert!(night.zenith.length() < day.zenith.length());
        // Not quite the bottom stop of the table: the orbit is tilted, so the
        // deepest the sun ever gets is the mirror of noon, 0.87 under. The
        // stops at the two ends are anchors the ramp is clamped against rather
        // than skies anything ever stands in.
        assert!(night.stars > 0.9, "the stars are only {} out", night.stars);
        assert_eq!(day.stars, 0.0);
    }

    /// The stop at the top of the table is the light this game had before it
    /// had a sky, and it is there so that the middle of the day still looks
    /// like every screenshot taken of the game so far -- including the impostor
    /// sheets, which were baked under exactly these numbers and are drawn unlit
    /// against whatever the world is lit by now.
    #[test]
    fn noon_is_the_light_the_game_was_tuned_with() {
        let noon = Look::at(1.0);
        let was = N64Lighting::default();
        assert!(
            (noon.key - was.key).length() < 1e-6,
            "the key light at noon is {:?} and used to be {:?}",
            noon.key,
            was.key
        );
        assert!((noon.ambient - was.ambient).length() < 1e-6);
    }

    /// The fog is asked for the horizon and has to get the horizon back,
    /// unwarmed -- otherwise the far distance is a different colour from the
    /// sky it is supposed to be dissolving into, and the join shows as a ring
    /// round the level.
    #[test]
    fn the_horizon_away_from_the_sun_is_the_colour_the_fog_takes() {
        let sky = at(9.0);
        let sun = sky.sun();
        let look = Look::at(sun.y);
        // Directly away from the sun, on the horizon: no halo, no band.
        let away = Vec3::new(-sun.x, 0.0, -sun.z).normalize();
        let colour = sky_colour(away, sun, &look);
        assert!(
            (colour - look.horizon).length() < 1e-4,
            "the horizon opposite the sun is {colour:?} and the fog would be {:?}",
            look.horizon
        );
    }

    /// The sunset has to be *in the west*, which is the whole reason the glow
    /// is a function of the direction to the sun rather than of the hour.
    #[test]
    fn the_glow_follows_the_sun_round_the_horizon() {
        let sky = at(18.2);
        let sun = sky.sun();
        let look = Look::at(sun.y);
        let toward = Vec3::new(sun.x, 0.0, sun.z).normalize();
        let away = -toward;
        let warm = |at: Vec3| {
            let colour = sky_colour(at, sun, &look);
            colour.x - colour.z
        };
        assert!(
            warm(toward) > warm(away) + 0.2,
            "the horizon under the setting sun is no warmer than the one behind it"
        );
    }

    /// Nothing above the horizon may go transparent and nothing well below it
    /// may be drawn, or the sun sits in the middle of a midnight sky -- there is
    /// no ground out at five hundred units to hide it behind.
    #[test]
    fn a_body_below_the_horizon_is_not_drawn() {
        assert_eq!(above_horizon(-0.5), 0.0);
        assert_eq!(above_horizon(0.5), 1.0);
        assert!(above_horizon(0.0) > 0.0 && above_horizon(0.0) < 1.0);
        assert_eq!(visible(above_horizon(-0.5)), Visibility::Hidden);
        assert_eq!(visible(above_horizon(0.5)), Visibility::Visible);
    }

    /// A star field with two stars in the same place is a scatter that is not
    /// scattering, which is the way a seeded generator usually fails.
    #[test]
    fn the_stars_are_spread_over_the_whole_sphere() {
        let mesh = stars();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the star field has no positions");
        };
        assert_eq!(positions.len(), STAR_COUNT * 4);
        // Every octant of the sky has some of them.
        let mut octants = [0usize; 8];
        for corner in positions.iter().step_by(4) {
            let at = Vec3::from_array(*corner);
            let octant =
                (at.x > 0.0) as usize | ((at.y > 0.0) as usize) << 1 | ((at.z > 0.0) as usize) << 2;
            octants[octant] += 1;
        }
        for (octant, count) in octants.iter().enumerate() {
            assert!(
                *count > STAR_COUNT / 20,
                "octant {octant} holds only {count} of {STAR_COUNT} stars"
            );
        }
    }

    /// The dome has to be closed. A sphere with a seam in it is a stripe of
    /// clear colour running from the zenith to the horizon, and the way to get
    /// one is to share the vertices at the wrap instead of doubling them.
    #[test]
    fn the_dome_is_a_closed_sphere() {
        let mesh = dome();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the dome has no positions");
        };
        assert_eq!(
            positions.len() as u32,
            (DOME_RINGS + 1) * (DOME_SEGMENTS + 1)
        );
        for position in positions {
            let radius = Vec3::from_array(*position).length();
            assert!(
                (radius - DOME_RADIUS).abs() < 1e-2,
                "a dome vertex sits at {radius} rather than {DOME_RADIUS}"
            );
        }
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("the dome is not indexed");
        };
        assert_eq!(indices.len() as u32, DOME_RINGS * DOME_SEGMENTS * 6);
        assert!(indices
            .iter()
            .all(|index| (*index as usize) < positions.len()));
    }

    /// The dome is drawn beyond where the haze is total, so a fogged one is a
    /// flat screen of fog colour with no sun, no stars and no gradient on it.
    ///
    /// Constant assertions on purpose: what is being held is a relationship
    /// between four constants that are edited independently and have no other
    /// reason to stay in that order.
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn the_sky_is_drawn_past_the_fog_and_is_not_fogged() {
        assert!(
            BODY_RADIUS > water::SIGHT,
            "the sun is inside the fog, where the haze would swallow it"
        );
        assert!(STAR_RADIUS > BODY_RADIUS && DOME_RADIUS > STAR_RADIUS);
        let material = shell(None, AlphaMode::Opaque);
        assert!(
            !material.uniform.is_fogged(),
            "the sky is fogged, which paints it out entirely"
        );
    }

    /// Both discs are the far side of the camera from the direction they are
    /// drawn in, facing back down it. Turned the other way they are drawn
    /// edge-on -- which for a flat quad is not drawn at all.
    #[test]
    fn a_body_faces_the_camera_from_where_it_hangs() {
        let eye = Vec3::new(-13.0, 10.0, 56.0);
        for hours in [7.0, 12.0, 17.0] {
            let sun = at(hours).sun();
            let placed = body(eye, sun, SUN_SIZE);
            assert!(
                (placed.translation - eye).normalize().dot(sun) > 0.999,
                "the sun is not drawn in the direction it is in"
            );
            // The quad's own face is its local +Z.
            let facing = placed.rotation * Vec3::Z;
            assert!(
                facing.dot(-sun) > 0.999,
                "the sun is turned {facing:?} rather than back at the camera"
            );
        }
    }

    /// Nothing is dimmed at noon, which is the one value in [`daylight`] that
    /// is not a matter of taste: every bake in the game was made under the top
    /// stop of the ramp, so with the sun there the multiplier has to be exactly
    /// one or the castle is a different colour from the day it was painted for.
    #[test]
    fn the_castle_is_its_own_colour_at_noon() {
        let level = daylight(&Look::at(1.0));
        assert!(
            (level - Vec3::ONE).length() < 1e-6,
            "a baked surface at noon is multiplied by {level:?} rather than one"
        );
        // And is dimmed, rather than merely tinted, at midnight.
        let night = daylight(&Look::at(-0.87));
        assert!(
            night.max_element() < 0.2,
            "midnight leaves {night:?} of the daylight, which is not a night"
        );
    }

    /// `sky_hour` is the one row in the tuning table the game writes back to,
    /// and both directions have to work: the row reads the clock while it runs,
    /// and a value put into the row is taken as the time. Get the handshake
    /// wrong one way and the slider never moves; wrong the other and the clock
    /// is dragged back to whatever the row last said, every frame, for ever.
    #[test]
    fn the_console_row_and_the_clock_are_the_same_number() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<GameTuning>()
            .init_resource::<Sky>()
            .init_resource::<N64Lighting>()
            .init_resource::<LevelId>()
            .init_resource::<CameraMedium>()
            .init_resource::<ClearColor>()
            .add_systems(Update, advance);
        app.update();
        let running = app.world().resource::<Sky>().hours;
        assert_eq!(
            app.world().resource::<GameTuning>().sky_hour,
            running,
            "the row does not follow the clock"
        );

        app.world_mut().resource_mut::<GameTuning>().sky_hour = 3.0;
        app.update();
        let scrubbed = app.world().resource::<Sky>().hours;
        assert!(
            (scrubbed - 3.0).abs() < 0.5,
            "three o'clock was typed into the row and the clock reads {scrubbed}"
        );
        assert_eq!(app.world().resource::<GameTuning>().sky_hour, scrubbed);
    }

    /// A level with a different idea of which way is up gets the light the game
    /// had before any of this, rather than whatever hour the castle was left
    /// at. Without it, stepping onto the planet at midnight is a planet in the
    /// dark that nothing there will ever turn the lights back on for.
    #[test]
    fn leaving_the_castle_puts_the_light_back() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<GameTuning>()
            .init_resource::<Sky>()
            .init_resource::<N64Lighting>()
            .init_resource::<LevelId>()
            .init_resource::<CameraMedium>()
            .init_resource::<ClearColor>()
            .add_systems(Update, advance);
        app.world_mut().resource_mut::<GameTuning>().sky_hour = 0.0;
        app.update();
        assert!(
            app.world().resource::<N64Lighting>().daylight.max_element() < 0.2,
            "midnight on the castle did not dim anything"
        );

        *app.world_mut().resource_mut::<LevelId>() = LevelId::Planet;
        app.update();
        let lighting = *app.world().resource::<N64Lighting>();
        assert_eq!(lighting.daylight, Vec3::ONE);
        assert_eq!(lighting.key, RAMP[RAMP.len() - 1].1.key);
        assert_eq!(
            app.world().resource::<ClearColor>().0,
            water::SKY_COLOUR,
            "the planet was left under the castle's midnight sky"
        );
    }

    /// The clock is a clock: it goes forward, it wraps, and it never leaves the
    /// range the slider is drawn over.
    #[test]
    fn the_clock_wraps_at_midnight() {
        let mut sky = at(23.5);
        let day_length = 60.0;
        for _ in 0..120 {
            sky.hours += HOURS * 0.5 / day_length;
            sky.hours = sky.hours.rem_euclid(HOURS);
            assert!(
                (0.0..HOURS).contains(&sky.hours),
                "the clock read {}",
                sky.hours
            );
        }
    }
}
