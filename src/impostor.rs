//! Distant enemies, drawn as sprites instead of as skeletons.
//!
//! This is the module the crowd budget lives or dies on, so it is worth saying
//! plainly what it is for.
//!
//! Bevy marks every skinned mesh `NoAutomaticBatching`, because each one needs
//! its own joint matrices, so **a skinned model is one draw call per mesh
//! primitive and no two of them ever merge**. The actors here are split by
//! material: two primitives for a slime, one for an ant, so a mixed field of
//! two thousand is three thousand draw calls a frame. That is already most of a
//! frame, and no amount of lowering the internal resolution touches it because
//! the cost is in submitting the draws rather than in filling the pixels. It
//! was far worse when this was written: the decomp's scuttlebug was fifteen
//! primitives for seventy-six triangles, and the same field cost seventeen
//! thousand draws.
//!
//! An impostor is the standard answer: past the distance where a model is a few
//! pixels tall, replace it with a flat quad showing a picture of that model,
//! taken from the angle you happen to be looking from. The pictures are baked
//! ahead of time into one atlas per enemy kind -- every frame of its walk cycle
//! across the columns, every bearing round it down the rows, and the whole
//! block of bearings again for each height the camera might be looking down
//! from. Two of those heights, at present: see [`Tier`] for why one is not
//! enough and three would be paying for very little.
//!
//! The saving comes from the quads being *one mesh*. They are not one entity
//! each: the whole crowd of a kind is rebuilt every frame into a single vertex
//! buffer, which is one draw call for a thousand slimes rather than two
//! thousand. That is also why this does not use a per-instance storage buffer
//! and a vertex-pulling shader, which would be marginally faster on a good GPU:
//! a plain vertex buffer works on anything, and the machine this is aimed at is
//! not a good GPU.
//!
//! Two draw calls for the whole distant field, then, against nineteen thousand.
//!
//! The near crowd is untouched and still drawn as skeletons -- an impostor seen
//! close up is obviously a cardboard cutout. Where the swap happens is
//! `enemy_draw` in the tuning console, and there is deliberately only the one
//! distance: [`crate::enemy::update`] hides a skinned enemy past it and [`draw`]
//! picks up exactly what it hid, so there is no band where an enemy is drawn
//! twice and none where it is drawn not at all.

pub mod bake;

use crate::{
    enemy::{Enemy, Kind, Quirk},
    n64::{N64Material, N64Uniform},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

/// Where the baked sheets live, relative to the asset root.
const SHEETS: &str = "impostors";

/// How much of a fragment has to survive to be drawn at all.
///
/// Masked rather than blended, and that is a performance decision as much as a
/// visual one: a thousand blended quads have to be depth-sorted against each
/// other and drawn back to front, which is a sort of the whole crowd every
/// frame and a guarantee that none of them batch. Masked quads are opaque
/// geometry, drawn in any order, with the depth buffer settling overlaps.
const ALPHA_CUTOFF: f32 = 0.5;

/// What the baker wrote beside an atlas: how to read it.
///
/// Deserialised rather than compiled in, so that re-baking a sheet with more
/// angles, another elevation or a longer clip is a change to two files in
/// `assets/` rather than a change to this module.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct SheetMeta {
    /// Which actor this was baked from, for the error message when it is not
    /// the one that was asked for.
    pub model: String,
    /// Pixels along one side of a cell.
    pub cell_px: u32,
    /// Distinct bearings round the model, evenly spaced. One per row **within a
    /// tier**, so the atlas is `angles * tiers.len()` rows tall in all.
    pub angles: usize,
    /// Frames of the walk cycle. One per column.
    pub frames: usize,
    /// How wide and tall a cell is in world units, which is the size of the
    /// quad it gets drawn onto.
    ///
    /// One number for the whole sheet rather than one per tier, and that is
    /// deliberate: the baker fits a single camera to the largest thing any tier
    /// sees, and pays for it in margin around the tiers that see less. A
    /// per-tier size would make every sprite in the field jump in size at the
    /// moment the tier changed, which is the one artefact tiers can add that a
    /// flat sheet cannot have.
    pub world_size: f32,
    /// How fast to play the columns, in frames a second.
    pub fps: f32,
    /// The elevations this sheet was baked from, flattest first, one block of
    /// `angles` rows each.
    pub tiers: Vec<Tier>,
}

/// One elevation the model was photographed from: how far above it the bake
/// camera stood, and where the model's origin came out in the picture.
///
/// Why there is more than one. A sprite is a photograph taken from a particular
/// height, and the crowd is not always looked at from the height it was
/// photographed at: stand on a wall above a courtyard of slimes and a sheet
/// baked level shows a field of enemies pictured from their own eye level,
/// under a camera forty degrees above them. Every one of them presents its
/// flank where it should be showing the top of its head, and the quads carrying
/// those pictures are turning edge on into the bargain. A second block of rows
/// baked from up there answers both, and costs the runtime nothing per sprite:
/// it is rows in an atlas and a division to find them.
///
/// The other half of the crowd it is for never leaves the ground at all -- or
/// rather, never leaves *its* ground. An ant on a wall is standing on the wall,
/// and a player facing that wall is looking straight down on the ant whatever
/// the camera's pitch says. Which tier a sprite is drawn from is settled in the
/// model's own frame, so the wall, the ceiling and every slope between come out
/// of the same arithmetic the flat ground does. See [`SheetMeta::tier`].
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Tier {
    /// How far above the model the camera stood, in degrees. Zero is level with
    /// it, ninety is straight overhead.
    pub elevation: f32,
    /// Where the model's own origin -- the point its `Transform` puts on the
    /// ground -- sits in the cell, as a fraction of the cell's height measured
    /// up from the bottom edge.
    ///
    /// Without it a sprite is centred on the enemy's feet and the enemy appears
    /// to be buried to the waist. With it the quad is lifted so that the ground
    /// in the picture lands on the ground in the world.
    ///
    /// Per tier, because it is a measurement of a *picture*: the same origin
    /// lands somewhere else in the cell when the camera photographing it is
    /// fifty-five degrees up rather than fifteen.
    pub foot: f32,
}

impl Tier {
    /// The up axis of a quad showing this tier's picture, given the surface the
    /// model is standing on (`standing`, its own up) and the direction the
    /// camera lies in *along* that surface (`flat`). Both unit, perpendicular.
    ///
    /// **The quads are not upright.** Each lies square on to the direction its
    /// picture was taken from -- for the flattest tier a fifteen degree lean
    /// nobody will see, for the steepest a card laid well back towards the
    /// ground. That is what makes a photograph a substitute for a model: it is
    /// only undistorted on a surface facing the way the camera did.
    ///
    /// It is also what saves the case an upright quad gets steadily worse at.
    /// An upright card is foreshortened by the cosine of the camera's own tilt
    /// and vanishes altogether from straight above; this one has turned to meet
    /// the camera by then, and what it is showing is the picture of the model's
    /// back, which is what is actually up there to see.
    pub fn up(&self, standing: Vec3, flat: Vec3) -> Vec3 {
        let tilt = self.elevation.to_radians();
        // The bake camera's own up vector, worked back out. It looked along
        // `-(flat * cos + standing * sin)` with no roll, which leaves this. Any
        // other axis and the picture is drawn leaning.
        standing * tilt.cos() - flat * tilt.sin()
    }
}

/// What a sheet with no tiers at all is read as, so that a malformed sidecar is
/// a wrong-looking sprite rather than a division by zero. [`read_meta`] rejects
/// one before it can get this far.
const FLAT: Tier = Tier {
    elevation: 0.0,
    foot: 0.5,
};

impl SheetMeta {
    /// Columns and rows the atlas is expected to have. The layout is fixed:
    /// **one column per frame, one row per bearing, and one block of `angles`
    /// rows per tier**, flattest tier first.
    pub fn grid(&self) -> (usize, usize) {
        (self.frames, self.angles * self.tiers.len())
    }

    /// How big the atlas has to be for this description to be true of it.
    pub fn atlas_size(&self) -> UVec2 {
        let (cols, rows) = self.grid();
        UVec2::new(cols as u32 * self.cell_px, rows as u32 * self.cell_px)
    }

    /// Which tier to draw a model by, given where the camera is relative to it
    /// **in the model's own frame**.
    ///
    /// What is read off the vector is the angle it makes with the model's own
    /// ground -- so a camera that is high but a long way off picks the flat
    /// tier, exactly as it should: what matters is the angle it looks down at
    /// *this* enemy, not how far up it is.
    ///
    /// And the model's own ground is not always the world's. An ant on a wall
    /// is standing on the wall; a player walking up to that wall and looking
    /// straight at the bug is looking straight down on it as far as the bug is
    /// concerned, and wants the picture taken from above it. Reading the angle
    /// in the model's frame gets that for nothing, and gets every slope between
    /// flat and vertical right on the way: a bug on a forty-five degree roof
    /// crosses to the steep tier when the camera is level with it, which is
    /// exactly when its back comes into view.
    ///
    /// Nearest baked elevation rather than the nearest below, so the error is
    /// half a gap either way instead of a whole gap in one direction. With
    /// tiers at fifteen and fifty-five degrees the hand-over is at thirty-five,
    /// and no sprite is ever showing a picture taken more than twenty degrees
    /// from where it is being looked at.
    pub fn tier(&self, local: Vec3) -> usize {
        let ground = Vec2::new(local.x, local.z).length();
        let elevation = local.y.atan2(ground).to_degrees();
        let mut best = 0;
        let mut closest = f32::MAX;
        for (index, tier) in self.tiers.iter().enumerate() {
            let error = (tier.elevation - elevation).abs();
            if error < closest {
                closest = error;
                best = index;
            }
        }
        best
    }

    /// The tier a row of the atlas was baked for. The inverse of the layout
    /// [`Self::grid`] promises, and the cheap half of it: a division rather
    /// than the search [`Self::tier`] does.
    pub fn tier_at(&self, row: usize) -> Tier {
        self.tiers
            .get(row / self.angles.max(1))
            .copied()
            .unwrap_or(FLAT)
    }

    /// Which cell shows a model turned to `facing` seen from a camera lying
    /// `to_camera` away from it, `phase` seconds into its walk.
    ///
    /// `to_camera` is the **whole** vector from the model to the camera rather
    /// than a flattened one, and `facing` is the model's **whole** rotation
    /// rather than a heading. Both together are one thing: the camera's
    /// position in the model's own frame, whose bearing picks the row inside a
    /// tier and whose height picks the tier.
    ///
    /// Measuring in the model's frame is what makes turning the enemy and
    /// orbiting the camera pick the same picture -- which is what makes a slime
    /// crawling away from you show you its back -- and, for a crawler, what
    /// makes the wall it is stuck to the floor it is standing on.
    ///
    /// Rounded to the nearest angle rather than truncated, so the error is half
    /// a step either way instead of a whole step in one direction, and wrapped
    /// rather than clamped, because a bearing is a circle.
    pub fn cell(&self, facing: Quat, to_camera: Vec3, phase: f32) -> (usize, usize) {
        let local = facing.inverse() * to_camera;
        let bearing = local.x.atan2(local.z);
        let step = std::f32::consts::TAU / self.angles as f32;
        let row = (bearing / step).round() as i64;
        let row = row.rem_euclid(self.angles as i64) as usize;
        let row = self.tier(local) * self.angles + row;
        let column = (phase * self.fps).floor() as i64;
        let column = column.rem_euclid(self.frames as i64) as usize;
        (column, row)
    }

    /// The corner of that cell in texture coordinates, and how big it is.
    ///
    /// Inset by half a texel on every side. Without it the sampler, given a
    /// coordinate exactly on the boundary between two cells, is free to pick up
    /// the neighbouring one -- which reads as a hairline of the wrong animation
    /// frame down the edge of every sprite in the field.
    pub fn uv(&self, column: usize, row: usize) -> (Vec2, Vec2) {
        let (cols, rows) = self.grid();
        let size = Vec2::new(1.0 / cols as f32, 1.0 / rows as f32);
        let inset = Vec2::splat(0.5) / self.atlas_size().as_vec2();
        let low = Vec2::new(column as f32, row as f32) * size + inset;
        (low, size - inset * 2.0)
    }
}

/// One kind's sheet, once it has been loaded.
///
/// The material is not kept: it is handed to the field entity at startup and
/// never written to again, which is the whole reason a thousand sprites of one
/// kind collapse into a single draw.
pub struct Sheet {
    pub meta: SheetMeta,
    pub mesh: Handle<Mesh>,
}

/// Every sheet the game has, and the quads built out of them.
#[derive(Resource, Default)]
pub struct Impostors {
    slime: Option<Sheet>,
    ant: Option<Sheet>,
    /// Every distant enemy's shadow, of every kind, in one mesh.
    ///
    /// One mesh rather than one per kind because a shadow is a shadow: they all
    /// share the disc texture and the solid rung of the fade ladder, so nothing
    /// distinguishes a slime's from an ant's except its radius. That
    /// makes the whole far crowd's shadows a single extra draw call.
    shadows: Option<Handle<Mesh>>,
}

impl Impostors {
    fn get(&self, kind: Kind) -> Option<&Sheet> {
        match kind {
            Kind::Slime => self.slime.as_ref(),
            Kind::Ant => self.ant.as_ref(),
        }
    }
}

/// How many enemies each tier of the crowd is drawing, for the corner readout.
///
/// The two numbers have to add up to the field. When they do not, something is
/// being drawn by neither -- which is the failure mode this whole module has,
/// and one that is invisible from a screenshot because "fewer enemies than I
/// expected" and "the enemies wandered elsewhere" look identical.
#[derive(Resource, Default, Clone, Copy)]
pub struct ImpostorStats {
    /// Enemies drawn as sprites.
    pub sprites: usize,
    /// Enemies drawn as skinned models, near enough to matter.
    pub skinned: usize,
    /// Enemies drawn by **neither** path this frame.
    ///
    /// Should always be zero. Anything else is an enemy that is in the world,
    /// is not far enough away to be a sprite, and has no model yet -- which is
    /// to say an enemy nobody can see. See [`drawn_as_model`].
    pub missing: usize,
}

/// The one entity per kind that a whole distant crowd is drawn by.
///
/// A marker rather than a handle to anything: what it is for is being able to
/// count these in a test and see them by name in an inspector, so that "the far
/// crowd is two draw calls" is a claim with something behind it.
#[derive(Component)]
pub struct ImpostorField;

/// The stem of the two files a kind's sheet lives in.
fn stem(kind: Kind) -> &'static str {
    match kind {
        Kind::Slime => "slime",
        Kind::Ant => "ant",
    }
}

/// Reads a sheet's sidecar off disk.
///
/// Synchronous, at startup, rather than through the asset server: it is two
/// small files, and everything downstream needs the numbers in them before it
/// can size a single quad. Loading them asynchronously would mean a frame or
/// two at the start of the game where the far crowd is drawn at the wrong size.
pub fn read_meta(root: &std::path::Path, kind: Kind) -> Result<SheetMeta, String> {
    let path = root.join(SHEETS).join(format!("{}.json", stem(kind)));
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let meta: SheetMeta =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if meta.model != stem(kind) {
        return Err(format!(
            "{} describes {:?} rather than {:?}",
            path.display(),
            meta.model,
            stem(kind)
        ));
    }
    // Everything downstream divides the rows up by tier, so a sheet claiming
    // none of them describes an atlas with no rows in it. Refused here, where
    // the failure is one line on the console and a kind that draws no far
    // crowd, rather than left to be discovered a division at a time.
    if meta.tiers.is_empty() {
        return Err(format!("{} lists no elevations", path.display()));
    }
    Ok(meta)
}

/// Builds the material and the empty mesh each kind's crowd is drawn with.
///
/// Called from startup. A kind whose sheet is missing or malformed is simply
/// left out: the game still runs, that kind's far crowd is drawn as nothing at
/// all beyond `enemy_draw`, and the reason is on stderr. A missing optimisation
/// should not be a game that will not start.
pub fn prepare(
    commands: &mut Commands,
    assets: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<N64Material>,
    shadow: Handle<StandardMaterial>,
    console: &mut crate::console::ConsoleState,
    root: &std::path::Path,
) {
    let mut impostors = Impostors::default();
    // The shadows go down first, so that when a sprite and a disc land on the
    // same pixel the sprite is the one that was drawn second.
    let shadows = meshes.add(empty_field());
    commands.spawn((
        ImpostorField,
        Name::new("impostor field: shadows"),
        Mesh3d(shadows.clone()),
        MeshMaterial3d(shadow),
        Transform::default(),
        NoFrustumCulling,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
    impostors.shadows = Some(shadows);
    for kind in [Kind::Slime, Kind::Ant] {
        let meta = match read_meta(root, kind) {
            Ok(meta) => meta,
            Err(error) => {
                // Both, and the console one is the one that matters. A packaged
                // Windows build has nobody reading stderr, and this failure is
                // otherwise silent for ever: the game runs, and every enemy past
                // `enemy_draw` is drawn as nothing at all.
                let note = format!(
                    "impostors: no sheet for {kind:?}, so nothing past enemy_draw \
                     will be drawn -- {error}"
                );
                eprintln!("{note}");
                console.report(note);
                continue;
            }
        };
        let atlas = assets.load(format!("{SHEETS}/{}.png", stem(kind)));
        let material = materials.add(N64Material {
            // Unlit: the sheet was baked with the world's own lighting already
            // resolved into it, exactly as the castle's vertex colours are.
            // Lighting it a second time would darken every distant enemy.
            uniform: N64Uniform::unlit(ALPHA_CUTOFF),
            base_color_texture: Some(atlas),
            alpha_mode: AlphaMode::Mask(ALPHA_CUTOFF),
            // A quad turned to face the camera is always seen from its front,
            // so there is no back to draw -- but the crawlers hang off walls and
            // ceilings, and one of those seen from underneath is the case that
            // would otherwise vanish.
            double_sided: true,
        });
        let mesh = meshes.add(empty_field());
        let sheet = Sheet {
            meta,
            mesh: mesh.clone(),
        };
        commands.spawn((
            ImpostorField,
            Name::new(format!("impostor field: {}", stem(kind))),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            // The mesh is rebuilt in world space every frame and its vertices
            // are spread over the whole castle, so its transform is the
            // identity and its bounds mean nothing. Culling it against a stale
            // bounding box is how the entire far crowd disappears when the
            // camera turns.
            Transform::default(),
            NoFrustumCulling,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ));
        match kind {
            Kind::Slime => impostors.slime = Some(sheet),
            Kind::Ant => impostors.ant = Some(sheet),
        }
    }
    commands.insert_resource(impostors);
}

/// A mesh with the right attributes and nothing in it.
///
/// Positions and texture coordinates only. The shader needs no normals -- an
/// impostor is unlit, so `n64.wgsl` never reaches the block that would use them
/// -- and no vertex colours, and leaving both out is a third off the bandwidth
/// of rebuilding the whole crowd every frame.
fn empty_field() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        // Rebuilt from the CPU every frame, so the main-world copy has to stay.
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new())
    .with_inserted_indices(Indices::U32(Vec::new()))
}

/// One member of a crowd, reduced to what drawing it as a sprite needs.
#[derive(Clone, Copy, Debug)]
pub struct Member {
    /// Where its feet are.
    pub at: Vec3,
    /// Which way it is turned -- and, for a crawler, what it is standing on.
    ///
    /// The whole rotation rather than a heading about the vertical, because the
    /// sheet was baked in the model's own frame and a bug on a wall does not
    /// share the world's idea of which way is up. It was a heading once, taken
    /// out of the rotation with `angle * axis.y.signum()`, which is the yaw
    /// exactly when the rotation is a yaw and something else entirely when it
    /// is not.
    pub facing: Quat,
    /// How far into its walk cycle it is, in seconds.
    pub phase: f32,
}

/// Writes a crowd into a mesh as camera-facing quads.
///
/// Kept out of the system so that the whole of the geometry can be checked
/// without a renderer: this is the part that can put a sprite at the wrong
/// height, the wrong size, or facing the wrong way.
///
/// The quads turn to face the camera about the model's own up -- like
/// everything else billboarded here, see [`crate::billboard`] -- and then lie
/// back by the elevation their picture was baked at, which is the one place a
/// photograph of a model is undistorted. See [`Tier::up`]; that lean is what
/// lets the crowd still read from a camera high above it.
///
/// **The model's own up, not the world's.** For everything walking on a floor
/// they are the same vector and this is the billboarding it always was. For a
/// crawler stuck to a wall or hanging from a ceiling the two have nothing to do
/// with each other, and it is the model's that is right: the wall is the floor
/// that bug is standing on, the picture it should be showing is the one taken
/// from out in the room, and the surface that picture belongs on is one facing
/// out of the wall rather than one standing up out of the ground.
pub fn build_field(meta: &SheetMeta, eye: Vec3, crowd: &[Member], mesh: &mut Mesh) {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(crowd.len() * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(crowd.len() * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(crowd.len() * 6);
    let half = meta.world_size * 0.5;
    for member in crowd {
        let to_camera = eye - member.at;
        // Which way is up for this one: out of the floor it is walking on, the
        // wall it is stuck to or the ceiling it is hanging from.
        let standing = member.facing * Vec3::Y;
        // And which way the camera lies along that surface, which is the axis
        // the bearing is measured round and the one the quad leans along.
        let flat = to_camera - standing * to_camera.dot(standing);
        // Near enough along the model's own up that there is no bearing to be
        // had: overhead of a walker, or square in front of a bug on a wall,
        // which is not a rare way to look at a wall at all.
        //
        // Judged against the distance rather than against zero, and that is the
        // whole of it. What is left after taking the `standing` component out
        // of a vector twenty metres long is perpendicular to `standing` to
        // within a millionth of that -- fine when the remainder is metres, and
        // *most of the remainder* when it is microns. Normalising one of those
        // gives an axis that is not perpendicular to `standing` at all, and the
        // quad built on it comes out a quarter of its proper size: a wall of
        // bugs that shrivels as you turn to face it square on.
        let square_on = flat.length_squared() <= to_camera.length_squared() * 1e-6;
        let flat = if square_on {
            // Only the roll of the picture is left to settle, its subject being
            // face on to the camera whatever is chosen. The model's own forward
            // is the choice that agrees with the row `cell` picks in the same
            // situation: bearing zero, the camera in front.
            member.facing * Vec3::Z
        } else {
            flat.normalize()
        };
        // Across the view, in the model's own ground plane. It is the same axis
        // whatever the tier: the quad leans back about it, so it never leaves
        // that plane.
        let right = standing.cross(flat);
        let (column, row) = meta.cell(member.facing, to_camera, member.phase);
        let (uv, size) = meta.uv(column, row);
        // The row already chose the tier; reading it back off the row is
        // cheaper than choosing it twice.
        let tier = meta.tier_at(row);
        let up = tier.up(standing, flat);
        // The cell's bottom edge sits `foot` of a cell-height below the origin
        // *along the quad's own up*, so that the ground in the picture meets
        // the ground in the world however far the quad is leaning.
        let bottom = member.at - up * (meta.world_size * tier.foot);
        let base = positions.len() as u32;
        let corners = [
            (-half, 0.0),
            (half, 0.0),
            (half, meta.world_size),
            (-half, meta.world_size),
        ]
        .map(|(across, height)| bottom + right * across + up * height);
        // How much of the quad is floated in front of where the enemy is
        // actually standing, and why any of it has to be. See `float_toward`.
        let lean = corners
            .iter()
            .fold(0.0_f32, |most, corner| most.max((*corner - eye).length()));
        let near = float_toward(eye, member.at, lean, meta.world_size * tier.foot);
        for (corner, (u, v)) in corners
            .into_iter()
            .zip([(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)])
        {
            positions.push((eye + (corner - eye) * near).to_array());
            uvs.push((uv + Vec2::new(u, v) * size).to_array());
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
}

/// How much of a card is drawn nearer than the enemy standing under it, as a
/// multiple of how far that card hangs below the enemy's feet.
///
/// One: the card is pulled forward by exactly its own dip, which is the amount
/// of it that was underground. A number rather than a tuned constant, and the
/// sweep behind it is worth writing down -- at nothing the sprite is still cut,
/// at four tenths its near legs still are, at eight tenths it is whole at both
/// tiers, and past there nothing more is bought. What the extra would cost is
/// the far crowd winning the depth test against more of what stands in front of
/// it, so the smallest amount that works is the right one.
const FLOAT: f32 = 1.0;

/// How far in toward the eye a quad has to be pulled for the ground the enemy
/// is standing on to stop cutting it in half, as a scale about the eye.
///
/// **A sprite is a plane through a point on the floor, so part of it is always
/// under the floor.** That is not the lean and it is not the tier: the picture
/// contains the model's near side -- the legs on the camera's side of an ant,
/// the front of a slime -- and those parts of the model are standing on the
/// ground a metre in front of its origin, while the card puts them a metre
/// *below* it. The depth buffer then does exactly what it is for and hands
/// those pixels to the lawn. On screen the enemy is sliced along the line where
/// the card leaves the ground, and grass shows through the cut. It is worst
/// looking down -- the card is laid back toward the floor at the steep tier, so
/// nearly half of it is buried -- and it is what eats the legs off every ant in
/// the field at every angle. See `a_sprite_is_drawn_clear_of_the_floor`.
///
/// Nothing about where the card *is* can fix that, because every plane through
/// a point on a floor goes under the floor. What can is drawing it nearer than
/// it stands, which is what this does.
///
/// **Scaled about the eye rather than translated, and that is the whole trick.**
/// A perspective camera projects a point by its direction from the eye alone, so
/// moving every corner along its own ray to the eye by the same *factor* leaves
/// the picture on screen pixel for pixel identical -- same place, same size, same
/// frame of the walk -- and changes nothing but the depth it is tested at.
/// Translating the card toward the camera instead would swell it by the ratio of
/// the two distances, which is a crowd that grows as it approaches the swap
/// distance.
///
/// What it costs is the sprite winning the depth test against anything within
/// [`FLOAT`] dips of it, which for a slime is about a metre and a half and for an
/// ant about three. That is the trade, and it is the right way round: an enemy
/// standing a little in front of a low wall reads as standing in front of it,
/// and an enemy sawn in half by the lawn reads as a bug.
fn float_toward(eye: Vec3, at: Vec3, furthest: f32, dip: f32) -> f32 {
        let wanted = (at - eye).length() - dip * FLOAT;
    // Both ends guarded. A quad with its furthest corner already at the eye has
    // no ray to slide along, and one whose dip is deeper than the enemy is far
    // away -- a sprite the camera is standing inside -- must not be turned
    // inside out through it.
    if furthest <= f32::EPSILON || wanted <= 0.0 {
        return 1.0;
    }
    (wanted / furthest).min(1.0)
}

/// How far a far shadow floats off the ground it is drawn on.
///
/// The same job [`crate::shadow::LIFT`] does for the near discs, but a little
/// more generous: these are laid flat rather than along the ground's own
/// normal, so on a slope one edge of the disc sits closer to the surface than
/// the middle does.
const SHADOW_LIFT: f32 = 0.05;

/// Writes the far crowd's shadows into a mesh as flat discs on the ground.
///
/// Cheap in the way that matters: **it asks the level nothing**. A walking
/// enemy's own `y` is already the height of the ground under it -- that is what
/// [`crate::enemy::settle`] and the flow field both spend their time
/// maintaining -- so the disc goes at its feet and no floor query is needed.
/// The near discs in [`crate::shadow::project`] each cost two grid lookups a
/// frame, which is affordable for a couple of hundred and not for two thousand.
///
/// Flat rather than lying along the slope, and square rather than round: the
/// texture's fade is what makes it read as a soft round shadow, and at this
/// distance the difference between a disc tilted onto a hillside and one laid
/// flat on it is well under a pixel.
pub fn build_shadows(crowd: &[(Vec3, f32)], mesh: &mut Mesh) {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(crowd.len() * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(crowd.len() * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(crowd.len() * 6);
    for (at, radius) in crowd {
        let base = positions.len() as u32;
        let centre = *at + Vec3::Y * SHADOW_LIFT;
        for ((dx, dz), uv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
        {
            positions.push((centre + Vec3::new(dx * radius, 0.0, dz * radius)).to_array());
            uvs.push([uv.0, uv.1]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    // The disc material is a `StandardMaterial`, which -- unlike `n64.wgsl` --
    // wants a normal whether it is unlit or not.
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![[0.0, 1.0, 0.0]; crowd.len() * 4],
    );
    mesh.insert_indices(Indices::U32(indices));
}

/// Gathers the far crowd and rebuilds the two meshes it is drawn with.
///
/// Runs once per rendered frame rather than per fixed step, because what it
/// depends on is where the camera is -- and a sprite that picked its angle on
/// the last simulation tick visibly lags the view as you turn.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn draw(
    time: Res<Time>,
    impostors: Option<Res<Impostors>>,
    mut stats: ResMut<ImpostorStats>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    crowd: Query<(&Enemy, &Transform, &Visibility, &Quirk, Option<&Children>)>,
    mut buffers: Local<Vec<(Kind, Vec<Member>)>>,
    mut shadows: Local<Vec<(Vec3, f32)>>,
) {
    let (Some(impostors), Ok(view)) = (impostors, camera.single()) else {
        return;
    };
    stats.skinned = 0;
    stats.sprites = 0;
    stats.missing = 0;
    let eye = view.translation();
    let elapsed = time.elapsed_secs();
    // The buffers are kept between frames: this refills them every frame for
    // every distant enemy in the world, and allocating a fresh pair of vectors
    // for a crowd of two thousand is a thing worth not doing sixty times a
    // second.
    if buffers.is_empty() {
        *buffers = vec![(Kind::Slime, Vec::new()), (Kind::Ant, Vec::new())];
    }
    for (_, members) in buffers.iter_mut() {
        members.clear();
    }
    shadows.clear();
    for (enemy, transform, visibility, quirk, children) in &crowd {
        // Exactly the ones the skinned pass is not drawing.
        // `crate::enemy::update` hides an enemy past `enemy_draw`, and this
        // picks up every one it hid -- so the two distances are one distance
        // and there is no band where an enemy is drawn twice or not at all.
        if *visibility != Visibility::Hidden {
            if children.is_some_and(|kids| !kids.is_empty()) {
                stats.skinned += 1;
            } else {
                stats.missing += 1;
            }
            continue;
        }
        let Some((_, members)) = buffers.iter_mut().find(|(kind, _)| *kind == enemy.kind) else {
            continue;
        };
        shadows.push((transform.translation, enemy.kind.shadow_radius()));
        members.push(Member {
            at: transform.translation,
            // Whole. A walker's rotation is a yaw and could be reduced to one;
            // a crawler's carries the surface it is on as well, and that is
            // half of what picks its picture.
            facing: transform.rotation,
            phase: elapsed + quirk.seed(),
        });
    }
    // Counted per kind against whether that kind actually has a sheet, rather
    // than as "everything we gathered". A kind whose atlas is missing gathers a
    // full crowd and draws none of it, and reporting those as sprites is what
    // let a packaged build with no sheets at all still claim to be drawing
    // thousands of them.
    for (kind, members) in buffers.iter() {
        if impostors.get(*kind).is_some() {
            stats.sprites += members.len();
        } else {
            stats.missing += members.len();
        }
    }
    if let Some(mesh) = impostors
        .shadows
        .as_ref()
        .and_then(|handle| meshes.get_mut(handle))
    {
        build_shadows(&shadows, mesh.into_inner());
    }
    for (kind, members) in buffers.iter() {
        let Some(sheet) = impostors.get(*kind) else {
            continue;
        };
        let Some(mesh) = meshes.get_mut(&sheet.mesh) else {
            continue;
        };
        build_field(&sheet.meta, eye, members, mesh.into_inner());
    }
}

/// Impostors are rebuilt after the animation and billboard work, in the same
/// schedule for the same reason: they are geometry aimed at the camera, and the
/// camera has finished moving by then.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    draw.into_configs()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG's dimensions, straight out of its IHDR chunk.
    ///
    /// Read by hand rather than through a decoder: the whole question is how
    /// wide and tall the file is, that lives in the first twenty-four bytes of
    /// every PNG ever written, and decoding twelve megabytes of atlas to find
    /// out would be the slowest test in the suite.
    fn png_size(path: &std::path::Path) -> UVec2 {
        let bytes = std::fs::read(path).expect("missing atlas");
        assert_eq!(&bytes[1..4], b"PNG", "{} is not a PNG", path.display());
        assert_eq!(&bytes[12..16], b"IHDR", "{} has no header", path.display());
        UVec2::new(
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        )
    }

    /// A PNG's size and its alpha channel, decoded with Bevy's own `image`.
    fn png_alpha(path: &std::path::Path) -> (UVec2, Vec<u8>) {
        let picture = image::open(path).expect("missing atlas").to_rgba8();
        let size = UVec2::new(picture.width(), picture.height());
        (size, picture.pixels().map(|pixel| pixel.0[3]).collect())
    }

    fn meta() -> SheetMeta {
        SheetMeta {
            model: "slime".into(),
            cell_px: 128,
            angles: 16,
            frames: 16,
            world_size: 1.2,
            fps: 12.0,
            tiers: vec![
                Tier {
                    elevation: 15.0,
                    foot: 0.15,
                },
                Tier {
                    elevation: 55.0,
                    foot: 0.3,
                },
            ],
        }
    }

    /// A camera `elevation` degrees above a sprite, on the `+z` side of it.
    fn eye_at(elevation: f32) -> Vec3 {
        let up = elevation.to_radians();
        Vec3::new(0.0, up.sin(), up.cos()) * 30.0
    }

    /// The quad [`build_field`] drew one member as, and the point that member's
    /// own origin was drawn at.
    ///
    /// `build_field` slides every card along its rays to the eye until the floor
    /// stops cutting it in half -- see [`float_toward`] -- which leaves the
    /// picture on screen untouched and every length in it scaled by the same
    /// factor. The factor is recoverable from the card's own side, and moving
    /// the model's origin by it puts the two back in one frame: that is the
    /// frame every claim about *where the picture sits on the model* is made in,
    /// and the only one it has an answer in.
    fn drawn(meta: &SheetMeta, eye: Vec3, member: Member) -> ([Vec3; 4], Vec3) {
        let mut mesh = empty_field();
        build_field(meta, eye, &[member], &mut mesh);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the field has no positions");
        };
        assert_eq!(p.len(), 4, "one member should be one quad");
        let corners = [0, 1, 2, 3].map(|index| Vec3::from(p[index]));
        // Off the card's own side rather than by recomputing what `build_field`
        // did: a test that works the scale out the same way the code did would
        // agree with it however wrong both were.
        let near = (corners[3] - corners[0]).length() / meta.world_size;
        (corners, eye + (member.at - eye) * near)
    }

    /// How a crawler stuck to a surface whose normal is `up` is turned, facing
    /// along `heading`. The same construction `enemy::orientation` uses, which
    /// is what puts the surface into the rotation's own Y column.
    fn stuck_to(up: Vec3, heading: Vec3) -> Quat {
        let up = up.normalize();
        let forward = (heading - up * heading.dot(up)).normalize();
        Quat::from_mat3(&Mat3::from_cols(up.cross(forward), up, forward))
    }

    /// Facing the camera picks the front, turning away picks the back, and the
    /// two are half the sheet apart. This is the property that makes a crowd
    /// read as a crowd of individuals rather than a wall of identical cards.
    #[test]
    fn the_cell_follows_the_angle_between_the_model_and_the_camera() {
        let meta = meta();
        let front = meta.cell(Quat::IDENTITY, Vec3::Z, 0.0).1;
        let back = meta.cell(Quat::from_rotation_y(std::f32::consts::PI), Vec3::Z, 0.0).1;
        assert_eq!(front, 0, "a model facing the camera is the first row");
        assert_eq!(
            back,
            meta.angles / 2,
            "a model facing away should be half the sheet round"
        );
        // Turning the model and orbiting the camera are the same thing.
        let turned = meta
            .cell(
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                Vec3::Z,
                0.0,
            )
            .1;
        let orbited = meta.cell(Quat::IDENTITY, Vec3::X, 0.0).1;
        assert_eq!(turned, meta.angles - orbited, "{turned} vs {orbited}");
    }

    /// A bearing is a circle, so every angle has to land on a real row --
    /// including the ones that round past the end of the sheet, which is where
    /// an index panic would live. And now that a row is a bearing *and* a tier,
    /// every height the camera can be at has to land on one too, straight down
    /// and straight up included.
    #[test]
    fn every_bearing_lands_on_a_row_that_exists() {
        let meta = meta();
        let rows = meta.grid().1;
        for step in -720..=720 {
            let yaw = step as f32 * 0.05;
            for height in [-90.0_f32, -35.0, 0.0, 15.0, 34.9, 35.1, 55.0, 89.0, 90.0] {
                let flat = Vec2::new(yaw.cos(), yaw.sin()) * height.to_radians().cos();
                let to_camera = Vec3::new(flat.x, height.to_radians().sin(), flat.y);
                let (column, row) =
                    meta.cell(Quat::from_rotation_y(yaw), to_camera, yaw.abs());
                assert!(row < rows, "bearing {yaw} at {height} chose row {row}");
                assert!(column < meta.frames, "phase {yaw} chose column {column}");
            }
        }
    }

    /// The whole point of the tiers: how far the camera is above an enemy picks
    /// which block of rows it is drawn from, and it is the *angle* that decides
    /// rather than the height -- a camera high in the air but a long way off is
    /// looking at a distant enemy almost level, and wants the flat pictures.
    #[test]
    fn the_camera_s_height_above_an_enemy_picks_the_tier() {
        let meta = meta();
        for (elevation, wanted) in [
            (0.0, 0),
            (15.0, 0),
            (34.0, 0),
            (36.0, 1),
            (55.0, 1),
            (89.0, 1),
        ] {
            let tier = meta.tier(eye_at(elevation));
            assert_eq!(
                tier, wanted,
                "a camera {elevation} degrees up chose tier {tier}"
            );
            let row = meta.cell(Quat::IDENTITY, eye_at(elevation), 0.0).1;
            assert_eq!(
                row / meta.angles,
                wanted,
                "...but its row {row} is in another tier"
            );
            assert_eq!(meta.tier_at(row), meta.tiers[wanted]);
        }
        // Far away and high up is not the same as overhead.
        let distant = Vec3::new(0.0, 40.0, 400.0);
        assert_eq!(meta.tier(distant), 0, "a distant camera wants flat pictures");
    }

    /// The same rule read in the model's own frame, which is what a crawler
    /// needs: a bug on a wall is standing on the wall, so a camera out in the
    /// room is *above* it however level the camera is, and a bug on the ceiling
    /// looked up at from below is being looked down on.
    ///
    /// This is the case the world-frame version of the test above gets exactly
    /// backwards. Every camera in it is level with the enemy, so every one of
    /// them would have picked the flat tier and shown a wall of bugs in
    /// profile, standing out of the wall sideways.
    #[test]
    fn a_crawler_s_own_surface_decides_what_is_above_it() {
        let meta = meta();
        // Out in the room, level with a bug on a wall whose normal is +z.
        let wall = stuck_to(Vec3::Z, Vec3::Y);
        let to_camera = Vec3::Z * 20.0;
        assert_eq!(
            meta.tier(wall.inverse() * to_camera),
            1,
            "a camera square in front of a bug on a wall is above it"
        );
        // Slid along the wall until it is edge on: back to the flat pictures.
        let along = Vec3::new(20.0, 0.0, 0.5);
        assert_eq!(
            meta.tier(wall.inverse() * along),
            0,
            "a camera nearly in the plane of the wall sees the bug's flank"
        );
        // Hanging from a ceiling, looked up at from the floor.
        let ceiling = stuck_to(Vec3::NEG_Y, Vec3::Z);
        assert_eq!(
            meta.tier(ceiling.inverse() * Vec3::new(0.0, -20.0, 3.0)),
            1,
            "a bug on the ceiling shows its back to the room below"
        );
        // A roof at forty-five degrees, from level with it: over the hand-over
        // at thirty-five, so the steep pictures. This is the \"sufficiently
        // steep surface\" case, and there is no threshold anywhere that says
        // so -- it falls out of measuring the angle in the model's frame.
        let roof = stuck_to(Vec3::new(0.0, 1.0, 1.0), Vec3::X);
        assert_eq!(meta.tier(roof.inverse() * (Vec3::Z * 20.0)), 1);
        // And a gentle one, from the same place, does not.
        let slope = stuck_to(Vec3::new(0.0, 6.0, 1.0), Vec3::X);
        assert_eq!(meta.tier(slope.inverse() * (Vec3::Z * 20.0)), 0);
    }

    /// A crawler's quad stands on the surface the crawler does. The sprite is a
    /// picture of a model standing on the ground, and the ground it is standing
    /// on here is the wall: the quad has to lie in the wall's plane rather than
    /// the world's vertical, or a bug on a wall is drawn as a card sticking out
    /// of it edge on.
    #[test]
    fn a_bug_on_a_wall_is_drawn_on_the_wall_rather_than_standing_out_of_it() {
        let meta = meta();
        let at = Vec3::new(2.0, 4.0, 0.0);
        // A wall facing +z, a bug on it heading upwards, a camera out in front.
        let facing = stuck_to(Vec3::Z, Vec3::Y);
        let eye = at + Vec3::new(1.0, 0.5, 20.0);
        let (corners, at) = drawn(
            &meta,
            eye,
            Member {
                at,
                facing,
                phase: 0.0,
            },
        );
        let (a, b, top) = (corners[0], corners[1], corners[3]);
        let up = (top - a).normalize();
        let across = (b - a).normalize();
        let normal = across.cross(up).normalize();
        // The steep tier, since the camera is out in the room.
        let tier = meta.tiers[1];
        let lean = tier.elevation.to_radians();
        // The wall is the bug's floor, so the quad is pitched up out of the
        // wall by the tier's elevation -- the same relationship to its own
        // surface that a sprite on the ground has to the ground.
        let wall = Vec3::Z;
        assert!(
            (up.dot(wall) - lean.cos()).abs() < 1e-3,
            "the quad's up is {up:?}, which is not {} out of the wall",
            tier.elevation
        );
        assert!(
            up.dot(Vec3::Y).abs() < 1.0,
            "the quad is standing up out of the world instead"
        );
        // So it faces mostly out of the wall, at the camera, rather than
        // standing up out of the floor with its edge to it.
        assert!(
            normal.z > 0.5,
            "the quad faces {normal:?}, which is not out of the wall"
        );
        // And the picture's ground line is behind the surface the bug is
        // standing on, exactly as a sprite on the floor has its own below the
        // floor.
        assert!(
            (a - at).dot(wall) < 0.0 && (b - at).dot(wall) < 0.0,
            "the quad's bottom edge is out in front of the wall"
        );
    }

    /// The card is drawn clear of the floor the enemy is standing on, and the
    /// picture on screen does not move when it is.
    ///
    /// Three claims, and they are the whole of [`float_toward`]:
    ///
    ///   * **no corner is further from the eye than the enemy is.** That is what
    ///     stops the floor cutting the sprite in half. A card is a plane through
    ///     a point on the ground, so some of it is always underground -- for the
    ///     steep tier nearly half of it -- and the depth buffer hands every one
    ///     of those pixels to the lawn. The enemy is sawn along the line where
    ///     its card leaves the floor and grass shows through the cut.
    ///   * **every corner is still on the ray it was on.** The card is scaled
    ///     about the eye rather than pushed toward it, and a perspective camera
    ///     projects a point by its direction from the eye alone -- so the sprite
    ///     lands on exactly the pixels it did before, at exactly the size. A
    ///     card that was *translated* toward the camera would swell by the ratio
    ///     of the two distances, which is a crowd that grows as it walks toward
    ///     the swap distance.
    ///   * **it is not pulled further than it needs to be.** What the float
    ///     costs is the sprite winning the depth test against whatever is in
    ///     front of it, so the amount is bounded by the card's own size rather
    ///     than by the distance to the camera.
    ///
    /// Checked at the steep tier and the flat one: the flat tier's card is
    /// nearly upright and dips under the floor by most of a cell all the same,
    /// which is the case that eats the legs off every ant in the field.
    #[test]
    fn a_sprite_is_drawn_clear_of_the_floor_without_moving_on_screen() {
        let meta = meta();
        let at = Vec3::new(3.0, 7.0, -2.0);
        for elevation in [5.0, 15.0, 40.0, 70.0] {
            let eye = at + eye_at(elevation);
            let member = Member {
                at,
                facing: Quat::IDENTITY,
                phase: 0.0,
            };
            let (corners, _) = drawn(&meta, eye, member);
            let reach = (at - eye).length();
            let tier = meta.tiers[meta.tier(eye_at(elevation))];
            let dip = meta.world_size * tier.foot;
            let deepest = corners
                .iter()
                .fold(0.0_f32, |most, corner| most.max((*corner - eye).length()));
            assert!(
                (deepest - (reach - dip)).abs() < 1e-4,
                "at {elevation} degrees the card's deepest corner is {deepest} from \
                 the eye rather than the {} that is one dip in front of the enemy",
                reach - dip
            );
            // Where the card would have been drawn with no float at all: what
            // the rays are compared against, and what the haul is measured from.
            let unfloated = {
                let to_camera = eye - at;
                let standing = Vec3::Y;
                let flat = (to_camera - standing * to_camera.dot(standing)).normalize();
                let right = standing.cross(flat);
                let up = tier.up(standing, flat);
                let bottom = at - up * (meta.world_size * tier.foot);
                let half = meta.world_size * 0.5;
                [
                    (-half, 0.0),
                    (half, 0.0),
                    (half, meta.world_size),
                    (-half, meta.world_size),
                ]
                .map(|(across, height)| bottom + right * across + up * height)
            };
            for (floated, plain) in corners.into_iter().zip(unfloated) {
                let one = (floated - eye).normalize();
                let other = (plain - eye).normalize();
                assert!(
                    one.distance(other) < 1e-5,
                    "at {elevation} degrees a corner moved off its own ray, from \
                     {other:?} to {one:?}"
                );
                // Measured as how far the corner actually travelled rather than
                // as where it ended up. A card leans toward the camera, so its
                // top corners start in front of the enemy and are *entitled* to
                // end up further in front still -- what has to be bounded is the
                // move, which is what costs the sprite its occlusion.
                // Bounded in the card's own units rather than by the distance
                // to the camera, which is the property that matters: what the
                // float costs is the sprite winning the depth test against
                // whatever is within the haul of it, and that must not grow as
                // the crowd recedes. A corner can be hauled the dip plus the
                // card's own depth -- the deepest corner is put one dip in
                // front, and the rest of the card follows it forward.
                let hauled = plain.distance(floated);
                let bound = dip * FLOAT + meta.world_size;
                assert!(
                    hauled <= bound,
                    "at {elevation} degrees a corner was hauled {hauled} forward, \
                     past the {bound} the card is deep"
                );
            }
            assert!(
                (corners[3] - corners[0]).length() > 0.0,
                "the card collapsed"
            );
        }
    }

    /// The one place the model's frame gives no bearing at all: a camera
    /// straight out along the model's own up. Overhead of a walker, and square
    /// in front of a bug on a wall, which is an ordinary way to look at a wall
    /// rather than a corner case -- so it has to draw a sprite rather than
    /// quietly drop one.
    #[test]
    fn a_sprite_seen_straight_down_its_own_up_is_still_drawn() {
        let meta = meta();
        let at = Vec3::new(1.0, 2.0, 3.0);
        for facing in [Quat::IDENTITY, stuck_to(Vec3::Z, Vec3::Y)] {
            let (corners, _) = drawn(
                &meta,
                at + facing * Vec3::Y * 20.0,
                Member {
                    at,
                    facing,
                    phase: 0.0,
                },
            );
            let (a, b, top) = (corners[0], corners[1], corners[3]);
            // Square and not collapsed. Its side is the cell's own size less
            // whatever `float_toward` took off, so the two sides are checked
            // against each other rather than against `world_size`.
            let (side, across) = ((top - a).length(), (b - a).length());
            assert!(
                side > meta.world_size * 0.5 && (side - across).abs() < 1e-4,
                "the quad collapsed to {side} by {across}"
            );
        }
    }

    /// The walk cycles rather than running off the end of the sheet, and it
    /// does actually advance -- a column that never moved would be a field of
    /// enemies sliding along frozen.
    #[test]
    fn the_walk_cycles_through_the_columns() {
        let meta = meta();
        let seen: std::collections::HashSet<usize> = (0..64)
            .map(|step| meta.cell(Quat::IDENTITY, Vec3::Z, step as f32 / 24.0).0)
            .collect();
        assert_eq!(
            seen.len(),
            meta.frames,
            "the walk only ever showed {} of its {} frames",
            seen.len(),
            meta.frames
        );
    }

    /// Cells must not overlap or leave gaps, and must stay inside the atlas.
    /// Half a texel of inset is deliberate; more than one would start eating
    /// the picture.
    #[test]
    fn the_cells_tile_the_atlas_without_bleeding_into_each_other() {
        let meta = meta();
        let (cols, rows) = meta.grid();
        let texel = 1.0 / meta.atlas_size().as_vec2();
        for row in 0..rows {
            for column in 0..cols {
                let (low, size) = meta.uv(column, row);
                let high = low + size;
                assert!(low.x >= 0.0 && low.y >= 0.0, "{low:?} is outside the atlas");
                assert!(
                    high.x <= 1.0 && high.y <= 1.0,
                    "{high:?} is outside the atlas"
                );
                // Inside its nominal cell, by no more than one texel.
                let nominal_low = Vec2::new(column as f32 / cols as f32, row as f32 / rows as f32);
                assert!((low - nominal_low).cmple(texel).all(), "{low:?}");
            }
        }
    }

    /// The whole point of `foot`: a sprite has to stand on the ground rather
    /// than be centred on it -- the enemy's origin has to land `foot` of the way
    /// up the quad, which is where it landed in the picture.
    ///
    /// Measured along the quad's **own** up rather than the world's, because
    /// the quad leans back by its tier's elevation and the world's vertical is
    /// no longer the axis the cell is laid along. The two are the same claim:
    /// the ground in the picture meets the ground in the world.
    #[test]
    fn a_sprite_stands_on_its_feet_rather_than_being_buried_to_the_waist() {
        let meta = meta();
        let at = Vec3::new(3.0, 7.0, -2.0);
        for elevation in [0.0, 15.0, 40.0, 70.0] {
            let (corners, anchor) = drawn(
                &meta,
                at + eye_at(elevation),
                Member {
                    at,
                    facing: Quat::IDENTITY,
                    phase: 0.0,
                },
            );
            // The corners run bottom left, bottom right, top right, top left.
            let (bottom, top) = (corners[0], corners[3]);
            let tier = meta.tiers[meta.tier(eye_at(elevation))];
            let along = top - bottom;
            // As a fraction of the cell rather than in metres, which is the
            // form the claim was always really in: the origin sits `foot` of
            // the way up the picture. `float_toward` scales the picture and
            // the origin together, so the fraction is what survives it.
            let stands = (anchor - bottom).dot(along.normalize()) / along.length();
            assert!(
                (stands - tier.foot).abs() < 1e-5,
                "at {elevation} degrees the origin is {stands} of the way up the \
                 quad rather than {}",
                tier.foot
            );
            // And it is still the ground the sprite meets: the picture's own
            // ground line is below the enemy's origin, never above it.
            assert!(
                bottom.y < anchor.y,
                "the quad's bottom edge floats above the feet"
            );
        }
    }

    /// A quad turns to face the camera about the vertical, and then lies back
    /// by exactly the elevation its picture was taken from -- no more and no
    /// less. Both halves matter: the turn is what shows the right side of the
    /// enemy, the lean is what stops a photograph taken from above being
    /// displayed on a surface that is not facing that way.
    ///
    /// The lean is by the *tier's* elevation rather than the camera's, so a
    /// camera at seventy degrees looking at a sheet baked at fifty-five sees
    /// the quad fifteen degrees off square, which costs three per cent of its
    /// height. An upright quad at seventy degrees would have lost two thirds.
    #[test]
    fn a_sprite_faces_the_camera_and_leans_back_by_its_own_tier() {
        let meta = meta();
        let at = Vec3::new(-4.0, 2.0, 6.0);
        for eye in [
            Vec3::new(-4.0, 2.0, 20.0),
            Vec3::new(30.0, 3.0, 6.0),
            Vec3::new(-20.0, 60.0, -10.0),
            at + eye_at(89.0),
        ] {
            let mut mesh = empty_field();
            build_field(
                &meta,
                eye,
                &[Member {
                    at,
                    facing: Quat::IDENTITY,
                    phase: 0.0,
                }],
                &mut mesh,
            );
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("no positions");
            };
            let (a, b) = (Vec3::from(p[0]), Vec3::from(p[1]));
            let across = (b - a).normalize();
            assert!(
                across.y.abs() < 1e-5,
                "the quad's width leans by {} with the camera at {eye:?}",
                across.y
            );
            let up = Vec3::from(p[3]) - a;
            let normal = across.cross(up).normalize();
            // Which way it faces, in the horizontal plane: at the camera.
            let mut flat = eye - at;
            flat.y = 0.0;
            let flat = flat.normalize();
            assert!(
                Vec3::new(normal.x, 0.0, normal.z).normalize().dot(flat) > 0.999,
                "the quad faces {normal:?} rather than {flat:?}"
            );
            // And how far it is tilted up out of that plane: the elevation the
            // row it is showing was baked at.
            let tier = meta.tiers[meta.tier(eye - at)];
            let leans = normal.y.asin().to_degrees();
            assert!(
                (leans - tier.elevation).abs() < 1e-3,
                "the quad lies back {leans} degrees showing a picture taken from \
                 {} with the camera at {eye:?}",
                tier.elevation
            );
        }
    }

    /// One draw call for the whole crowd is the entire point, so the crowd had
    /// better all be in one mesh.
    #[test]
    fn a_whole_crowd_becomes_one_mesh() {
        let meta = meta();
        let crowd: Vec<Member> = (0..1000)
            .map(|index| Member {
                at: Vec3::new(index as f32 * 0.7, 0.0, (index % 37) as f32),
                facing: Quat::from_rotation_y(index as f32 * 0.1),
                phase: index as f32 * 0.03,
            })
            .collect();
        let mut mesh = empty_field();
        build_field(&meta, Vec3::new(0.0, 20.0, -60.0), &crowd, &mut mesh);
        assert_eq!(mesh.count_vertices(), crowd.len() * 4);
        assert_eq!(
            mesh.indices().map(|indices| indices.len()),
            Some(crowd.len() * 6)
        );
    }

    /// Every cell holds a whole actor, fitted to the cell and not clipped by it.
    ///
    /// What this guards is the geometry the runtime trusts: `world_size` is the
    /// cell's extent in world units, so an actor that overflows its cell is an
    /// actor drawn cropped, and one that rattles around inside a cell far too
    /// big for it is drawn at the wrong size.
    ///
    /// What it deliberately does **not** claim to catch is the worst bug these
    /// sheets have had. The baker was running without `billboard::systems()`,
    /// so the scuttlebug's three billboard joints came out
    /// at the quarter scale the exporter baked onto the skeleton rather than
    /// having it put back -- and single-sided, so they were culled from half the
    /// angles too. The sprites covered 52% of the pixels the models did and
    /// enemies visibly shrank as they crossed the swap distance. None of that
    /// moves the silhouette: the face sits inside the head. The survey extents
    /// before and after the fix are identical to four decimal places.
    ///
    /// That one is guarded structurally instead, by the baker running
    /// [`crate::drawing`] itself rather than a copy of it.
    #[test]
    fn every_baked_cell_holds_a_whole_actor() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for kind in [Kind::Slime, Kind::Ant] {
            let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
            let path = root.join(SHEETS).join(format!("{}.png", stem(kind)));
            let (size, alpha) = png_alpha(&path);
            let cell = meta.cell_px;
            let mut biggest = 0.0_f32;
            // Every row of every tier: a sheet is only as good as its worst
            // one, and an actor that fits its camera from the side can fill it
            // from above.
            for row in 0..meta.grid().1 as u32 {
                for column in 0..meta.frames as u32 {
                    let (mut low, mut high) = (UVec2::splat(cell), UVec2::ZERO);
                    for y in 0..cell {
                        for x in 0..cell {
                            let at = ((row * cell + y) * size.x + column * cell + x) as usize;
                            if alpha[at] > 8 {
                                low = low.min(UVec2::new(x, y));
                                high = high.max(UVec2::new(x + 1, y + 1));
                            }
                        }
                    }
                    assert!(
                        high.x > low.x && high.y > low.y,
                        "{kind:?} cell {column},{row} is empty"
                    );
                    // Nothing may touch the edge, or the actor is cropped and
                    // `world_size` is a lie about what the cell contains.
                    assert!(
                        low.x > 0 && low.y > 0 && high.x < cell && high.y < cell,
                        "{kind:?} cell {column},{row} is clipped by its own edge"
                    );
                    let span = (high - low).as_vec2() / cell as f32;
                    biggest = biggest.max(span.max_element());
                }
            }
            // The baker fits the content to the cell, so the fullest cell of a
            // sheet should very nearly fill it in its larger axis. Well under
            // this means the camera was sized against something other than what
            // was drawn.
            assert!(
                biggest > 0.7,
                "{kind:?}'s fullest cell reaches only {:.0}% of the cell, so the \
                 sheet was sized against something other than the actor in it",
                biggest * 100.0
            );
        }
    }

    /// [`Kind::lift`] is a measurement of the art, so it is checked against the
    /// art rather than trusted.
    ///
    /// The sheets are renders of the real posed actor through the game's own
    /// draw chain, and their metadata says exactly where the model's origin sits
    /// inside a cell -- `foot` of a cell-height up from the bottom edge. So the
    /// lowest opaque pixel of any cell, measured down from there, is how far the
    /// actor's geometry hangs below its own transform origin. For a crawler
    /// rigged from its body rather than its feet that is a fifth of a metre or
    /// more, and seating that origin on the floor is what buried the scuttlebug
    /// to its belly in solid stone.
    ///
    /// Checking it here rather than writing the number down twice means
    /// re-baking a sheet, re-rigging an actor or changing the exporter's root
    /// offset fails this test instead of silently sinking an enemy into the
    /// ground.
    /// How far `kind`'s art hangs below its own transform origin, read off the
    /// baked sheet.
    ///
    /// The sheets are renders of the real posed actor through the game's own
    /// draw chain, and their metadata says exactly where the origin sits inside
    /// a cell -- `foot` of a cell-height up from the bottom edge. So the lowest
    /// opaque pixel of any cell, measured down from there, is the hang.
    ///
    /// **The flattest tier only.** Every objection below to reading a hang off
    /// a silhouette gets worse the higher the camera goes: from fifty-five
    /// degrees a body reaching towards the camera projects most of its own
    /// length below the origin, and the steep rows would report an actor
    /// hanging half a metre under a floor it is standing flat on. The flat
    /// rows are the ones this measurement was ever meant for.
    ///
    /// Shared with `enemy`'s placement test rather than measured twice. That
    /// test needs a number that does **not** come from [`Kind::lift`], or it
    /// merely subtracts back out whatever the placement put in and passes with
    /// the lift set to zero -- which two earlier versions of it duly did.
    pub(crate) fn hang_in_sheet(kind: Kind) -> f32 {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
        let path = root.join(SHEETS).join(format!("{}.png", stem(kind)));
        let (size, alpha) = png_alpha(&path);
        let cell = meta.cell_px;
        let tier = meta.tiers[0];
        // A cell is a picture taken from `elevation` above, which foreshortens
        // world height by its cosine -- so a pixel of the picture is rather
        // more than a pixel's worth of metres in the world.
        let metres = meta.world_size / cell as f32 / tier.elevation.to_radians().cos();
        // Where the origin is, counted up from the bottom edge of a cell.
        let origin = tier.foot * cell as f32;
        let mut hangs = f32::MIN;
        for row in 0..meta.angles as u32 {
            for column in 0..meta.frames as u32 {
                for y in (0..cell).rev() {
                    let line = (row * cell + y) * size.x + column * cell;
                    if (0..cell).any(|x| alpha[(line + x) as usize] > 8) {
                        let above_bottom = (cell - 1 - y) as f32;
                        hangs = hangs.max((origin - above_bottom) * metres);
                        break;
                    }
                }
            }
        }
        hangs
    }

    /// What the sheets can and cannot settle about [`Kind::lift`].
    ///
    /// They settled it exactly for the scuttlebug, which is the actor the
    /// constant was written for: a tall, narrow bug whose hang was a rig-root
    /// offset, the same on every frame and from every angle, so the lowest
    /// opaque pixel of a cell really was where the model stopped.
    ///
    /// Neither actor that ships now is that shape, and what the instrument
    /// cannot do is worth writing down rather than rediscovering. A sheet cell
    /// is a *silhouette*, drawn by a camera tilted `ELEVATION` degrees down, so
    /// the near rim of a wide body projects below where its own origin projects
    /// even when there is nothing at all below it in world space -- half a
    /// metre of body reaching towards the camera buys 13 cm of that at 15
    /// degrees. The measurement also takes the deepest frame of the walk rather
    /// than any frame the actor rests at.
    ///
    /// Between them those put the slime at 0.33 m when the posed mesh says it
    /// rests on 0.000 and dips to 0.177 for a few frames of `Scoot_Move`, and
    /// the ant at 0.36 m against a rig that plants its feet 0.216 below its
    /// origin and holds them there all the way through both its clips.
    /// Believing either number would hover the actor to keep its own picture
    /// off a floor it is standing on.
    ///
    /// So what is asserted here is what remains true either way: a lift is
    /// never more than the silhouette reaches, because a model held further up
    /// than its own picture ever extends is a model hovering.
    /// `tools/measure_actor_hang.py` is what settles a resting offset against a
    /// transient dip when the two disagree.
    #[test]
    fn the_lift_matches_what_the_baked_sheets_show() {
        for kind in [Kind::Slime, Kind::Ant] {
            let measured = hang_in_sheet(kind);
            let claimed = kind.lift();
            assert!(
                claimed <= measured + 0.02,
                "{kind:?} is lifted {claimed:.3} m but its silhouette only \
                 ever reaches {measured:.3} m below its origin, so it hovers"
            );
            assert!(claimed >= 0.0, "{kind:?} is lifted downwards");
        }
    }

    /// That a sheet is a picture of the model the game is loading now.
    ///
    /// The two halves of an actor's size are measured in completely different
    /// ways, and this is the only thing that puts them in the same room.
    /// `enemy::Kind::body` reads the glTF's own position bounds -- a header,
    /// never rendered -- while `world_size` is what the bake camera had to cover
    /// to fit the actor on screen, which is the *renderer's* answer, through
    /// skinning, billboards and every node transform in the file. A model whose
    /// drawn size differs from its authored bounds shows up here and nowhere
    /// else.
    ///
    /// It is also the guard against the ordinary version of the same mistake:
    /// resizing an actor and re-exporting it without re-baking, which draws it
    /// at the new size up close and the old one past `enemy_draw`.
    ///
    /// The band is wide on purpose. A cell has to hold a walk cycle's widest
    /// pose plus `FINAL_MARGIN`, and it is square, so a flat wide actor's cell
    /// is much taller than the actor -- the slime's is 1.26x its own width.
    /// What it is looking for is a factor, not a percentage. It reaches slightly
    /// below 1.0 because a bind pose can be wider than anything the actor ever
    /// does -- a T-pose is the standard example -- and the cell is fitted to the
    /// poses.
    #[test]
    fn the_sheets_agree_with_the_models_they_were_baked_from() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for kind in [Kind::Slime, Kind::Ant] {
            let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
            let (radius, height) = kind.body();
            let across = (radius * 2.0).max(height);
            assert!(
                (0.95..2.0).contains(&(meta.world_size / across)),
                "{kind:?}'s model is {across:.3} m at its largest but its sheet was \
                 baked to cover {:.3} m. Either the model was re-exported at a \
                 different size without re-baking -- run tools/build_assets.py \
                 --only impostors -- or something in the file is scaling what the \
                 renderer draws.",
                meta.world_size,
            );
        }
    }

    /// The Windows package has to actually contain the sheets.
    ///
    /// This is the test for the bug that made all of the above pointless in the
    /// build people actually play. `build_windows.sh` copies a *named list* of
    /// assets rather than the whole tree -- deliberately, because the sound
    /// directories hold thousands of files -- and the impostor sheets were
    /// simply never added to it. The packaged game therefore started normally,
    /// found no atlas, drew no sprites at all, and every enemy past
    /// `enemy_draw` was nothing whatsoever until you walked close enough for it
    /// to become a model.
    ///
    /// Nothing catches that at runtime: the failure is a line on a stderr that a
    /// `windows_subsystem = "windows"` build has nobody attached to, and every
    /// test in this file passes because they all read the *source* tree. The
    /// same trap the shader is guarded against in `display.rs`, sprung on a
    /// different asset.
    ///
    /// So the guard is here, on the packaging script itself: whatever this
    /// module loads at runtime, the script must be seen to copy.
    #[test]
    fn the_windows_package_ships_the_sheets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = std::fs::read_to_string(root.join("build_windows.sh"))
            .expect("build_windows.sh has gone missing");
        assert!(
            script.contains(&format!("assets/{SHEETS}")),
            "build_windows.sh does not copy assets/{SHEETS}, so the packaged \
             game will draw no distant enemies at all"
        );
        for kind in [Kind::Slime, Kind::Ant] {
            for suffix in ["png", "json"] {
                let file = format!("{}.{suffix}", stem(kind));
                assert!(
                    root.join("assets").join(SHEETS).join(&file).is_file(),
                    "{SHEETS}/{file} is loaded at runtime but is not in the tree"
                );
            }
        }
    }

    /// The sheets have to carry a tier for looking down at, and their tiers
    /// have to be in the order the layout assumes.
    ///
    /// This is the guard on the thing a re-bake can quietly undo. Drop
    /// `bake::ELEVATIONS` back to one entry and everything else still passes:
    /// the atlas matches its sidecar, every cell holds an actor, the sizes
    /// agree. The game simply goes back to showing a field of enemies
    /// photographed from their own eye level to a camera looking down at
    /// seventy degrees, which is a thing you have to be standing somewhere
    /// specific to notice.
    #[test]
    fn the_committed_sheets_can_be_looked_down_on() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for kind in [Kind::Slime, Kind::Ant] {
            let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
            assert!(
                meta.tiers.len() >= 2,
                "{kind:?}'s sheet was baked from only {:?}, so there is no picture \
                 of it from above -- re-bake with tools/build_assets.py --only impostors",
                meta.tiers,
            );
            assert!(
                meta.tiers.windows(2).all(|pair| pair[0].elevation < pair[1].elevation),
                "{kind:?}'s tiers are not flattest first: {:?}",
                meta.tiers,
            );
            let steepest = meta.tiers.last().expect("no tiers").elevation;
            // The camera's own pitch reaches 43 degrees down and the ground
            // under the crowd adds to that. A sheet whose steepest picture is
            // flatter than the camera can look is a sheet with a blind spot.
            assert!(
                steepest > 45.0,
                "{kind:?}'s steepest tier is only {steepest} degrees up"
            );
            for tier in &meta.tiers {
                assert!(
                    (0.0..1.0).contains(&tier.foot),
                    "{kind:?}'s {tier:?} puts the origin outside its own cell"
                );
            }
        }
    }

    /// The sheets on disk have to be the ones this code thinks it is reading.
    /// A re-bake that changed the layout and not the sidecar would otherwise
    /// show up as a field of enemies playing the wrong frame.
    #[test]
    fn the_committed_sheets_match_their_atlases() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for kind in [Kind::Slime, Kind::Ant] {
            let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
            let path = root.join(SHEETS).join(format!("{}.png", stem(kind)));
            assert_eq!(
                png_size(&path),
                meta.atlas_size(),
                "{} does not match the size its sidecar describes",
                path.display()
            );
        }
    }
}
