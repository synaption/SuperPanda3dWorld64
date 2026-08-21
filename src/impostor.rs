//! Distant enemies, drawn as sprites instead of as skeletons.
//!
//! This is the module the crowd budget lives or dies on, so it is worth saying
//! plainly what it is for.
//!
//! Bevy marks every skinned mesh `NoAutomaticBatching`, because each one needs
//! its own joint matrices, so **a skinned model is one draw call per mesh
//! primitive and no two of them ever merge**. The actors here are split by
//! material: two primitives for a slime, fifteen for a scuttlebug -- fifteen
//! draw calls to put seventy-six triangles on the screen. A field of two
//! thousand is around seventeen thousand draw calls a frame, which is the whole
//! frame, and no amount of lowering the internal resolution touches it because
//! the cost is in submitting the draws rather than in filling the pixels.
//!
//! An impostor is the standard answer: past the distance where a model is a few
//! pixels tall, replace it with a flat quad showing a picture of that model,
//! taken from the angle you happen to be looking from. The pictures are baked
//! ahead of time into one atlas per enemy kind -- every viewing angle across the
//! rows, every frame of its walk cycle across the columns.
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
/// angles or a longer clip is a change to two files in `assets/` rather than a
/// change to this module.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct SheetMeta {
    /// Which actor this was baked from, for the error message when it is not
    /// the one that was asked for.
    pub model: String,
    /// Pixels along one side of a cell.
    pub cell_px: u32,
    /// Distinct viewing angles, evenly spaced around the model. One per row.
    pub angles: usize,
    /// Frames of the walk cycle. One per column.
    pub frames: usize,
    /// How wide and tall a cell is in world units, which is the size of the
    /// quad it gets drawn onto.
    pub world_size: f32,
    /// Where the model's own origin -- the point its `Transform` puts on the
    /// ground -- sits in the cell, as a fraction of the cell's height measured
    /// up from the bottom edge.
    ///
    /// Without it a sprite is centred on the enemy's feet and the enemy appears
    /// to be buried to the waist. With it the quad is lifted so that the ground
    /// in the picture lands on the ground in the world.
    pub foot: f32,
    /// How fast to play the columns, in frames a second.
    pub fps: f32,
    /// The camera's downward tilt when the sheet was baked, in degrees. Not
    /// read at runtime -- the quads are upright whatever it was -- but recorded
    /// because a sheet baked at one tilt and viewed from a camera at a very
    /// different one is the reason an impostor crowd can look subtly wrong, and
    /// a number in the file is how that gets diagnosed.
    pub elevation: f32,
}

impl SheetMeta {
    /// Columns and rows the atlas is expected to have. The layout is fixed:
    /// **one row per angle, one column per frame.**
    pub fn grid(&self) -> (usize, usize) {
        (self.frames, self.angles)
    }

    /// How big the atlas has to be for this description to be true of it.
    pub fn atlas_size(&self) -> UVec2 {
        let (cols, rows) = self.grid();
        UVec2::new(cols as u32 * self.cell_px, rows as u32 * self.cell_px)
    }

    /// Which cell shows a model turned to `yaw` seen from the direction
    /// `to_camera`, `phase` seconds into its walk.
    ///
    /// The angle is the bearing of the camera *in the model's own frame*, so
    /// turning the enemy and orbiting the camera pick the same picture -- which
    /// is what makes a slime crawling away from you show you its back.
    ///
    /// Rounded to the nearest angle rather than truncated, so the error is half
    /// a step either way instead of a whole step in one direction, and wrapped
    /// rather than clamped, because a bearing is a circle.
    pub fn cell(&self, yaw: f32, to_camera: Vec3, phase: f32) -> (usize, usize) {
        let bearing = to_camera.x.atan2(to_camera.z) - yaw;
        let step = std::f32::consts::TAU / self.angles as f32;
        let row = (bearing / step).round() as i64;
        let row = row.rem_euclid(self.angles as i64) as usize;
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
    scuttlebug: Option<Sheet>,
    /// Every distant enemy's shadow, of every kind, in one mesh.
    ///
    /// One mesh rather than one per kind because a shadow is a shadow: they all
    /// share the disc texture and the solid rung of the fade ladder, so nothing
    /// distinguishes a slime's from a scuttlebug's except its radius. That
    /// makes the whole far crowd's shadows a single extra draw call.
    shadows: Option<Handle<Mesh>>,
}

impl Impostors {
    fn get(&self, kind: Kind) -> Option<&Sheet> {
        match kind {
            Kind::Slime => self.slime.as_ref(),
            Kind::Scuttlebug => self.scuttlebug.as_ref(),
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
        Kind::Scuttlebug => "scuttlebug",
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
    for kind in [Kind::Slime, Kind::Scuttlebug] {
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
            Kind::Scuttlebug => impostors.scuttlebug = Some(sheet),
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
    /// Which way it is facing, as a heading about the vertical.
    pub yaw: f32,
    /// How far into its walk cycle it is, in seconds.
    pub phase: f32,
}

/// Writes a crowd into a mesh as camera-facing quads.
///
/// Kept out of the system so that the whole of the geometry can be checked
/// without a renderer: this is the part that can put a sprite at the wrong
/// height, the wrong size, or facing the wrong way.
///
/// The quads turn about the vertical only, like everything else billboarded
/// here -- see [`crate::billboard`] -- so a camera looking down at the field
/// does not tip a thousand sprites onto their backs.
pub fn build_field(meta: &SheetMeta, eye: Vec3, crowd: &[Member], mesh: &mut Mesh) {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(crowd.len() * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(crowd.len() * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(crowd.len() * 6);
    let half = meta.world_size * 0.5;
    for member in crowd {
        let mut away = eye - member.at;
        away.y = 0.0;
        let Some(away) = away.try_normalize() else {
            // The camera is directly overhead, where a vertical quad is edge on
            // and there is no bearing to pick a picture by either. Nothing
            // useful can be drawn, and drawing nothing is the honest answer.
            continue;
        };
        // Across the view, in the horizontal plane. The quad's own up is the
        // world's up, which is what keeps it standing.
        let right = Vec3::Y.cross(away);
        let (uv, size) = {
            let (column, row) = meta.cell(member.yaw, away, member.phase);
            meta.uv(column, row)
        };
        // The cell's bottom edge sits `foot` of a cell-height below the origin,
        // so that the ground in the picture meets the ground in the world.
        let bottom = member.at - Vec3::Y * (meta.world_size * meta.foot);
        let base = positions.len() as u32;
        for (corner, (u, v)) in [
            (-half, 0.0),
            (half, 0.0),
            (half, meta.world_size),
            (-half, meta.world_size),
        ]
        .into_iter()
        .zip([(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)])
        {
            let (across, up) = corner;
            positions.push((bottom + right * across + Vec3::Y * up).to_array());
            uvs.push((uv + Vec2::new(u, v) * size).to_array());
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
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
        *buffers = vec![(Kind::Slime, Vec::new()), (Kind::Scuttlebug, Vec::new())];
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
        let (axis, angle) = transform.rotation.to_axis_angle();
        members.push(Member {
            at: transform.translation,
            // The walkers are turned about the vertical and this reads their
            // heading straight back; a crawler stuck to a wall is turned about
            // some other axis, and the sign keeps its sprite facing the way it
            // is going rather than the mirror of it.
            yaw: angle * axis.y.signum(),
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
            foot: 0.15,
            fps: 12.0,
            elevation: 15.0,
        }
    }

    /// Facing the camera picks the front, turning away picks the back, and the
    /// two are half the sheet apart. This is the property that makes a crowd
    /// read as a crowd of individuals rather than a wall of identical cards.
    #[test]
    fn the_cell_follows_the_angle_between_the_model_and_the_camera() {
        let meta = meta();
        let front = meta.cell(0.0, Vec3::Z, 0.0).1;
        let back = meta.cell(std::f32::consts::PI, Vec3::Z, 0.0).1;
        assert_eq!(front, 0, "a model facing the camera is the first row");
        assert_eq!(
            back,
            meta.angles / 2,
            "a model facing away should be half the sheet round"
        );
        // Turning the model and orbiting the camera are the same thing.
        let turned = meta.cell(std::f32::consts::FRAC_PI_2, Vec3::Z, 0.0).1;
        let orbited = meta.cell(0.0, Vec3::X, 0.0).1;
        assert_eq!(turned, meta.angles - orbited, "{turned} vs {orbited}");
    }

    /// A bearing is a circle, so every angle has to land on a real row --
    /// including the ones that round past the end of the sheet, which is where
    /// an index panic would live.
    #[test]
    fn every_bearing_lands_on_a_row_that_exists() {
        let meta = meta();
        for step in -720..=720 {
            let yaw = step as f32 * 0.05;
            let (column, row) = meta.cell(yaw, Vec3::new(yaw.cos(), 0.0, yaw.sin()), yaw.abs());
            assert!(row < meta.angles, "bearing {yaw} chose row {row}");
            assert!(column < meta.frames, "phase {yaw} chose column {column}");
        }
    }

    /// The walk cycles rather than running off the end of the sheet, and it
    /// does actually advance -- a column that never moved would be a field of
    /// enemies sliding along frozen.
    #[test]
    fn the_walk_cycles_through_the_columns() {
        let meta = meta();
        let seen: std::collections::HashSet<usize> = (0..64)
            .map(|step| meta.cell(0.0, Vec3::Z, step as f32 / 24.0).0)
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
    /// than be centred on it. With the origin 15% up the cell, a 1.2-unit quad
    /// reaches from 0.18 below the enemy's feet to 1.02 above them.
    #[test]
    fn a_sprite_stands_on_its_feet_rather_than_being_buried_to_the_waist() {
        let meta = meta();
        let mut mesh = empty_field();
        let at = Vec3::new(3.0, 7.0, -2.0);
        build_field(
            &meta,
            Vec3::new(3.0, 7.0, 10.0),
            &[Member {
                at,
                yaw: 0.0,
                phase: 0.0,
            }],
            &mut mesh,
        );
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the field has no positions");
        };
        let low = positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let high = positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!(
            (low - (at.y - meta.world_size * meta.foot)).abs() < 1e-5,
            "the sprite's bottom edge is at {low}, not below the feet at {}",
            at.y
        );
        assert!(
            (high - low - meta.world_size).abs() < 1e-5,
            "the sprite is {} tall rather than {}",
            high - low,
            meta.world_size
        );
    }

    /// A quad turns to face the camera about the vertical only, and stays
    /// upright when the camera looks down on it from a great height.
    #[test]
    fn a_sprite_faces_the_camera_and_never_tips_over() {
        let meta = meta();
        let at = Vec3::new(-4.0, 2.0, 6.0);
        for eye in [
            Vec3::new(-4.0, 2.0, 20.0),
            Vec3::new(30.0, 3.0, 6.0),
            Vec3::new(-20.0, 60.0, -10.0),
        ] {
            let mut mesh = empty_field();
            build_field(
                &meta,
                eye,
                &[Member {
                    at,
                    yaw: 0.0,
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
            // Its normal points at the camera, in the horizontal plane.
            let up = Vec3::from(p[3]) - a;
            let normal = across.cross(up).normalize();
            let mut wanted = eye - at;
            wanted.y = 0.0;
            assert!(
                normal.dot(wanted.normalize()).abs() > 0.999,
                "the quad faces {normal:?} rather than {:?}",
                wanted.normalize()
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
                yaw: index as f32 * 0.1,
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
    /// at the quarter scale the exporter bakes onto the skeleton rather than
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
        for kind in [Kind::Slime, Kind::Scuttlebug] {
            let meta = read_meta(&root, kind).expect("a committed sheet failed to load");
            let path = root.join(SHEETS).join(format!("{}.png", stem(kind)));
            let (size, alpha) = png_alpha(&path);
            let cell = meta.cell_px;
            let mut biggest = 0.0_f32;
            for row in 0..meta.angles as u32 {
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
    /// actor's geometry hangs below its own transform origin. For the
    /// scuttlebug that is a third of a metre, and seating that origin on the
    /// floor is what buried the bug to its belly in solid stone.
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
        let metres = meta.world_size / cell as f32;
        // Where the origin is, counted up from the bottom edge of a cell.
        let origin = meta.foot * cell as f32;
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
    /// For the scuttlebug they settle it exactly, and that is the case the
    /// constant was written for: its hang is a rig-root offset, the same on
    /// every frame and from every angle, so the lowest opaque pixel of a cell
    /// really is where the model stops.
    ///
    /// The slime taught us what the instrument cannot do, and it is worth
    /// writing down rather than rediscovering. A sheet cell is a *silhouette*,
    /// drawn by a camera tilted `ELEVATION` degrees down, so the near rim of a
    /// wide body projects below where its own origin projects even when there
    /// is nothing at all below it in world space -- and the measurement takes
    /// the deepest frame of the walk, which for the slime is the middle of a
    /// squash rather than anything it rests at. Between them those put the
    /// slime at 0.33 m when the posed mesh says it rests on 0.000 and dips to
    /// 0.177 for a few frames of `Scoot_Move`. Believing 0.33 would float a
    /// 0.70 m creature by half its height to stop a squash touching the floor
    /// it is squashing against.
    ///
    /// So what is asserted here is what remains true either way: a lift is
    /// never more than the silhouette reaches, because a model held further up
    /// than its own picture ever extends is a model hovering. The number
    /// itself comes from `tools/measure_actor_hang.py`, which evaluates the
    /// skinned mesh frame by frame and can tell a resting offset from a dip.
    #[test]
    fn the_lift_matches_what_the_baked_sheets_show() {
        let measured = hang_in_sheet(Kind::Scuttlebug);
        let claimed = Kind::Scuttlebug.lift();
        assert!(
            (measured - claimed).abs() < 0.02,
            "the scuttlebug hangs {measured:.3} m below its origin in the \
             sheets but `Kind::lift` claims {claimed:.3} m -- an enemy seated \
             on the ground will be that far into it"
        );
        for kind in [Kind::Slime, Kind::Scuttlebug] {
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
        for kind in [Kind::Slime, Kind::Scuttlebug] {
            for suffix in ["png", "json"] {
                let file = format!("{}.{suffix}", stem(kind));
                assert!(
                    root.join("assets").join(SHEETS).join(&file).is_file(),
                    "{SHEETS}/{file} is loaded at runtime but is not in the tree"
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
        for kind in [Kind::Slime, Kind::Scuttlebug] {
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
