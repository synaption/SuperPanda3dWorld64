//! Stellarators: machines you put down by pointing at the ground, whose plasma
//! is never actually drawn.
//!
//! Two halves, and they are only in one file because they arrive together.
//!
//! **Placing one** is the squad's whistle with a different thing on the end of
//! it. There is no cursor to click a spot with -- the crosshair is welded to
//! the middle of the screen -- so the site is aimed exactly as an order to the
//! Marios is aimed, through [`crate::squad::aim_point`]: the view direction is
//! marched until it meets ground, and the answer is taken on the bearing from
//! the player so the camera sitting off his shoulder does not skew it. One
//! button, held and released, and how long it was held is what it says:
//!
//!   * **tapped** -- the smallest machine, dropped where the view was pointing.
//!   * **held** -- a footprint ring opens on the ground and the machine grows
//!     inside it up to a cap, exactly the way the whistle circle grows. Letting
//!     go builds whatever is standing there.
//!
//! The ring is green while the site is clear and red while it is not, and a
//! release over a red ring builds nothing. Two stellarators may not overlap:
//! see [`fits`], which is the whole of the rule and is plain arithmetic so it
//! can be tested without a window.
//!
//! **The plasma is not the plasma mesh.**
//! `tools/generate_stellarator.py` writes a `Twisted Plasma Surface` into the
//! .glb -- a closed, faintly transparent skin sitting inside the coils -- and
//! this module hides it the frame it arrives (see [`claim`]). Drawn, it is a
//! blue balloon: a single blended shell with no depth to it, which reads as a
//! solid object wedged in the machine rather than as confined plasma, and which
//! this renderer's one diffuse light has nothing interesting to say about.
//!
//! **What is drawn instead is the machine's stock.** A stellarator is where the
//! squad's nuclonium ends up -- see [`crate::nuclonium`] for the road it comes
//! home on -- and what turns inside the coils is that nuclonium itself,
//! [`Orbit`]ing the *same* flux surface the plasma mesh describes: round the
//! ring, winding around the tube as it goes. [`flux_point`] is the generator's
//! own parametrisation transcribed, so what you see is where the field is
//! rather than near it, and the twist chasing round the coils is the machine's
//! real field periods.
//!
//! There were streaks of light here before, on the same surface, and they were
//! prettier. They were also a lie the player could not read anything off: a
//! machine holding nothing and a machine holding four hundred looked exactly
//! alike. **A field made of the stock is a stock gauge you can see from across
//! the valley** -- an empty reactor is dark, a full one is a solid green ring
//! -- which is worth more than the streaks were, and costs the same draw call.
//! Emptiness being visible is the point rather than a regression.
//!
//! Above [`ORBIT_CAP`] the machine stops adding motes and goes on counting.
//! Five hundred already reads as full, a five-hundred-and-first is a mote
//! nobody can pick out of the ring it joins, and the alternative is a fifty
//! thousandth one -- so the cap is what stops a long session turning the
//! machine into a frame-rate bill. What is *held* is never capped; see
//! [`Store`].
//!
//! Nothing here is random. Every mote's phase, speed and winding comes off
//! [`crate::squad::GOLDEN_ANGLE`] advanced per index, which is the same trick
//! the ambling Marios and the enemy crowd use and for the same reason: a field
//! that never repeats, out of a machine with no random number generator in it,
//! so a whole session stays reproducible in a test.

use crate::{
    console::GameTuning,
    input::InputState,
    level::LevelData,
    player::Player,
    squad::{self, GOLDEN_ANGLE},
};
use bevy::prelude::*;

// -- the machine ------------------------------------------------------------

/// The glTF node holding the plasma skin, hidden by [`claim`] as it arrives.
///
/// A string because that is what the generator writes and what Bevy's loader
/// puts in a [`Name`]; the same way [`crate::weapon::claim`] finds the muzzle.
const PLASMA_NODE: &str = "Twisted Plasma Surface";

/// The machine's model, named once: [`spawn`] loads it and [`measure`] reads
/// it off disk, and a game whose two halves disagree about which file it is
/// draws one machine and measures another.
const MODEL: &str = "actors/stellarator.glb#Scene0";

/// The plasma's *shape*, as `tools/generate_stellarator.py` draws it -- its
/// proportions, not its size.
///
/// **These have to match the .glb on disk**, because the wisps ride a surface
/// the coils were built around and half a period out is a field visibly
/// threading the wrong side of them. `assets/actors/stellarator.glb` is written
/// with `--field-periods 7 --coils 15`; regenerate it with a different count
/// and `PERIODS` is the number that follows it.
///
/// How *big* the machine is is deliberately not here. That is the generator's
/// `--scale`, and it is read back off the file by [`measure`] rather than
/// written down twice.
const MAJOR: f32 = 3.0;
const MINOR: f32 = 0.72;
const PERIODS: f32 = 7.0;

/// The machine's size, as the file on disk actually describes it.
///
/// **This is why there is no size constant in this module**, and the argument
/// is [`crate::enemy::sizes`]'s, which this is modelled on: an actor is drawn
/// at the size it was authored at and measured at the size it is drawn, so the
/// two cannot disagree. Changing how big a stellarator is means changing the
/// stellarator -- `tools/generate_stellarator.py --scale` -- and everything
/// here follows on the next run.
///
/// It matters more here than it does for a slime, because four separate things
/// have to agree with the file: the ring drawn on the ground, the overlap test
/// that ring is promising, the lift that stands the machine on the lawn rather
/// than in it, and the flux surface the plasma rides. Written down as constants,
/// a re-export at half size leaves a four-metre ring around a two-metre machine
/// hanging a metre and a half in the air with its plasma outside its own coils
/// -- four bugs from one edit, none of which names the edit.
#[derive(Debug, Clone, Copy)]
pub struct Machine {
    /// What the file was baked at, relative to the shape [`flux_point`]
    /// describes: `--scale`, recovered by comparing the plasma mesh's own
    /// bounds against that shape's.
    pub scale: f32,
    /// How much ground it covers, drawn at the size it was authored.
    pub radius: f32,
    /// How high above the machine's own origin the plasma sits.
    ///
    /// The model stands on its origin, the way every model in this game does,
    /// so the flux surface -- which the shape describes about a centre plane of
    /// its own -- is up in the middle of the coils rather than down on the
    /// lawn. This is that offset, and it is what the wisps are raised by.
    ///
    /// Recovered from the file rather than written here: the generator lifts by
    /// whatever its own lowest coil reaches down to, and a coil that reaches
    /// further is a machine that stands higher.
    pub lift: f32,
}

/// What the game falls back on with the model missing from the install.
///
/// Only reachable in a build that has no stellarator to draw at all, so what it
/// mostly buys is a preview ring that is still a sensible size and a `measure`
/// that cannot divide by zero. Roughly the shipped file, and deliberately not
/// pinned to it: keeping a constant in step with an asset somebody is free to
/// re-export is the job this whole module hands to [`measure`].
const UNMEASURED: Machine = Machine {
    scale: 0.45,
    radius: 2.0,
    lift: 0.64,
};

/// The measurement, taken once.
pub fn machine() -> &'static Machine {
    static MACHINE: std::sync::OnceLock<Machine> = std::sync::OnceLock::new();
    MACHINE.get_or_init(|| measure().unwrap_or(UNMEASURED))
}

/// Reads the machine's size out of `assets/actors/stellarator.glb`.
///
/// A header read rather than a walk over vertices: every glTF accessor carries
/// `min` and `max`, and the generator writes one mesh per node with no skins
/// anywhere, so the node chain is the whole of the transform. `enemy::measure`
/// is the same job with the skinning cases this file does not have.
///
/// Straight out of the file, in the game's own axes. It was not always: the
/// generator wrote its Z-up shape into a Y-up format, and both this and
/// [`spawn`] turned it back a quarter about X on the way in. The file is
/// written upright now, which is what a level editor linking the same model
/// needs -- see `to_game` in `tools/generate_stellarator.py`.
fn measure() -> Option<Machine> {
    let bytes = std::fs::read(crate::asset_path().join(MODEL.trim_end_matches("#Scene0"))).ok()?;
    let length = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    let json: serde_json::Value = serde_json::from_slice(bytes.get(20..20 + length)?).ok()?;
    // The box the machine stands in, and -- separately -- the one plasma vertex
    // that says how big the file was baked.
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut plasma = Vec3::ZERO;
    for node in json["nodes"].as_array()? {
        let Some(index) = node["mesh"].as_u64() else {
            continue;
        };
        let mesh = &json["meshes"][index as usize];
        for primitive in mesh["primitives"].as_array()? {
            let accessor =
                &json["accessors"][primitive["attributes"]["POSITION"].as_u64()? as usize];
            let corner = |which: &str| -> Option<Vec3> {
                let bounds = accessor[which].as_array()?;
                Some(Vec3::new(
                    bounds.first()?.as_f64()? as f32,
                    bounds.get(1)?.as_f64()? as f32,
                    bounds.get(2)?.as_f64()? as f32,
                ))
            };
            low = low.min(corner("min")?);
            high = high.max(corner("max")?);
            if mesh["name"].as_str() == Some(PLASMA_NODE) {
                // Not the bounding box. A box is the extreme of the vertices
                // that happen to have been *sampled*, so on the low-poly
                // preset it falls short of the surface it approximates by a
                // few percent -- and a scale taken from it comes out at 0.96
                // for a file baked at 1.0, which is a plasma sitting slightly
                // inside its own coils for no reason anyone would find.
                //
                // One vertex compared against the same point of the shape is
                // exact at any resolution. `surface_mesh` walks `u` then `v`,
                // so the first position it writes is `flux_point(0, 0)`. That
                // is an agreement with `tools/generate_stellarator.py` rather
                // than something checked here: if the generator ever writes
                // its samples in another order, the measured scale comes out
                // wrong and `the_machine_is_measured_off_the_file_it_is_drawn
                // _from` is what notices.
                plasma = first_position(&bytes, &json, accessor, length)?;
            }
        }
    }
    // Two numbers out of one vertex, and the axis each is read on is what
    // separates them. `flux_point(0, 0)` is on the shape's own centre plane, so
    // its height is nought: whatever height that vertex has in the file is the
    // lift the generator baked in, and its distance from the axis is the scale,
    // untouched by the lift.
    let flat = Vec2::new(plasma.x, plasma.z).length();
    if low.x > high.x || flat <= 0.0 {
        return None;
    }
    Some(Machine {
        scale: flat / flux_point(0.0, 0.0).length(),
        radius: low.x.abs().max(high.x).max(low.z.abs()).max(high.z),
        lift: plasma.y,
    })
}

/// The first vertex of an accessor, in the file's own coordinates.
///
/// The one place this module reaches past the JSON chunk into the binary one.
/// Twelve bytes: the accessor's offset within its buffer view, the view's
/// within the chunk, and the chunk's own eight-byte header after the JSON.
fn first_position(
    bytes: &[u8],
    json: &serde_json::Value,
    accessor: &serde_json::Value,
    json_length: usize,
) -> Option<Vec3> {
    let view = &json["bufferViews"][accessor["bufferView"].as_u64()? as usize];
    let at = 20
        + json_length
        + 8
        + view["byteOffset"].as_u64().unwrap_or(0) as usize
        + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    let read = |offset: usize| -> Option<f32> {
        Some(f32::from_le_bytes(
            bytes.get(at + offset..at + offset + 4)?.try_into().ok()?,
        ))
    };
    Some(Vec3::new(read(0)?, read(4)?, read(8)?))
}

/// A built machine, and how much ground it is standing on.
///
/// The radius is carried rather than recomputed from the transform because
/// [`fits`] is the one thing that has to agree with the ring the player was
/// looking at when they let go, and a scale read back off a `Transform` is a
/// second place for that to be worked out.
#[derive(Component)]
pub struct Stellarator {
    pub radius: f32,
}

/// The one preview: a footprint ring and a plasma field with no machine around
/// it yet, moved to wherever the crosshair is pointing and hidden the rest of
/// the time.
///
/// Spawned once at startup and never despawned, the same way
/// [`crate::squad::spawn_circle`]'s ring is, so changing level does not take
/// the thing you build with away.
#[derive(Component)]
pub struct BuildSite;

/// The ring on the ground under the preview. Its material is the answer to
/// "will this go here".
#[derive(Component)]
pub struct Footprint;

/// One mote of nuclonium turning inside the coils, and the two clocks that
/// make it its own.
///
/// `u` runs round the ring the long way and `v` round the tube; `rate` and
/// `wind` are how fast this one does each. All four are seeded off the golden
/// angle by the mote's own index in [`mote`], which is what lets the field be
/// grown one mote at a time and still be evenly spread at every count: the
/// hundredth arrival lands in the gap the first ninety-nine left, without
/// anything having to be re-laid.
#[derive(Component)]
pub struct Orbit {
    /// Which arrival this was, counting from the machine's first. Kept so that
    /// a machine giving stock back sheds its *newest* motes and leaves an
    /// evenly spread field behind -- see [`stock`].
    index: u32,
    u: f32,
    rate: f32,
    v: f32,
    wind: f32,
    /// Which nested flux surface it rides, from the axis out. See
    /// [`MOTE_REACH`]: the field fills the tube rather than tiling its skin,
    /// which is both what plasma looks like and what stops five hundred motes
    /// forming a hollow shell with a hole down the middle.
    rho: f32,
    /// How much of its arrival is still to happen, from one down to nothing.
    ///
    /// **A mote is a ball that just flew here, and it has to arrive rather than
    /// appear.** What a shipment does is land at this machine's feed point --
    /// the middle of the coils, which is `Vec3::Y * machine().lift` in the
    /// machine's own frame -- and stop existing. If the mote it turns into is
    /// placed on its flux surface on the first frame, the substance the player
    /// watched crossing the whole valley finishes by blinking out at one point
    /// and back in at another one a metre away.
    ///
    /// So a new mote starts exactly where the flight ended and swims out to its
    /// orbit over [`ARRIVE_SECONDS`]. It costs one lerp per mote per frame and
    /// it is what makes the hand-off continuous -- and because a mote carries
    /// the same [`crate::nuclonium::Trail`] a ball on the lawn does, the swim
    /// out draws its own wake into the coils.
    settling: f32,
}

/// What one machine is holding, and how much of it is on screen.
///
/// Its own component rather than a field on [`Stellarator`], because it is what
/// the *machine* is for rather than what it is: the build preview is a
/// stellarator with no stock, and the ore patches `next.md` wants would credit
/// this same component from somewhere that is not a pylon at all.
///
/// The two numbers are deliberately not the same number. `held` is the truth
/// and has no ceiling; `shown` is how many motes are actually parented to the
/// machine and stops at [`ORBIT_CAP`]. [`stock`] is the one thing that moves
/// `shown`, by spawning or despawning the difference.
#[derive(Component, Default, Debug)]
pub struct Store {
    /// Units of nuclonium this machine has taken in. Never capped.
    pub held: u32,
    /// How many are drawn. `min(held, ORBIT_CAP)`, reached one tick at a time.
    shown: u32,
}

impl Store {
    /// How many motes a machine holding this much should be drawing.
    ///
    /// The cap in one place, as a function, so the test that pins it does not
    /// have to build a world to ask.
    pub fn drawn(held: u32) -> u32 {
        held.min(ORBIT_CAP)
    }
}

/// One of the machine's modular coils, and what makes it drift on its own
/// clock.
///
/// `rest` is where the glTF put it, kept rather than assumed to be the origin:
/// the coils are baked into their vertices today and the node transform is
/// identity, but a .blend-authored machine would not be, and a drift written as
/// an absolute position would silently move every coil to the middle.
#[derive(Component)]
pub struct Coil {
    rest: Vec3,
    phase: f32,
    rate: f32,
}

/// What a coil's node is called before its number. `tools/generate_stellarator.py`
/// writes `Modular Coil 01` upward, one per coil.
const COIL_NODE: &str = "Modular Coil ";

/// A point on the plasma's flux surface, in the game's axes.
///
/// Transcribed from `plasma()` in `tools/generate_stellarator.py`, through the
/// same `to_game` turn that file applies on the way out: the shape is described
/// with the machine lying in x/y about an axis of z, and written with the
/// machine lying in x/z about an axis of y. So this is `plasma()` with the last
/// two coordinates swapped and one negated, and a wisp and the coils it threads
/// are in one frame.
///
/// About the shape's own centre plane, not the model's origin -- the file is
/// lifted so it stands on the ground, and [`Machine::lift`] is that offset.
pub fn flux_point(u: f32, v: f32) -> Vec3 {
    flux_point_at(u, v, 1.0)
}

/// The same shape, `rho` of the way out from its own axis.
///
/// `rho = 1` is [`flux_point`] -- the plasma's skin, the surface the .glb's mesh
/// describes. `rho = 0` is the magnetic axis: the curve running down the middle
/// of the tube, which is what is left when the two terms that carry `v` are
/// taken away. Anything between is a nested surface inside the last one, which
/// is what a flux surface *is*, so this is the shape's own natural parameter
/// rather than a fudge factor bolted onto it.
///
/// It exists so the motes can fill the tube instead of tiling its skin -- see
/// [`MOTE_REACH`], which is how far out they are allowed to go and why that is
/// less than all the way.
pub fn flux_point_at(u: f32, v: f32, rho: f32) -> Vec3 {
    let twist = PERIODS * u;
    let section = MINOR * (1.0 + 0.16 * twist.cos());
    let radius = MAJOR + rho * section * v.cos() + 0.16 * twist.cos();
    let height = rho * 0.72 * MINOR * v.sin() + 0.18 * twist.sin();
    Vec3::new(radius * u.cos(), height, -(radius * u.sin()))
}

// -- placing ----------------------------------------------------------------

/// How big a machine the build button puts down: **one size, the size the model
/// was authored at.**
///
/// The hold used to grow it, between half and full over a second and a bit,
/// borrowing the shape of [`crate::squad::circle_radius`] so that one button in
/// the player's hands behaved one way. It looked like a feature and played like
/// a mistake: every machine came out a slightly different size depending on how
/// long a thumb happened to rest on a key, none of the sizes meant anything --
/// a small stellarator is not cheaper, does not hold less and does not power
/// less -- and two machines side by side just looked wrong. A stellarator is a
/// stellarator.
///
/// The hold still opens the footprint ring, which is the half of it that was
/// ever load-bearing: it says *where* the machine is going and whether the site
/// is clear, and it is the same ring at the same size the whole time.
///
/// One, and not a multiple of one, so `--scale` in `generate_stellarator.py` is
/// the last word on how big a stellarator can be. That has been wrong before: a
/// cap of 1.6 meant re-exporting the model at half size still left a
/// fourteen-metre machine reachable, and the only way to find that out was to
/// build one.
pub const BUILD_SCALE: f32 = 1.0;

/// How much ground a machine built at this size stands on.
///
/// The one place the footprint is turned into a radius, so the ring drawn on
/// the ground, the overlap test and anything asking after the fact all get the
/// same number -- and all three come off [`machine`], so all three follow the
/// file.
pub fn footprint(scale: f32) -> f32 {
    machine().radius * scale
}

/// How big the machine is after the button has been down this long.
///
/// A constant function of its argument, kept as a function rather than dissolved
/// into the two call sites because it is *the* answer to "how big is the machine
/// about to be" -- the preview ring, the overlap test and the spawn all ask it,
/// and a size that stops being one decision is a ring that stops matching what
/// gets built. See [`BUILD_SCALE`] for why holding no longer grows it.
pub fn build_scale(_held_for: f32) -> f32 {
    BUILD_SCALE
}

/// Is there room for a machine of this size here?
///
/// Flat distance against the sum of the two footprints, which is the same
/// question the ring on the ground is drawing. Height is deliberately not in
/// it: two machines on ledges ten metres apart vertically still have their
/// support rings in the same column of air, and a rule that let them through
/// would put one through the other.
pub fn fits(at: Vec3, radius: f32, placed: &[(Vec3, f32)]) -> bool {
    placed.iter().all(|(other, other_radius)| {
        Vec2::new(at.x - other.x, at.z - other.z).length() >= radius + other_radius
    })
}

/// The live placement: how long the button has been down, where it resolves to,
/// how big that has grown, and whether it will go there.
///
/// The same shape as [`crate::squad::Whistle`], for the same reason: the state
/// belongs to the button rather than to any one entity, and the systems that
/// draw the preview and the system that builds want the same answer.
#[derive(Resource, Default)]
pub struct Build {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    pub scale: f32,
    pub fits: bool,
}

impl Build {
    /// The preview is only shown once the press has outlasted a tap, so a tap
    /// does not flash a ring on the ground on its way to building.
    pub fn showing(&self) -> bool {
        self.held_for.is_some_and(|held| held >= squad::TAP_SECONDS)
    }
}

/// The meshes and materials every machine and every preview is drawn with.
///
/// Built once and shared, exactly like [`crate::weapon::ShotAssets`], and for
/// the same reason: putting a machine down should allocate a handful of
/// entities and nothing else.
///
/// Deliberately **not** under a [`bevy::world_serialization::WorldAssetRoot`].
/// `n64::convert` restyles what it finds inside a loaded scene, and a mote of
/// nuclonium is not level geometry that was lit offline -- it is drawn with the
/// same unlit core and camera-facing glow a ball on the lawn wears, so it keeps
/// its standard material and is left alone. This is why the machine's own model
/// hangs off a *child* of the machine: the walk up to a scene root has to miss
/// the motes and find the .glb.
///
/// There are no mote handles in here. They live in [`crate::nuclonium::Art`],
/// which is the whole point -- what turns in the coils has to be visibly the
/// same substance the Marios carried in, and two modules building a green ball
/// each is two green balls that drift apart the first time one is tuned.
#[derive(Resource, Clone)]
pub struct FieldArt {
    ring: Handle<Mesh>,
    clear: Handle<StandardMaterial>,
    blocked: Handle<StandardMaterial>,
}

/// How many motes one machine will draw before it stops drawing more.
///
/// Five hundred, which is the number that reads as *full*: the flux surface is
/// about forty metres of ring and a mote is a few centimetres across, so at
/// this count they are shoulder to shoulder and the machine is a solid turning
/// band of green. Adding a five hundred and first changes nothing anybody could
/// see and costs two entities, every frame, for the rest of the session.
///
/// The count itself is not capped -- see [`Store`] -- so a player who banks ten
/// thousand has banked ten thousand and the HUD says so. This is a drawing
/// limit and nothing else.
pub const ORBIT_CAP: u32 = 500;

/// How big one mote is against a ball lying on the lawn.
///
/// Much smaller, and it has to be: five hundred at field size would be five
/// hundred one-metre glows inside a machine four metres across, which is not a
/// dense field but a solid green sphere with a stellarator somewhere inside it.
/// At this size a full machine is a band you can see the twist in, and a machine
/// holding three is three distinct sparks going round -- which is the reading
/// the low end has to support just as much as the high end does.
const MOTE_SCALE: f32 = 0.22;

/// How wide the plasma is at its narrowest, measured from its own axis, in the
/// shape's units.
///
/// The tube's cross-section is an ellipse: `section` across and `0.72 * MINOR`
/// tall, and `section` is never smaller than `MINOR * (1.0 - 0.16)`. So the
/// tight direction is the vertical one, and that is the one the motes have to be
/// kept clear of.
const TUBE_HALF: f32 = 0.72 * MINOR;

/// How much room one mote's glow needs, in the shape's units.
///
/// The *glow*, not the little sphere at the middle of it. What the player sees
/// of a mote is a card three times wider than the ball, and a field fitted to
/// the balls is a field of halos hanging out through the coils -- which is
/// exactly what the first version did.
///
/// It covers the trail behind a mote as well, without a second number: a trail
/// is laid along the path -- which is a flux surface, and therefore already
/// inside -- and it is drawn narrower than the glow that makes it (see
/// [`crate::nuclonium::Trail`]), so anything this clears, that clears too.
const MOTE_GLOW: f32 = crate::nuclonium::GLOW_RADIUS * MOTE_SCALE;

/// How far out from the magnetic axis a mote may sit, as a fraction of the way
/// to the plasma's surface.
///
/// **Not one, and it is derived rather than chosen.** The motes ride the flux
/// surface family (see [`flux_point_at`]), and one sitting exactly *on* the
/// outermost surface has half its glow outside it. So they are held back by
/// their own glow's width: the outermost mote's card reaches the skin and stops
/// there, which is what "inside the plasma" means when the thing being contained
/// is light rather than a body.
///
/// Deriving it is what keeps it true. Every term here -- the ball's radius, how
/// far its halo reaches, how big a mote is drawn, how fat the tube is -- belongs
/// to somebody else and any of them can move; written down as a number, this
/// would be right until the first time one of them did.
const MOTE_REACH: f32 = (TUBE_HALF - MOTE_GLOW) / TUBE_HALF;

/// How long a mote takes to swim from the feed point out to its orbit.
///
/// Long enough to be a journey and short enough that a machine taking delivery
/// of a dozen at once is not a machine with a dozen things visibly wandering
/// about in it. See [`Orbit::settling`].
const ARRIVE_SECONDS: f32 = 0.6;
const _: () = assert!(
    MOTE_REACH > 0.0,
    "a mote's glow is wider than the plasma it is supposed to be inside"
);

/// How far a coil rides up and down from where it was modelled, and how often.
///
/// Gentle is the whole specification. Seven centimetres on a machine eight
/// metres across is under a hundredth of it, and once every five and a half
/// seconds is slower than a breath -- you notice it in the corner of your eye
/// while looking at something else, which is what makes the machine read as
/// held together by its field rather than bolted together. Anything you can
/// watch a coil *travel* is a machine falling apart.
const COIL_RISE: f32 = 0.07;
const COIL_HZ: f32 = 0.18;

/// How much of a coil's own drift comes from where in the world it is.
///
/// Without it every machine on the lawn breathes in step, because their coils
/// are the same fifteen nodes out of the same file with the same fifteen
/// phases. A radian every seven metres is enough to put two machines built side
/// by side visibly out of time with each other, and much too gentle to be seen
/// as a wave running round the coils of any one of them.
const COIL_SEPARATION: f32 = 0.15;

/// Builds the shared art and puts the preview up, hidden. Called from startup.
pub fn prepare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> FieldArt {
    let flat = |colour: Color| StandardMaterial {
        base_color: colour,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let art = FieldArt {
        // The whistle's annulus, at unit radius, scaled to the footprint when
        // it is drawn. Shared with the squad so the two rings a player is asked
        // to read are visibly the same kind of mark.
        ring: meshes.add(squad::ring_mesh()),
        clear: materials.add(flat(Color::srgba(0.40, 0.95, 1.00, 0.80))),
        blocked: materials.add(flat(Color::srgba(1.00, 0.35, 0.30, 0.80))),
    };
    let site = commands
        .spawn((
            BuildSite,
            Transform::default(),
            Visibility::Hidden,
            // Nothing about the preview should be lit, and nothing about it
            // should be in anybody else's way.
            bevy::light::NotShadowCaster,
        ))
        .id();
    commands.entity(site).with_child((
        Footprint,
        bevy::light::NotShadowCaster,
        Mesh3d(art.ring.clone()),
        MeshMaterial3d(art.clear.clone()),
        // Just clear of the ground, so the ring is not half-buried in the slope
        // it is drawn on -- the same 5cm the whistle's ring is lifted by.
        // The footprint the machine will actually stand on, off the file.
        Transform::from_xyz(0.0, 0.05, 0.0).with_scale(Vec3::new(
            machine().radius,
            1.0,
            machine().radius,
        )),
    ));
    // No field on the preview, and none on a machine at the moment it is built.
    // A stellarator arrives empty and fills up; what the ring on the ground
    // promises is a footprint, which is the part of the preview that was ever
    // load-bearing.
    //
    // Handed back as well as inserted, because the level that comes up in this
    // same run wants it and an inserted resource does not exist until the next
    // sync point. Nothing but handles, so the clone is free.
    commands.insert_resource(art.clone());
    art
}

/// One mote of nuclonium, as a bundle, seeded off its own index.
///
/// The motes are children of the machine rather than of its model, which is
/// what lets the model be turned upright and scaled without the field having to
/// be un-turned again: everything below the machine is in the generator's own
/// units, and the machine's own uniform scale is the only size in it.
///
/// A function of the index alone, so [`stock`] can add the next one without
/// knowing anything about the ones already turning.
fn mote(art: &crate::nuclonium::Art, index: u32) -> impl Bundle + use<> {
    // One golden-angle walk feeds all four numbers. Successive motes are never
    // at the same place and never at the same speed, and the whole field is a
    // function of how many have arrived.
    let phase = index as f32 * GOLDEN_ANGLE;
    let spread = |scale: f32| (phase * scale).sin().abs();
    (
        Orbit {
            index,
            u: phase,
            rate: 0.65 + 0.70 * spread(0.37),
            v: phase * 2.0,
            wind: 0.80 + 1.10 * spread(0.21),
            // Square-rooted, so the motes are spread evenly over the tube's
            // *area* rather than over its radius -- without it half of them
            // crowd into the middle, where a circle has almost no room.
            rho: MOTE_REACH * spread(0.53).sqrt(),
            // The whole of the arrival is still ahead of it. See
            // [`Orbit::settling`].
            settling: 1.0,
        },
        crate::nuclonium::core(art, crate::nuclonium::Kind::Nuclonium),
        // Placed by `orbit` on the first frame it runs; this is what it wears
        // for the one frame before that.
        Transform::default(),
        Visibility::Hidden,
    )
}

/// Puts one machine in the world: the model, and an empty store.
///
/// No shared art is wanted any more, and its absence is the change: a machine's
/// field is grown out of what it is holding by [`stock`], so there is nothing
/// to build at the moment one goes up.
///
/// Shared by the build button and by anything else that ever wants one, the
/// same way [`crate::squad::spawn_ally`] is shared between the console and the
/// warp pipe.
pub fn spawn(
    commands: &mut Commands,
    assets: &AssetServer,
    at: Vec3,
    yaw: f32,
    scale: f32,
) -> Entity {
    let built = commands
        .spawn((
            Stellarator {
                radius: footprint(scale),
            },
            // Empty. Every unit in it arrived down a beam -- see
            // [`crate::nuclonium::ship`] -- so a machine that has just gone up
            // is a machine with a dark field, which is the truth about it.
            Store::default(),
            Transform::from_translation(at)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::splat(scale)),
            Visibility::default(),
        ))
        .id();
    commands.entity(built).with_child((
        bevy::world_serialization::WorldAssetRoot(assets.load(MODEL)),
        // Nothing. The model is upright in the file and stands on its own
        // origin, so where the machine is put is where it goes -- and this
        // child exists only to give `n64::convert` a scene root to walk down
        // from that the wisps are not under. Both corrections that used to be
        // here, the quarter turn about X and the lift, are the generator's now.
        Transform::default(),
    ));
    built
}

/// Everything the preview and the machines both need to be kept off each
/// other's `Transform`.
///
/// Every exclusion is load-bearing for the same reason [`crate::squad`]'s
/// `CircleQuery` documents: Bevy proves two queries disjoint from their filters
/// alone, so a mutable `Transform` that does not name the other `Transform`
/// queries in its own system is a schedule that refuses to build -- which, in a
/// windowed build, is a game that opens and shuts without a word.
type SiteQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<BuildSite>,
        Without<Player>,
        Without<Camera3d>,
        Without<Stellarator>,
    ),
>;

/// The build button: held opens a site, released builds on it.
///
/// Runs at the render rate rather than on the fixed step, for
/// [`crate::squad::whistle`]'s reason -- the machine grows with wall-clock time
/// and the preview is drawn every frame -- and it takes the released edge the
/// same latched way, so a press is neither lost nor counted twice across the
/// fixed-step boundary.
#[allow(clippy::too_many_arguments)]
pub fn place(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut input: ResMut<InputState>,
    level: Res<LevelData>,
    art: Res<FieldArt>,
    mut build: ResMut<Build>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
    player: Query<&Transform, With<Player>>,
    placed: Query<(&Transform, &Stellarator)>,
    mut site: SiteQuery,
    mut visibility: Query<&mut Visibility, With<BuildSite>>,
    mut ring: Query<&mut MeshMaterial3d<StandardMaterial>, With<Footprint>>,
) {
    let (Ok(camera), Ok(leader)) = (camera.single(), player.single()) else {
        return;
    };
    let released = InputState::take(&mut input.build_released);
    if input.build || released {
        // Refreshed on the press as well as on the hold, so a tap too short to
        // have opened a site still builds somewhere.
        build.aim = squad::aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
        );
        let held = if input.build {
            let held = build.held_for.unwrap_or(0.0) + time.delta_secs();
            build.held_for = Some(held);
            held
        } else {
            build.held_for.unwrap_or(0.0)
        };
        build.scale = build_scale(held);
        let taken: Vec<_> = placed
            .iter()
            .map(|(transform, machine)| (transform.translation, machine.radius))
            .collect();
        build.fits = fits(build.aim, footprint(build.scale), &taken);
    }
    if released {
        let held = build.held_for.take().unwrap_or(0.0);
        if build.fits {
            // Turned to face whoever built it. The machine is very nearly
            // symmetrical about its axis, so this is a courtesy rather than a
            // decision -- but a field of them all facing the same compass point
            // reads as a texture rather than as things somebody put there.
            let away = build.aim - leader.translation;
            let yaw = away.x.atan2(away.z);
            spawn(&mut commands, &assets, build.aim, yaw, build_scale(held));
        }
    }
    let showing = build.showing();
    if let Ok(mut transform) = site.single_mut() {
        if showing {
            transform.translation = build.aim;
            transform.scale = Vec3::splat(build.scale);
        }
    }
    if let Ok(mut visible) = visibility.single_mut() {
        *visible = if showing {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let Ok(mut material) = ring.single_mut() {
        let wanted = if build.fits { &art.clear } else { &art.blocked };
        if material.0.id() != wanted.id() {
            material.0 = wanted.clone();
        }
    }
}

/// Takes the two things this module wants out of a machine's scene as it
/// arrives: the plasma skin, which is hidden, and the coils, which are set
/// floating.
///
/// By name, the way [`crate::weapon::claim`] finds the socket and the muzzle,
/// because a glTF node is the only thing this port and the generator have
/// agreed on. The plasma is hidden rather than despawned: the entity is a
/// scene's own, and removing it out from under Bevy's loader is a fight this
/// does not need to pick to stop a surface being drawn.
///
/// `try_insert` rather than `insert` for [`crate::animation::claim_players`]'s
/// reason -- a scene can be despawned by a level change between the query and
/// the flush, and a scene that has gone has nothing to claim.
///
/// Runs in the overlay rather than with the rest of the presentation. A scene
/// finishes loading whenever it finishes loading, and the console being open at
/// that moment must not be the difference between a machine with a blue balloon
/// in it and one without.
pub fn claim(
    mut commands: Commands,
    arrivals: Query<(Entity, &Name, Option<&Transform>), Added<Name>>,
) {
    for (entity, name, transform) in &arrivals {
        if name.as_str() == PLASMA_NODE {
            commands.entity(entity).try_insert(Visibility::Hidden);
            continue;
        }
        let Some(number) = name.as_str().strip_prefix(COIL_NODE) else {
            continue;
        };
        let Ok(index) = number.trim().parse::<u32>() else {
            continue;
        };
        // The golden angle again, off the coil's own number rather than off the
        // order the loader happened to hand them over in, so the same machine
        // drifts the same way every run. Fifteen coils stepped by it are
        // fifteen phases that never line up into a wave, which is what keeps
        // this a machine breathing rather than a machine pulsing.
        let phase = index as f32 * GOLDEN_ANGLE;
        commands.entity(entity).try_insert(Coil {
            rest: transform.map_or(Vec3::ZERO, |transform| transform.translation),
            phase,
            rate: 0.75 + 0.5 * (phase * 0.31).sin().abs(),
        });
    }
}

/// Floats every coil, gently, on its own clock.
///
/// The rise is written along the coil's **local Z**, which is the machine's own
/// axis. `tools/generate_stellarator.py` writes Z-up coordinates into a Y-up
/// file, so the model is stood upright by a quarter turn about X on the entity
/// above it -- see [`spawn`] -- and inside that turn the machine's up is still
/// the generator's Z. Writing world Y here would slide the coils sideways out
/// of the frame instead, which is exactly what it looks like.
///
/// The position term is read off the coil's `GlobalTransform`, which in `Update`
/// is last frame's. That is fine and deliberate: it is a *phase*, it only uses
/// the horizontal axes, and the drift this system writes does not move them.
pub fn float_coils(time: Res<Time>, mut coils: Query<(&Coil, &GlobalTransform, &mut Transform)>) {
    let now = time.elapsed_secs();
    for (coil, world, mut transform) in &mut coils {
        let here = world.translation();
        let phase = coil.phase + (here.x + here.z) * COIL_SEPARATION;
        let drift = (now * COIL_HZ * std::f32::consts::TAU * coil.rate + phase).sin();
        // Scaled with the file like everything else: seven centimetres on a
        // machine re-exported at a third the size is a coil visibly climbing
        // out of its own frame.
        transform.translation = coil.rest + Vec3::Z * COIL_RISE * machine().scale * drift;
    }
}

/// Grows and shrinks each machine's field to match what it is holding.
///
/// One mote per unit of nuclonium in the store, up to [`ORBIT_CAP`]. Nothing
/// here re-lays the field: a mote's place on the flux surface is a function of
/// its index alone (see [`mote`]), so the arrival that takes a machine from
/// ninety-nine to a hundred spawns one entity and disturbs nothing, and the
/// hundredth mote lands in the gap the first ninety-nine left.
///
/// `Store::shown` is written here and nowhere else, and it is stepped *before*
/// the spawn it stands for has happened. That is deliberate: `Commands` are
/// deferred to the next sync point, so a system that counted its own children
/// instead would spawn the same mote again on every frame until the queue
/// flushed -- which, at sixty frames a second, is a machine that eats the world
/// because a ball arrived.
///
/// Shrinking is the same loop backwards and is unreachable today -- nothing
/// spends what a machine is holding. It is written anyway because the first
/// thing that spends it will be a cost taken in one place, and a field that
/// stayed at its high-water mark would be a machine reporting money it no
/// longer has.
pub fn stock(
    mut commands: Commands,
    art: Res<crate::nuclonium::Art>,
    mut machines: Query<(Entity, &mut Store)>,
    motes: Query<(Entity, &Orbit, &ChildOf)>,
) {
    for (machine, mut store) in &mut machines {
        let wanted = Store::drawn(store.held);
        while store.shown < wanted {
            let index = store.shown;
            let born = commands.spawn((mote(&art, index), ChildOf(machine))).id();
            // The same glow a ball on the lawn wears, aimed at the camera every
            // frame by `nuclonium::shimmer` along with all the others -- there
            // is nothing stellarator-shaped about a mote's appearance, and that
            // is the point of it looking like what the Marios carried in.
            commands.entity(born).with_child(crate::nuclonium::halo(
                &art,
                crate::nuclonium::Kind::Nuclonium,
                index as f32 * GOLDEN_ANGLE,
            ));
            store.shown += 1;
        }
        if store.shown > wanted {
            // Everything from `wanted` up, taken by index rather than by
            // whatever order the query hands them over in: what has to be left
            // behind is the *prefix* of the sequence `mote` lays down, because
            // that is the part that is evenly spread.
            for (spare, _, _) in motes
                .iter()
                .filter(|(_, mote, parent)| parent.parent() == machine && mote.index >= wanted)
            {
                commands.entity(spare).despawn();
            }
            store.shown = wanted;
        }
    }
}

/// Turns every mote in every machine: round the ring, and round the tube as it
/// goes.
///
/// Reads the wall clock rather than counting frames, so the field turns at the
/// same rate whatever the frame rate is. Both sliders are read live: what a
/// machine looks like at a given speed and mote size is not something either
/// number predicts, which is the same argument `tracer_width` is on a slider
/// for.
pub fn orbit(
    time: Res<Time>,
    tuning: Res<GameTuning>,
    mut motes: Query<(&mut Orbit, &mut Transform, &mut Visibility)>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    // Everything below is in the *file's* units rather than in the shape's,
    // which is what keeps the field inside the coils when the model is
    // re-exported at another `--scale`.
    let size = machine().scale;
    for (mut mote, mut transform, mut visible) in &mut motes {
        let u = mote.u + now * tuning.stellarator_spin * mote.rate;
        let v = mote.v + now * tuning.stellarator_spin * mote.wind;
        // Lifted with the machine, so a mote is inside the coils rather than
        // buried in the lawn they are standing on.
        let axis = Vec3::Y * machine().lift;
        let riding = flux_point_at(u, v, mote.rho) * size + axis;
        // Still swimming out from where its shipment landed, if it only just
        // got here. Smoothed at both ends rather than a straight lerp, so it
        // leaves the feed point and joins its orbit without a corner at either.
        // See [`Orbit::settling`].
        mote.settling = (mote.settling - dt / ARRIVE_SECONDS).max(0.0);
        let along = 1.0 - mote.settling;
        let centre = axis.lerp(riding, along * along * (3.0 - 2.0 * along));
        *transform = Transform::from_translation(centre)
            .with_scale(Vec3::splat(MOTE_SCALE * size * tuning.stellarator_glow));
        // Hidden until it has been put somewhere, so a mote never flashes at
        // the machine's origin -- down on the lawn -- for the frame between
        // being spawned and being placed.
        if *visible == Visibility::Hidden {
            *visible = Visibility::Inherited;
        }
    }
}

/// Carries out `nuclonium store <n>` from the console.
///
/// Sets what every machine on the map is holding, which [`stock`] then grows or
/// sheds a field to match on the next frame. In the overlay with the rest of
/// the console's requests, for [`crate::pylon::command`]'s reason: the console
/// is open at the moment the line is typed.
pub fn command(mut console: ResMut<crate::console::ConsoleState>, mut machines: Query<&mut Store>) {
    for request in console.take_requests() {
        let crate::console::Request::Stock(held) = request else {
            console.defer(request);
            continue;
        };
        for mut store in &mut machines {
            store.held = held;
        }
    }
}

/// Aiming and drawing, in the order one frame does them.
pub fn systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (place, stock, orbit, float_coils).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holding the button aims the machine. It does not size it.
    #[test]
    fn every_machine_the_button_builds_is_the_same_size() {
        // A tap, a hold and leaning on the key for a minute all build the same
        // machine. The hold aims it; it does not size it.
        for held in [0.0, squad::TAP_SECONDS, 0.6, 1.4, 60.0] {
            assert_eq!(build_scale(held), BUILD_SCALE, "held for {held}");
        }
        // And the one size is the size the file was authored at, so the
        // generator's `--scale` is the last word on how big one can be.
        assert_eq!(footprint(build_scale(2.0)), machine().radius);
    }

    /// Two machines may stand beside each other and may not stand through each
    /// other, and how high up they are has nothing to do with it.
    #[test]
    fn a_site_is_clear_only_when_the_footprints_miss() {
        let there = Vec3::new(0.0, 0.0, 0.0);
        let radius = machine().radius;
        let placed = [(there, radius)];
        // Touching rims is allowed; anything nearer is not.
        assert!(fits(Vec3::new(radius * 2.0, 0.0, 0.0), radius, &placed));
        assert!(!fits(
            Vec3::new(radius * 2.0 - 0.1, 0.0, 0.0),
            radius,
            &placed
        ));
        // Straight up a cliff face from one is still through it: the support
        // rings are in the same column of air.
        assert!(!fits(Vec3::new(0.0, 40.0, 0.0), radius, &placed));
        // And an empty field takes anything.
        assert!(fits(there, radius, &[]));
    }

    /// The plasma is inside the machine.
    ///
    /// The wisps ride a surface transcribed by hand out of a Python generator,
    /// into a different set of axes, and the two ways that goes wrong are both
    /// invisible in a headless run and glaring on screen: a field threading
    /// outside the coils, or one lying in the lawn because the quarter turn
    /// went the other way. Both are caught by asking where the surface is.
    #[test]
    fn the_field_stays_inside_the_coils_and_above_the_ground() {
        let mut samples = 0;
        for i in 0..97 {
            for j in 0..31 {
                let u = i as f32 / 97.0 * std::f32::consts::TAU;
                let v = j as f32 / 31.0 * std::f32::consts::TAU;
                let point = flux_point(u, v);
                // The coils are swept at 1.13 from the ring's centre line; the
                // plasma's own section is under one. Anything outside that is
                // a field drawn through the copper.
                let radius = Vec2::new(point.x, point.z).length();
                assert!(
                    (radius - MAJOR).abs() < 1.13,
                    "u={u} v={v} lands {radius} out"
                );
                // And the machine is lifted by what it measures, so a surface
                // that reached further than that from the centre plane would be
                // under the lawn. Compared in the file's units, which is where
                // both numbers end up.
                let high = point.y * machine().scale;
                assert!(
                    high.abs() < machine().lift,
                    "u={u} v={v} is {high} high against a lift of {}",
                    machine().lift
                );
                samples += 1;
            }
        }
        assert_eq!(samples, 97 * 31);
    }

    /// Every mote, glow and all, is inside the plasma.
    ///
    /// The thing being contained is the *card*, not the little sphere at the
    /// middle of it -- a field fitted to the balls hangs its halos out through
    /// the coils, which is what the first version of this did and what it was
    /// reported as. So the glow is treated as a box around each mote and the
    /// whole box has to be inside the tube's own ellipse.
    ///
    /// Swept over the surface rather than over the motes an actual machine
    /// happens to have spawned, because what has to hold is the *rule*: any
    /// `(u, v)` at [`MOTE_REACH`], which is the furthest out one can be put.
    #[test]
    fn a_motes_glow_stays_inside_the_plasma_it_is_confined_by() {
        let mut samples = 0;
        for i in 0..89 {
            for j in 0..37 {
                let u = i as f32 / 89.0 * std::f32::consts::TAU;
                let v = j as f32 / 37.0 * std::f32::consts::TAU;
                let twist = PERIODS * u;
                let section = MINOR * (1.0 + 0.16 * twist.cos());
                // The tube's own axis at this angle, and the mote's offset from
                // it -- which is the pair `flux_point_at` interpolates between.
                let axis = flux_point_at(u, v, 0.0);
                let mote = flux_point_at(u, v, MOTE_REACH);
                let across =
                    Vec2::new(mote.x, mote.z).length() - Vec2::new(axis.x, axis.z).length();
                let along = mote.y - axis.y;
                assert!(
                    (across.abs() + MOTE_GLOW) <= section,
                    "u={u} v={v}: a glow reaches {} out of a tube {section} wide",
                    across.abs() + MOTE_GLOW
                );
                assert!(
                    (along.abs() + MOTE_GLOW) <= TUBE_HALF,
                    "u={u} v={v}: a glow reaches {} up a tube {TUBE_HALF} tall",
                    along.abs() + MOTE_GLOW
                );
                samples += 1;
            }
        }
        assert_eq!(samples, 89 * 37);
        // And the axis really is the middle: with `rho` at nothing there is no
        // `v` left in the answer, which is what makes the interpolation above a
        // radius rather than a fudge.
        for v in [0.0, 1.1, 2.7, 5.5] {
            let point = flux_point_at(0.3, v, 0.0);
            assert!((point - flux_point_at(0.3, 0.0, 0.0)).length() < 1e-5);
        }
        // `flux_point` is still exactly the skin, so the older test that pins
        // the surface against the coils is still pinning the same thing.
        assert_eq!(flux_point(1.2, 2.3), flux_point_at(1.2, 2.3, 1.0));
    }

    /// A machine draws what it is holding, up to the cap, and goes on counting
    /// past it.
    ///
    /// The arithmetic on its own, because the two halves fail in ways that look
    /// nothing alike: a cap applied to `held` is a player whose bank quietly
    /// stops going up at five hundred, and a cap not applied to the field is a
    /// machine that spawns two entities a ball for the rest of the session.
    #[test]
    fn the_field_stops_growing_long_before_the_count_does() {
        assert_eq!(Store::drawn(0), 0, "an empty machine draws an empty field");
        assert_eq!(Store::drawn(1), 1);
        assert_eq!(Store::drawn(ORBIT_CAP - 1), ORBIT_CAP - 1);
        assert_eq!(Store::drawn(ORBIT_CAP), ORBIT_CAP);
        // Past the cap the field stands still.
        assert_eq!(Store::drawn(ORBIT_CAP + 1), ORBIT_CAP);
        assert_eq!(Store::drawn(50_000), ORBIT_CAP);
        // And the count itself is untouched -- there is no ceiling on `held`,
        // which is the whole distinction the two fields exist to make.
        let store = Store {
            held: 50_000,
            shown: Store::drawn(50_000),
        };
        assert_eq!(store.held, 50_000);
    }

    /// Nuclonium arriving at a machine ends up turning inside it, and stops
    /// arriving on screen at the cap.
    ///
    /// Through [`stock`] itself rather than through the whole game, because
    /// what is being checked is the bookkeeping: that `shown` is stepped by the
    /// system rather than by counting children -- `Commands` are deferred, so a
    /// system that counted would spawn the same mote again every frame until
    /// the queue flushed -- and that a machine handed more than the cap stops.
    #[test]
    fn a_machine_grows_a_field_out_of_what_it_is_holding() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(Assets::<Image>::default());
        // The motes are drawn out of `nuclonium`'s own art, which is the point
        // of them: what turns in the coils is the substance the Marios carried
        // in. So the real builder is run rather than a stand-in.
        world
            .run_system_once(
                |mut commands: Commands,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut materials: ResMut<Assets<StandardMaterial>>,
                 mut images: ResMut<Assets<Image>>| {
                    crate::nuclonium::prepare(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                    );
                },
            )
            .expect("the art would not build");
        let machine = world
            .spawn((Stellarator { radius: 2.0 }, Store::default()))
            .id();

        // Nothing held, nothing drawn.
        world.run_system_once(stock).expect("stock would not run");
        let mut motes = world.query::<&Orbit>();
        assert_eq!(motes.iter(&world).count(), 0);

        // Three arrive.
        world.get_mut::<Store>(machine).unwrap().held = 3;
        world.run_system_once(stock).expect("stock would not run");
        let mut motes = world.query::<&Orbit>();
        assert_eq!(motes.iter(&world).count(), 3);
        // Running again adds nothing. This is the deferred-command trap: the
        // three above did not exist yet when `shown` was stepped past them.
        world.run_system_once(stock).expect("stock would not run");
        let mut motes = world.query::<&Orbit>();
        assert_eq!(
            motes.iter(&world).count(),
            3,
            "it spawned the same field twice"
        );

        // And far past the cap it draws exactly the cap.
        world.get_mut::<Store>(machine).unwrap().held = ORBIT_CAP + 250;
        world.run_system_once(stock).expect("stock would not run");
        let mut motes = world.query::<&Orbit>();
        assert_eq!(motes.iter(&world).count(), ORBIT_CAP as usize);
        assert_eq!(
            world.get::<Store>(machine).unwrap().held,
            ORBIT_CAP + 250,
            "the drawing limit was applied to the count"
        );
    }

    /// A mote swims out from where its shipment landed rather than appearing on
    /// its orbit.
    ///
    /// The last link in "nuclonium never changes place without travelling". A
    /// shipment ends its flight at this machine's feed point -- the middle of
    /// the coils -- and stops existing; if the mote it becomes were placed on
    /// its flux surface on the first frame, the substance the player watched
    /// cross the valley would finish by blinking from one point to another. See
    /// [`Orbit::settling`].
    #[test]
    fn a_mote_swims_out_from_the_feed_point_it_arrived_at() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(GameTuning::default());
        let mut clock: Time = Time::default();
        clock.advance_by(std::time::Duration::from_millis(16));
        world.insert_resource(clock);
        let mote = world
            .spawn((
                Orbit {
                    index: 0,
                    u: 0.0,
                    rate: 1.0,
                    v: 0.0,
                    wind: 1.0,
                    rho: MOTE_REACH,
                    settling: 1.0,
                },
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        // Where the flight ends, in the machine's own frame: `feed_point` in
        // `pylon` is this same lift, taken in the world.
        let feed = Vec3::Y * machine().lift;
        world.run_system_once(orbit).expect("orbit would not run");
        let landed = world.get::<Transform>(mote).unwrap().translation;
        assert!(
            landed.distance(feed) < 0.05,
            "a mote appeared {} m from the point its shipment landed at",
            landed.distance(feed)
        );
        // And it is on its orbit shortly afterwards, rather than drifting in
        // for ever.
        for _ in 0..(ARRIVE_SECONDS / 0.016) as usize + 2 {
            world.run_system_once(orbit).expect("orbit would not run");
        }
        assert_eq!(
            world.get::<Orbit>(mote).unwrap().settling,
            0.0,
            "the arrival never finished"
        );
        let riding = world.get::<Transform>(mote).unwrap().translation;
        assert!(
            riding.distance(feed) > 0.05,
            "the mote never left the middle of the machine"
        );
    }

    /// The two names this module and the generator have agreed on, and what
    /// each one gets.
    ///
    /// A unit test on the rule rather than on a loaded .glb, because what can
    /// break here is the *matching*: a generator that renames a node, or a
    /// prefix test that starts catching the support rings as well. Either one
    /// is silent -- a machine with a blue balloon in it, or a machine whose
    /// frame drifts away from its coils -- and neither is reachable from any
    /// other test in this file.
    #[test]
    fn the_plasma_is_hidden_and_only_the_coils_are_set_floating() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let plasma = world
            .spawn((Name::new(PLASMA_NODE), Transform::default()))
            .id();
        // Not at the origin, so a `rest` that was assumed rather than read
        // would move it.
        let coil = world
            .spawn((
                Name::new("Modular Coil 07"),
                Transform::from_xyz(0.1, 0.2, 0.3),
            ))
            .id();
        let ring = world
            .spawn((Name::new("Support Ring 1"), Transform::default()))
            .id();
        world.run_system_once(claim).expect("claim would not run");

        assert!(
            matches!(world.get::<Visibility>(plasma), Some(Visibility::Hidden)),
            "the plasma skin is still being drawn"
        );
        let floating = world.get::<Coil>(coil).expect("the coil was not claimed");
        assert_eq!(floating.rest, Vec3::new(0.1, 0.2, 0.3));
        assert!(floating.rate > 0.0);
        // The frame is the frame. A machine whose support rings drift away from
        // the coils bolted between them is not floating, it is coming apart.
        assert!(world.get::<Coil>(ring).is_none(), "a support ring floats");
        assert!(world.get::<Coil>(plasma).is_none());
        assert!(world.get::<Visibility>(ring).is_none());
    }

    /// A coil rides its own axis and nothing else's.
    ///
    /// The one mistake here that no headless test would otherwise catch: the
    /// machine is stood upright by a quarter turn on the entity above the
    /// coils, so inside that turn "up" is local **Z**, and a drift written
    /// along Y slides the coils out of the machine sideways. Both look like a
    /// number in a `sin` from in here; only the axis says which.
    #[test]
    fn a_coil_floats_along_the_machines_axis_and_stays_near_where_it_was_put() {
        use bevy::ecs::system::RunSystemOnce;
        let rest = Vec3::new(1.5, -0.25, 0.75);
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        let coil = world
            .spawn((
                Coil {
                    rest,
                    phase: 0.4,
                    rate: 1.0,
                },
                Transform::from_translation(rest),
                GlobalTransform::from_translation(rest),
            ))
            .id();
        // The drift is scaled with the file like everything else, so what the
        // coil may travel is the constant times whatever the model was baked at.
        let rise = COIL_RISE * machine().scale;
        let mut travelled: f32 = 0.0;
        // A whole period of the drift, sampled: whatever phase it starts on, it
        // has been somewhere over the course of one.
        for _ in 0..40 {
            world
                .resource_mut::<Time<()>>()
                .advance_by(std::time::Duration::from_millis(150));
            world
                .run_system_once(float_coils)
                .expect("it would not run");
            let at = world.get::<Transform>(coil).unwrap().translation;
            assert_eq!(
                (at.x, at.y),
                (rest.x, rest.y),
                "the coil left the machine's axis"
            );
            travelled = travelled.max((at.z - rest.z).abs());
            assert!(
                (at.z - rest.z).abs() <= rise + 1e-5,
                "the coil rode {} from where it was modelled",
                at.z - rest.z
            );
        }
        assert!(
            travelled > rise * 0.9,
            "the coil never really moved: {travelled} against a rise of {rise}"
        );
    }

    /// The machine measures what the file says it is, and the numbers the rest
    /// of the module hangs off are sane.
    ///
    /// The scale is the load-bearing one: it is a ratio, and a ratio that comes
    /// out slightly wrong is a plasma that sits slightly inside the coils and a
    /// footprint ring slightly out -- which nobody reports as a bug, they just
    /// think the effect looks a bit off. It came out at 0.96 for a file baked at
    /// 1.0 exactly once, from measuring a low-poly mesh's bounding box, and this
    /// is what would have caught it.
    #[test]
    fn the_machine_is_measured_off_the_file_it_is_drawn_from() {
        let measured = machine();
        assert!(measured.scale > 0.0, "{measured:?}");
        // The drift catcher, and the only assertion here that does not survive
        // a `--scale`: the footprint is measured off the machine's widest part
        // and the scale off a plasma vertex, so their ratio is a property of the
        // *shape* rather than of how big the file was baked.
        //
        // The widest part is the coils rather than the support rings, which is
        // worth stating because it is not what the machine looks like: a coil
        // sweeps to `MAJOR + 1.13 + 0.2` with a tube of 0.105 round it and
        // bulges thirteen centimetres through rings that stop at 4.30. It is
        // `SHAPE_WIDTH / 2` in `tools/generate_stellarator.py`, and the two have
        // to agree or the generator's `MACHINE_WIDTH` is not the width it says.
        //
        // It comes out wrong the moment either measurement starts approximating,
        // which is how a bounding-box read of a low-poly mesh once put the scale
        // at 0.96 for a file baked at 1.0.
        const WIDEST: f32 = MAJOR + 1.13 + 0.2 + 0.105;
        let ratio = measured.radius / measured.scale;
        assert!(
            (ratio - WIDEST).abs() < WIDEST * 0.03,
            "the machine measures {ratio} across in shape units, not {WIDEST}: \
             either tools/generate_stellarator.py changed shape -- and MAJOR, \
             MINOR, PERIODS and its own SHAPE_WIDTH have to follow it -- or a \
             measurement here has drifted"
        );
        // It is a wide flat machine and it rests on the ground. Both are
        // properties of the shape rather than of its size, which is the point:
        // how big a stellarator is is the author's to change in
        // `tools/generate_stellarator.py`, and a test that failed when they
        // changed it would be the same coupling this measurement exists to
        // remove.
        assert!(measured.lift > 0.0, "{measured:?}");
        assert!(measured.radius > measured.lift * 2.0, "{measured:?}");
    }

    // There is no test here comparing `flux_point` against the vertices in
    // `assets/actors/stellarator.glb`. There was, and it locked the order
    // `tools/generate_stellarator.py` happens to write its samples in to the
    // arithmetic in this file: regenerating the machine at a different
    // resolution -- which is a thing the author is meant to be able to do --
    // failed a test on an asset that was perfectly correct. Blender and the
    // generator are the source of truth for the shape; what this module needs
    // to be right about is measured from the file it loads, in
    // `the_machine_is_measured_off_the_file_it_is_drawn_from` above.

    /// The twist is the machine's, not a torus's.
    ///
    /// A plain torus would give the same section at every angle round the ring;
    /// what makes this a stellarator is that it does not. If the field periods
    /// ever fall out of the transcription this is what notices.
    #[test]
    fn the_surface_twists_as_it_goes_round() {
        let period = std::f32::consts::TAU / PERIODS;
        let here = flux_point(0.0, 0.0);
        let quarter = flux_point(period * 0.25, 0.0);
        let whole = flux_point(period, 0.0);
        // A quarter of one field period along, the section has visibly moved.
        assert!(
            (Vec2::new(here.x, here.z).length() - Vec2::new(quarter.x, quarter.z).length()).abs()
                > 0.05,
            "the section did not change across a quarter period"
        );
        // A whole one along, it is back where it started -- which is what
        // "field period" means.
        assert!(
            (Vec2::new(here.x, here.z).length() - Vec2::new(whole.x, whole.z).length()).abs()
                < 1e-4,
            "the twist did not close over one field period"
        );
    }
}
