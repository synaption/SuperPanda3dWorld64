//! The water sheet, and the view from under it.
//!
//! Ported from `sm64py/level.py`'s `build_water_surface`/`animate_water` and
//! the camera medium in `app/main.py`. On the castle, water is not part of the
//! level mesh -- it is the axis-aligned boxes the collision data carries,
//! drawn as one flat quad at each box's height, unlit and half transparent,
//! seen from both sides because most of the time it is looked at from
//! underneath.
//!
//! A planet's sea is the other way round: it is one sphere at sea level,
//! generated with the terrain and shipped in the same glTF, so this file finds
//! it rather than building it. The two meet at [`camera_medium`], which asks
//! the level how deep the camera is and gets an answer without knowing which
//! kind of world it is in.
//!
//! Every constant here is the Panda3D build's, converted from SM64 units to
//! the port's world scale of 1/100.

use crate::level::{LevelData, WaterBox};
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    pbr::{DistanceFog, FogFalloff},
    prelude::*,
};

/// The alpha the moving-texture data gives the water quads, 0x96 of 0xFF.
const WATER_ALPHA: f32 = 0x96 as f32 / 255.0;

/// How many times the texture repeats per world unit. The original sizes its
/// UVs per quad rather than per box; repeating on a fixed world scale keeps
/// the wave size consistent whatever the box measures.
const UV_SCALE: f32 = 1.0 / 20.48;

/// How fast the surface drifts, in world units per second, and which way.
///
/// A translation rather than a spin, and for a concrete reason: rotating the
/// UVs moves every point by its distance from the centre of rotation, so one
/// corner of a 150-unit water box crawls while the opposite corner races. The
/// two bodies drift apart so they do not read as one sheet.
const DRIFT_SPEED: f32 = 0.25;
const DRIFT_DIRECTIONS: [Vec2; 2] = [Vec2::new(0.60, 0.80), Vec2::new(-0.80, 0.60)];

/// The castle waterfall is not a collision water box. It is SM64's separate
/// 15-vertex `MOVTEX_CASTLE_WATERFALL` mesh, whose first texture coordinate is
/// advanced by 70/1024 of a repeat on every 30 Hz game tick.
const WATERFALL_UV_SPEED: f32 = 70.0 * 30.0 / 1024.0;
const WATERFALL_ALPHA: f32 = 0xb4 as f32 / 255.0;
const WATERFALL_VERTICES: [[i16; 5]; 15] = [
    [-4469, -800, -6413, 0, 0],
    [-5525, 1171, -7026, 2, 0],
    [-6292, 2028, -7463, 4, 0],
    [-7302, 2955, -7461, 6, 0],
    [-4883, -800, -5690, 0, 3],
    [-5547, 1110, -6097, 2, 3],
    [-6732, 2587, -6770, 4, 3],
    [-7603, 3004, -7160, 6, 3],
    [-5580, -800, -4740, 0, 6],
    [-6205, 1068, -5347, 2, 6],
    [-7249, 2566, -6192, 4, 6],
    [-6895, -800, -4714, 0, 9],
    [-7201, 1083, -5071, 2, 9],
    [-7578, 2042, -5766, 4, 9],
    [-8132, 2961, -6761, 6, 9],
];
const WATERFALL_INDICES: [u32; 51] = [
    0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 6, 3, 7, 6, 4, 5, 9, 4, 9, 8, 5, 6, 9, 6, 10, 9, 6,
    7, 10, 8, 9, 12, 8, 12, 11, 9, 10, 13, 9, 13, 12, 10, 7, 14, 10, 14, 13,
];

/// Sky and underwater colours, shared with the fog and the clear colour.
pub const SKY_COLOUR: Color = Color::srgb(0.32, 0.60, 0.86);
pub const UNDERWATER_COLOUR: Color = Color::srgb(0.06, 0.28, 0.36);

/// Where the haze starts and where it becomes total, above and below water.
/// Underwater the view closes in hard: that is what sells being submerged far
/// more than the surface quad does, because the water is a single flat sheet
/// with nothing behind it and the camera below the line would otherwise look
/// identical to the camera above it.
const AIR_FOG: (f32, f32) = (90.0, 200.0);
const UNDERWATER_FOG: (f32, f32) = (2.0, 42.0);

/// A drawn water sheet, holding what the drift needs to move it.
#[derive(Component)]
pub struct WaterSurface {
    uv_velocity: Vec2,
    /// The UVs at rest, so drift is an offset from a fixed base rather than an
    /// accumulation that loses precision as the session runs on.
    base_uvs: Vec<[f32; 2]>,
}

/// The fog the camera is currently in, so the swap only runs on a change.
#[derive(Resource, Default, PartialEq, Clone, Copy)]
pub struct CameraMedium {
    submerged: bool,
}

/// Air fog for the camera at startup. Bevy applies fog per camera rather than
/// per scene, so the camera carries this and [`camera_medium`] retunes it.
pub fn air_fog() -> DistanceFog {
    DistanceFog {
        color: SKY_COLOUR,
        falloff: FogFalloff::Linear {
            start: AIR_FOG.0,
            end: AIR_FOG.1,
        },
        ..default()
    }
}

/// Where a box's sheet sits in the world. The mesh is built around this
/// rather than in world coordinates, because Bevy sorts transparent objects by
/// the distance to their *origin*: a sheet whose vertices are in world space
/// has its origin at the map origin, which is nowhere near the water and makes
/// it sort against the fence and the castle doorway as if it were there.
fn centre(water: &WaterBox) -> Vec3 {
    Vec3::new(
        (water.min_x + water.max_x) * 0.5,
        water.surface_y,
        (water.min_z + water.max_z) * 0.5,
    )
}

/// Builds the quad for one box: four corners at the surface height, local to
/// [`centre`], with UVs taken from *world* position so the texture tiles at a
/// fixed size however the sheet is placed.
fn quad(water: &WaterBox) -> Mesh {
    let origin = centre(water);
    let corners = [
        [water.min_x, water.min_z],
        [water.max_x, water.min_z],
        [water.max_x, water.max_z],
        [water.min_x, water.max_z],
    ];
    let positions: Vec<[f32; 3]> = corners
        .iter()
        .map(|[x, z]| [x - origin.x, 0.0, z - origin.z])
        .collect();
    let uvs: Vec<[f32; 2]> = corners
        .iter()
        .map(|[x, z]| [x * UV_SCALE, z * UV_SCALE])
        .collect();
    // A mesh now declares which worlds keep a copy of it, and `MAIN_WORLD` is
    // load-bearing here rather than a default worth trimming: `drift` rewrites
    // this sheet's UVs every frame through `Assets<Mesh>`, and a mesh kept only
    // in the render world has nothing left on the CPU side to rewrite -- the
    // lookup returns `None` and the water silently stops moving.
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

/// Rebuilds the exact curved waterfall strip from the original N64 movtex
/// data. Keeping it local to its centre gives Bevy a useful transparent-sort
/// origin, just as it does for the horizontal sheets.
fn waterfall() -> (Mesh, Vec3) {
    const SCALE: f32 = 0.01;
    let origin = WATERFALL_VERTICES.iter().fold(Vec3::ZERO, |sum, vertex| {
        sum + Vec3::new(vertex[0] as f32, vertex[1] as f32, vertex[2] as f32) * SCALE
    }) / WATERFALL_VERTICES.len() as f32;
    let positions: Vec<[f32; 3]> = WATERFALL_VERTICES
        .iter()
        .map(|vertex| {
            (Vec3::new(vertex[0] as f32, vertex[1] as f32, vertex[2] as f32) * SCALE - origin)
                .to_array()
        })
        .collect();
    // Movtex coordinates are 6.10 fixed point; the integer offsets in the
    // level data therefore name whole texture repeats.
    let uvs: Vec<[f32; 2]> = WATERFALL_VERTICES
        .iter()
        .map(|vertex| [vertex[3] as f32, vertex[4] as f32])
        .collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 15]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(WATERFALL_INDICES.to_vec()));
    (mesh, origin)
}

/// Spawns one sheet per water box. Called from startup with the loaded level.
pub fn spawn(
    commands: &mut Commands,
    assets: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    level: &LevelData,
) {
    // The sheet tiles, and Bevy clamps by default, so the sampler has to be
    // asked for repetition at load time.
    let texture = assets
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..default()
            });
        })
        .load("bevy/water.png");
    for (index, water) in level.water_boxes.iter().enumerate() {
        let mesh = quad(water);
        let base_uvs = uvs_of(&mesh);
        let material = materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 1.0, 1.0, WATER_ALPHA),
            base_color_texture: Some(texture.clone()),
            alpha_mode: AlphaMode::Blend,
            // The level mesh carries baked vertex colour and the water is a
            // flat sheet, so neither wants lighting.
            unlit: true,
            // Seen from underneath as well, which is most of the time while
            // swimming.
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        commands.spawn((
            WaterSurface {
                uv_velocity: DRIFT_DIRECTIONS[index % DRIFT_DIRECTIONS.len()]
                    * DRIFT_SPEED
                    * UV_SCALE,
                base_uvs,
            },
            // A transparent sheet has no business darkening the lakebed.
            NotShadowCaster,
            NotShadowReceiver,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::from_translation(centre(water)),
        ));
    }

    let (mesh, origin) = waterfall();
    let base_uvs = uvs_of(&mesh);
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, WATERFALL_ALPHA),
        base_color_texture: Some(texture),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        WaterSurface {
            // SM64 advances the S coordinate, not T, for this movtex.
            uv_velocity: Vec2::new(WATERFALL_UV_SPEED, 0.0),
            base_uvs,
        },
        NotShadowCaster,
        NotShadowReceiver,
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        Transform::from_translation(origin),
    ));
}

/// The planet's sea: one node of the planet's own glTF, tagged on arrival.
#[derive(Component)]
pub struct Ocean;

/// The axis the sea turns about. Any fixed axis will do -- it is a sphere --
/// so this is the castle's first drift direction, laid on its side.
const OCEAN_AXIS: Vec3 = Vec3::new(0.60, 0.80, 0.0);

/// Finds the sea in the planet's scene and marks it, once.
///
/// The sea arrives as geometry rather than as something this game builds,
/// because sea level is the generator's number and a mesh is how it is carried
/// across -- `src/world.rs` reads the same node for the radius it swims
/// against. All that is wanted here is a handle on the entity, and the name
/// off the glTF node is the only thing that identifies it: by the time
/// anything else could, [`crate::n64::convert`] has already moved it onto the
/// port's own material along with the rest of the scene.
pub fn find_ocean(mut commands: Commands, arrivals: Query<(Entity, &Name), Added<Name>>) {
    for (entity, name) in &arrivals {
        if is_the_sea(name.as_str()) {
            commands.entity(entity).insert(Ocean);
        }
    }
}

/// Is this the glTF node holding the sea, rather than something under it?
///
/// `planetgen` names the node `ocean`, or `ocean_lod1` for the space mesh. The
/// glTF loader then names the primitive hanging off it after its mesh and its
/// material -- `ocean.PlanetOcean` -- so a prefix match finds the sea twice,
/// once as itself and once as its own child. Tagging both would turn the sea
/// at double speed, the child riding on the parent's rotation and adding its
/// own.
fn is_the_sea(name: &str) -> bool {
    name == "ocean" || name.starts_with("ocean_")
}

/// Drifts the sea by turning it.
///
/// The flat sheets in [`drift`] move their UVs, which is four vertices' worth
/// of work and no material support at all. Neither is available here: the sea
/// is 25,344 vertices, and it is drawn with [`crate::n64::N64Material`], which
/// has no texture transform to offset -- the console it imitates had nowhere
/// to put one.
///
/// A sphere spun about its own centre occupies exactly the space it did
/// before, so the turn moves nothing except what is drawn on it: the ripples
/// slide across the surface and the coastline stays where it is. Rotating a
/// sheet is the mistake the castle's water deliberately avoids -- one corner
/// crawls while the other races -- and a globe is the one shape where it is
/// not, because every point of it is the same distance from the axis except
/// the two the axis runs through.
pub fn drift_ocean(
    time: Res<Time>,
    level: Res<LevelData>,
    mut sea: Query<&mut Transform, With<Ocean>>,
) {
    // Radians a second that put the surface at the same metres a second the
    // castle's sheets drift at, whatever size the planet turns out to be.
    let Some(spin) = level.sea_radius().map(|radius| DRIFT_SPEED / radius.max(1.0)) else {
        return;
    };
    for mut transform in &mut sea {
        transform.rotation = Quat::from_axis_angle(OCEAN_AXIS.normalize(), spin * time.elapsed_secs());
    }
}

fn uvs_of(mesh: &Mesh) -> Vec<[f32; 2]> {
    if let Some(bevy::render::mesh::VertexAttributeValues::Float32x2(uvs)) =
        mesh.attribute(Mesh::ATTRIBUTE_UV_0)
    {
        return uvs.clone();
    }
    Vec::new()
}

/// Drifts each sheet's texture across the surface.
///
/// The offset is written into the mesh's UVs because Bevy's standard material
/// has no texture transform of its own. Four vertices per body of water makes
/// that cheaper than the custom material and shader the alternative would
/// need.
pub fn drift(
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    surfaces: Query<(&WaterSurface, &Mesh3d)>,
) {
    for (surface, handle) in &surfaces {
        let Some(mut mesh) = meshes.get_mut(&handle.0) else {
            continue;
        };
        let offset = surface.uv_velocity * time.elapsed_secs();
        let moved: Vec<[f32; 2]> = surface
            .base_uvs
            .iter()
            .map(|uv| [uv[0] + offset.x, uv[1] + offset.y])
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, moved);
    }
}

/// Swaps the fog and the sky for whichever side of the surface the camera is
/// on.
///
/// Tested against the camera rather than the player: swimming just below the
/// surface leaves the camera in open air looking down through it, and tinting
/// the whole world in that case looks wrong.
pub fn camera_medium(
    level: Res<LevelData>,
    mut medium: ResMut<CameraMedium>,
    mut clear: ResMut<ClearColor>,
    mut cameras: Query<(&Transform, &mut DistanceFog), With<Camera3d>>,
) {
    let Ok((camera, mut fog)) = cameras.single_mut() else {
        return;
    };
    let submerged = level
        .water_depth(camera.translation)
        .is_some_and(|depth| depth > 0.0);
    if submerged == medium.submerged {
        return;
    }
    medium.submerged = submerged;
    let (colour, range) = if submerged {
        (UNDERWATER_COLOUR, UNDERWATER_FOG)
    } else {
        (SKY_COLOUR, AIR_FOG)
    };
    fog.color = colour;
    fog.falloff = FogFalloff::Linear {
        start: range.0,
        end: range.1,
    };
    clear.0 = colour;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(surface_y: f32) -> WaterBox {
        WaterBox {
            min_x: -71.29,
            min_z: -72.22,
            max_x: 82.53,
            max_z: -0.58,
            surface_y,
        }
    }

    #[test]
    fn a_sheet_covers_its_box_at_the_surface_height() {
        let water = box_at(-0.81);
        let mesh = quad(&water);
        let Some(bevy::render::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the quad has no positions");
        };
        assert_eq!(positions.len(), 4);
        let origin = centre(&water);
        assert_eq!(
            origin.y, water.surface_y,
            "the sheet is at the wrong height"
        );
        for [x, y, z] in positions {
            // Local to the sheet's own origin, which is what makes it sort
            // against nearby transparent geometry rather than against the map
            // origin.
            assert_eq!(*y, 0.0, "the sheet is not flat");
            // Rebuilt world position, within rounding of the box's edge.
            let (world_x, world_z) = (x + origin.x, z + origin.z);
            assert!(world_x >= water.min_x - 1e-3 && world_x <= water.max_x + 1e-3);
            assert!(world_z >= water.min_z - 1e-3 && world_z <= water.max_z + 1e-3);
        }
        // Both triangles, so the quad is not a single visible half.
        assert_eq!(mesh.indices().map(|i| i.len()), Some(6));
    }

    #[test]
    fn uvs_tile_by_world_size_rather_than_by_box() {
        // The wave size must not stretch with the body of water: a box twice
        // as wide gets twice as many repeats, not larger waves.
        let narrow = quad(&WaterBox {
            max_x: -71.29 + 20.48,
            ..box_at(0.0)
        });
        let wide = quad(&WaterBox {
            max_x: -71.29 + 40.96,
            ..box_at(0.0)
        });
        let span = |mesh: &Mesh| {
            let uvs = uvs_of(mesh);
            uvs.iter().map(|uv| uv[0]).fold(f32::MIN, f32::max)
                - uvs.iter().map(|uv| uv[0]).fold(f32::MAX, f32::min)
        };
        assert!((span(&narrow) - 1.0).abs() < 1e-4, "{}", span(&narrow));
        assert!((span(&wide) - 2.0).abs() < 1e-4, "{}", span(&wide));
    }

    /// The rule that keeps the sea from being found twice, and from being
    /// missed entirely at LOD1.
    #[test]
    fn the_sea_is_the_node_and_not_the_primitive_hanging_off_it() {
        assert!(is_the_sea("ocean"));
        assert!(is_the_sea("ocean_lod1"));
        assert!(!is_the_sea("ocean.PlanetOcean"));
        assert!(!is_the_sea("tile_0_2_1_1"));
    }

    #[test]
    fn the_two_bodies_drift_apart() {
        // One shared direction would read as a single sheet under the castle.
        assert!(DRIFT_DIRECTIONS[0].dot(DRIFT_DIRECTIONS[1]).abs() < 0.01);
        for direction in DRIFT_DIRECTIONS {
            assert!((direction.length() - 1.0).abs() < 1e-3, "{direction:?}");
        }
    }

    #[test]
    fn waterfall_matches_the_original_movtex_strip() {
        let (mesh, origin) = waterfall();
        let Some(bevy::render::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the waterfall has no positions");
        };
        assert_eq!(positions.len(), 15);
        assert_eq!(mesh.indices().map(|indices| indices.len()), Some(51));
        assert_eq!(uvs_of(&mesh).len(), 15);
        let first = Vec3::from_array(positions[0]) + origin;
        assert!((first - Vec3::new(-44.69, -8.0, -64.13)).length() < 1e-4);
        assert!((WATERFALL_UV_SPEED - 2.0507813).abs() < 1e-6);
    }

    #[test]
    fn the_castle_moat_has_water_to_draw() {
        let (level, _) = crate::level::load();
        assert!(!level.water_boxes.is_empty(), "nothing to draw");
        for water in &level.water_boxes {
            assert!(water.max_x > water.min_x && water.max_z > water.min_z);
        }
    }

    /// A sheet buried in the lakebed is drawn and still invisible, which is
    /// exactly what a units or axis mistake in the conversion would produce.
    /// So check the surface against the level it sits in: a good part of each
    /// box must have its floor *below* the surface, which is the part where
    /// water is what you see.
    ///
    /// Not all of it, and that is not a defect. SM64's water boxes are plain
    /// rectangles laid over a bay of the map, so each one also covers dry
    /// ground that rises through the sheet and open space off the edge with no
    /// floor under it at all. The moat is the part that reads as water.
    #[test]
    fn each_body_of_water_is_exposed_over_a_real_stretch_of_lakebed() {
        let (level, _) = crate::level::load();
        for (index, water) in level.water_boxes.iter().enumerate() {
            let mut submerged = 0;
            let mut probes = 0;
            for gz in 0..40 {
                for gx in 0..40 {
                    let x = water.min_x + (water.max_x - water.min_x) * (gx as f32 / 39.0);
                    let z = water.min_z + (water.max_z - water.min_z) * (gz as f32 / 39.0);
                    probes += 1;
                    if level
                        .floor_height(Vec3::new(x, water.surface_y, z))
                        .is_some_and(|bed| bed < water.surface_y)
                    {
                        submerged += 1;
                    }
                }
            }
            let share = submerged as f32 / probes as f32;
            assert!(
                share > 0.15,
                "water box {index} covers {:.0}% submerged ground; the sheet would be \
                 buried or floating over nothing",
                share * 100.0
            );
        }
    }
}
