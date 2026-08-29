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
//! **The plasma is not the plasma mesh.** `tools/generate_stellarator.py`
//! writes a `Twisted Plasma Surface` into the .glb -- a closed, faintly
//! transparent skin sitting inside the coils -- and this module hides it the
//! frame it arrives (see [`claim`]). Drawn, it is a blue balloon: a single
//! blended shell with no depth to it, which reads as a solid object wedged in
//! the machine rather than as confined plasma, and which this renderer's one
//! diffuse light has nothing interesting to say about.
//!
//! What is drawn instead is [`Wisp`]s: short glowing streaks riding the *same*
//! flux surface the mesh describes, marching round the ring, winding around the
//! tube as they go, and flaring and dying on their own clocks. [`flux_point`]
//! is the generator's own parametrisation transcribed, so the light is where
//! the field is rather than near it, and the twist you can see chasing round
//! the coils is the machine's real field periods.
//!
//! Nothing here is random. Every wisp's phase, speed and pulse comes off
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
            let accessor = &json["accessors"]
                [primitive["attributes"]["POSITION"].as_u64()? as usize];
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

/// One streak of plasma, and the four clocks that make it its own.
///
/// `u` runs round the ring the long way and `v` round the tube; `rate` and
/// `wind` are how fast this one does each, and `pulse` is where it is in its
/// own fade. All five are seeded off the golden angle in [`add_field`].
#[derive(Component)]
pub struct Wisp {
    u: f32,
    rate: f32,
    v: f32,
    wind: f32,
    pulse: f32,
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
    let twist = PERIODS * u;
    let section = MINOR * (1.0 + 0.16 * twist.cos());
    let radius = MAJOR + section * v.cos() + 0.16 * twist.cos();
    let height = 0.72 * MINOR * v.sin() + 0.18 * twist.sin();
    Vec3::new(radius * u.cos(), height, -(radius * u.sin()))
}

// -- placing ----------------------------------------------------------------

/// The smallest machine a tap puts down, the largest a full hold reaches, and
/// how long the hold takes to cross between them.
///
/// The growth is [`crate::squad::circle_radius`]'s shape and it starts from the
/// same [`crate::squad::TAP_SECONDS`], so one button in the player's hands
/// behaves one way: nothing happens for the length of a tap, and then the thing
/// under the crosshair opens out.
///
/// **The top of the range is the authored size and not a multiple of it.** The
/// hold picks how much of a machine to build, never how much *more* than one,
/// so `--scale` in the generator is the last word on how big a stellarator can
/// be. It stopped being that for a while: a cap of 1.6 meant re-exporting the
/// model at half size still left a fourteen-metre machine reachable, and the
/// only way to find that out was to build one.
const SCALE_MIN: f32 = 0.5;
const SCALE_MAX: f32 = 1.0;
const GROW_SECONDS: f32 = 1.2;

/// How much ground a machine built at this size stands on.
///
/// The one place the footprint is turned into a radius, so the ring drawn on
/// the ground, the overlap test and anything asking after the fact all get the
/// same number -- and all three come off [`machine`], so all three follow the
/// file.
pub fn footprint(scale: f32) -> f32 {
    machine().radius * scale
}

/// How big the machine has grown after the button has been down this long.
pub fn build_scale(held_for: f32) -> f32 {
    let grown = ((held_for - squad::TAP_SECONDS) / GROW_SECONDS).clamp(0.0, 1.0);
    SCALE_MIN + (SCALE_MAX - SCALE_MIN) * grown
}

/// Is there room for a machine of this size here?
///
/// Flat distance against the sum of the two footprints, which is the same
/// question the ring on the ground is drawing. Height is deliberately not in
/// it: two machines on ledges ten metres apart vertically still have their
/// support rings in the same column of air, and a rule that let them through
/// would put one through the other.
pub fn fits(at: Vec3, radius: f32, placed: &[(Vec3, f32)]) -> bool {
    placed
        .iter()
        .all(|(other, other_radius)| {
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
/// `n64::convert` restyles what it finds inside a loaded scene, and a wisp is
/// not level geometry that was lit offline -- it is a light source drawn as a
/// flat bright streak, so it keeps its standard material and is left alone.
/// This is why the machine's own model hangs off a *child* of the machine: the
/// walk up to a scene root has to miss the wisps and find the .glb.
#[derive(Resource, Clone)]
pub struct FieldArt {
    wisp: Handle<Mesh>,
    ring: Handle<Mesh>,
    /// Dim to hot, and what a wisp's brightness is quantised onto.
    ladder: Vec<Handle<StandardMaterial>>,
    clear: Handle<StandardMaterial>,
    blocked: Handle<StandardMaterial>,
}

/// How many streaks one machine's field is made of.
///
/// Sixty-four is what it takes for the ring to read as *confined plasma*
/// rather than as sparks in a cage. Half that and the eye follows individual
/// streaks around the machine and counts them, which is a very different
/// picture: what should look like one thing turning looks like thirty things
/// orbiting.
pub const WISPS: usize = 64;

/// How many brightnesses there are to be drawn at.
///
/// A ladder of shared materials rather than one material per wisp whose alpha
/// is written every frame. Thirty-two streaks a machine is thirty-two uniform
/// buffers to keep rewriting, and the difference between one step of this
/// ladder and the next is not something you can see on a streak five
/// centimetres wide: what carries the fade smoothly is the *length*, which is
/// in the wisp's own transform and costs nothing.
const LADDER: usize = 8;

/// How long a streak is at its dimmest and at its brightest, in model units,
/// and how thick.
///
/// The length is what carries the fade -- see [`LADDER`] -- so the two ends of
/// it are far apart on purpose: a dying wisp shrinks to a spark and a flaring
/// one draws a real arc through the coils.
///
/// The width was five and a half centimetres and had to grow. A machine is
/// looked at from across a lawn as often as from inside it, and at twenty
/// metres that was under two pixels: a field that disappeared at exactly the
/// distance the player usually stands. Nine centimetres holds up out to the
/// far side of the castle grounds and is still thin enough up close to be a
/// filament rather than a bar.
const WISP_SHORT: f32 = 0.18;
const WISP_LONG: f32 = 1.25;
const WISP_WIDTH: f32 = 0.09;

/// How much of the ring apart the two samples the streak is aimed along are.
const TANGENT: f32 = 0.02;

/// How many times a second a wisp flares, before its own rate is applied.
const FLARE_HZ: f32 = 0.55;

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

/// How sharply a flare falls away either side of its peak.
///
/// A plain sine spends half its time bright, which is a ring of solid light.
/// Raised, it spends most of it near nothing and passes briefly through hot,
/// which is what makes the field read as something moving through the coils
/// rather than as a lamp bolted inside them. Above three the ring is mostly
/// dark and the flares read as flashes going off; this is the far side of
/// that, where at any moment a good third of the field is showing.
const FLARE_SHARPNESS: f32 = 2.2;

/// Builds the shared art and puts the preview up, hidden. Called from startup.
pub fn prepare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> FieldArt {
    let ladder = (0..LADDER)
        .map(|step| {
            let heat = (step + 1) as f32 / LADDER as f32;
            // Blue throughout, and never all the way to white. The red
            // channel comes in on the cube, so it is still nearly absent
            // three-quarters of the way up the ladder and reaches only two
            // thirds at the top: a hot wisp is a pale cyan filament with a
            // blue body, which is what plasma looks like. Taking red to one
            // put white sticks inside the coils -- the machine's own colour
            // fell out of the effect at exactly the brightness you look at.
            let colour = Color::srgb(
                0.05 + 0.55 * heat * heat * heat,
                0.42 + 0.48 * heat,
                1.0,
            );
            materials.add(StandardMaterial {
                base_color: colour.with_alpha(0.10 + 0.80 * heat),
                emissive: colour.to_linear() * (2.0 + 10.0 * heat),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .collect();
    let flat = |colour: Color| StandardMaterial {
        base_color: colour,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let art = FieldArt {
        // The tracer's mesh, and the tracer's trick: a unit cuboid stretched
        // along one axis is a streak, and there is one of them rather than one
        // mesh per wisp.
        wisp: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        // The whistle's annulus, at unit radius, scaled to the footprint when
        // it is drawn. Shared with the squad so the two rings a player is asked
        // to read are visibly the same kind of mark.
        ring: meshes.add(squad::ring_mesh()),
        ladder,
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
    add_field(commands, site, &art);
    // Handed back as well as inserted, because the level that comes up in this
    // same run wants it and an inserted resource does not exist until the next
    // sync point. Nothing but handles, so the clone is free.
    commands.insert_resource(art.clone());
    art
}

/// Hangs one machine's worth of plasma off `parent`.
///
/// The wisps are children of the machine rather than of its model, which is
/// what lets the model be turned upright and scaled without the field having to
/// be un-turned again: everything below the machine is in the generator's own
/// units, and the machine's own uniform scale is the only size in it.
fn add_field(commands: &mut Commands, parent: Entity, art: &FieldArt) {
    for index in 0..WISPS {
        // One golden-angle walk feeds all five numbers. Successive wisps are
        // never at the same place, never at the same speed and never flaring
        // together, and the whole field is a function of its index.
        let phase = index as f32 * GOLDEN_ANGLE;
        let spread = |scale: f32| (phase * scale).sin().abs();
        commands.entity(parent).with_child((
            Wisp {
                u: phase,
                rate: 0.65 + 0.70 * spread(0.37),
                v: phase * 2.0,
                wind: 0.80 + 1.10 * spread(0.21),
                pulse: phase * 1.7,
            },
            bevy::light::NotShadowCaster,
            Mesh3d(art.wisp.clone()),
            MeshMaterial3d(art.ladder[0].clone()),
            Transform::default(),
            // Shown by `animate` on the first frame it is worth seeing.
            Visibility::Hidden,
        ));
    }
}

/// Puts one machine in the world: the model, and the field inside it.
///
/// Shared by the build button and by anything else that ever wants one, the
/// same way [`crate::squad::spawn_ally`] is shared between the console and the
/// warp pipe.
pub fn spawn(
    commands: &mut Commands,
    assets: &AssetServer,
    art: &FieldArt,
    at: Vec3,
    yaw: f32,
    scale: f32,
) -> Entity {
    let built = commands
        .spawn((
            Stellarator {
                radius: footprint(scale),
            },
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
    add_field(commands, built, art);
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
            spawn(
                &mut commands,
                &assets,
                &art,
                build.aim,
                yaw,
                build_scale(held),
            );
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

/// Runs every wisp in every field: round the ring, round the tube, and up and
/// down its own flare.
///
/// One system for the built machines and the preview alike -- the preview is a
/// field with no machine around it, so what you are shown while you are aiming
/// is exactly what you are about to build.
///
/// Reads the wall clock rather than counting frames, so the field turns at the
/// same rate whatever the frame rate is. Both sliders are read live: what a
/// plasma looks like at a given speed and brightness is not something either
/// number predicts, which is the same argument `tracer_width` is on a slider
/// for.
pub fn animate(
    time: Res<Time>,
    tuning: Res<GameTuning>,
    art: Res<FieldArt>,
    mut wisps: Query<(
        &Wisp,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let now = time.elapsed_secs();
    for (wisp, mut transform, mut visible, mut material) in &mut wisps {
        let u = wisp.u + now * tuning.stellarator_spin * wisp.rate;
        let v = wisp.v + now * tuning.stellarator_spin * wisp.wind;
        // Nought to one, most of it spent near nought. See [`FLARE_SHARPNESS`].
        let flare = (0.5 + 0.5 * (now * FLARE_HZ * wisp.rate + wisp.pulse).sin())
            .powf(FLARE_SHARPNESS)
            * tuning.stellarator_glow;
        let step = ((flare * LADDER as f32) as usize).min(LADDER - 1);
        if flare <= 0.0 {
            *visible = Visibility::Hidden;
            continue;
        }
        *visible = Visibility::Inherited;
        if material.0.id() != art.ladder[step].id() {
            material.0 = art.ladder[step].clone();
        }
        // The streak lies along the ring's own tangent, taken as a chord across
        // two nearby samples rather than differentiated: the parametrisation is
        // a sum of cosines with a seven-fold twist in it, and a chord is both
        // shorter to write and exactly what is being drawn.
        //
        // Everything below is in the *file's* units rather than in the shape's,
        // which is what keeps the plasma inside the coils when the model is
        // re-exported at another `--scale`.
        let size = machine().scale;
        let ahead = flux_point(u + TANGENT, v) * size;
        let behind = flux_point(u - TANGENT, v) * size;
        let along = ahead - behind;
        let Some(direction) = along.try_normalize() else {
            continue;
        };
        let length = (WISP_SHORT + (WISP_LONG - WISP_SHORT) * flare.min(1.0)) * size;
        // Lifted with the machine, so a wisp is inside the coils rather than
        // buried in the lawn they are standing on.
        let centre = (ahead + behind) * 0.5 + Vec3::Y * machine().lift;
        *transform = Transform::from_translation(centre)
            // `looking_to` points local -Z along what it is given, so the -Z
            // here puts local +Z -- the axis the cuboid is stretched on -- along
            // the direction of travel. The same turn `weapon::spawn_tracer`
            // makes, for the same mesh.
            .looking_to(-direction, Vec3::Y)
            .with_scale(Vec3::new(WISP_WIDTH * size, WISP_WIDTH * size, length));
    }
}

/// Aiming and drawing, in the order one frame does them.
pub fn systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (place, animate, float_coils).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hold grows the machine the same way the whistle grows its circle,
    /// and stops in the same two places.
    #[test]
    fn the_machine_grows_from_a_tap_to_its_cap() {
        assert_eq!(build_scale(0.0), SCALE_MIN);
        assert_eq!(build_scale(squad::TAP_SECONDS), SCALE_MIN);
        let half = build_scale(squad::TAP_SECONDS + GROW_SECONDS * 0.5);
        assert!(half > SCALE_MIN && half < SCALE_MAX, "{half}");
        let grown = build_scale(squad::TAP_SECONDS + GROW_SECONDS);
        assert!((grown - SCALE_MAX).abs() < 1e-3, "{grown}");
        // Leaning on the button does not grow it past the cap.
        assert!(build_scale(60.0) <= SCALE_MAX + 1e-3);
    }

    /// Two machines may stand beside each other and may not stand through each
    /// other, and how high up they are has nothing to do with it.
    #[test]
    fn a_site_is_clear_only_when_the_footprints_miss() {
        let there = Vec3::new(0.0, 0.0, 0.0);
        let radius = machine().radius;
        let placed = [(there, radius)];
        // Touching rims is allowed; anything nearer is not.
        assert!(fits(
            Vec3::new(radius * 2.0, 0.0, 0.0),
            radius,
            &placed
        ));
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
        let plasma = world.spawn((Name::new(PLASMA_NODE), Transform::default())).id();
        // Not at the origin, so a `rest` that was assumed rather than read
        // would move it.
        let coil = world
            .spawn((Name::new("Modular Coil 07"), Transform::from_xyz(0.1, 0.2, 0.3)))
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
            world.run_system_once(float_coils).expect("it would not run");
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
