use bevy::prelude::*;

#[derive(Resource)]
pub struct LevelData {
    pub water_boxes: Vec<WaterBox>,
    triangles: Vec<CollisionTriangle>,
    index: Index,
}

/// How a level's collision is filed, which is the same question as which way
/// up runs across it.
///
/// A flat level is filed by `(x, z)`, because a column of world over one point
/// of the ground holds a handful of triangles and every query is "what is
/// under me". A planet cannot be: project one onto `(x, z)` and the far side
/// lands on top of the near side, so every column holds two hemispheres and
/// the highest surface in it is on the wrong one. Filed by the cube-sphere
/// face a direction points through, a cell is a patch of *surface* again and
/// the query is the same shape it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Up is `+Y` everywhere.
    Flat,
    /// Up is away from `centre`. `radius` is the sea-level radius, which is
    /// what the void test below the terrain is measured against rather than
    /// anything the collision itself needs.
    Planet { centre: Vec3, radius: f32 },
}

/// Where one copy of a planet stands: the turn it has made about its own
/// centre, and where that centre has got to relative to the authored one.
///
/// The mapping every query goes through on a copied or turning world. World
/// space and the filed geometry's space are related by "rotate about the
/// authored centre, then shift to where the copy stands", and the two
/// directions of that are [`Self::to_local`] and [`Self::to_world`]. The
/// authored centre is carried inside the frame so the arithmetic reads at the
/// call site as the two verbs and not as six terms.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlanetFrame {
    /// The centre the geometry was filed about.
    centre: Vec3,
    /// The copy's centre minus the authored one. Zero for the geometry as
    /// filed.
    offset: Vec3,
    /// The copy's spin about its own centre.
    rotation: Quat,
}

impl PlanetFrame {
    /// The geometry exactly as filed: no shift, no turn. What every query on
    /// a flat level and a lone stationary planet goes through, at the cost of
    /// one identity-quaternion multiply.
    const IDENTITY: Self = Self {
        centre: Vec3::ZERO,
        offset: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    /// A world-space point, asked where it is in the filed geometry.
    fn to_local(&self, point: Vec3) -> Vec3 {
        self.centre + self.rotation.inverse() * (point - self.centre - self.offset)
    }

    /// A filed-geometry point, answered where it stands in the world.
    fn to_world(&self, point: Vec3) -> Vec3 {
        self.centre + self.offset + self.rotation * (point - self.centre)
    }

    /// Directions -- ups, normals, pushes -- turn but do not shift.
    fn direction_to_local(&self, direction: Vec3) -> Vec3 {
        self.rotation.inverse() * direction
    }

    fn direction_to_world(&self, direction: Vec3) -> Vec3 {
        self.rotation * direction
    }
}

/// One round world's worth of filed geometry: its own cells, its own middle,
/// its own sea, and the places copies of it stand.
///
/// The first world in the list is *the* planet -- the terrain glb both
/// orbiting bodies are copies of, and the one [`LevelData::shape`] speaks
/// for. Anything after it is a fixture with different geometry standing
/// somewhere else in the same level: the `test_world` diagnostic bodies, each
/// filed by [`LevelData::add_world`].
struct PlanetWorld {
    centre: Vec3,
    radius: f32,
    /// Distance from `centre` out to the water's surface, or `None` on a
    /// dry planet. A planet ocean is one sphere and not a list of boxes:
    /// every basin on the world is under the same surface, so "how deep am
    /// I" is one subtraction wherever it is asked.
    sea: Option<f32>,
    /// Whether the inside of it is somewhere a body cannot legitimately be.
    /// True for a solid ball, whose core the out-of-bounds respawn guards;
    /// false for a fixture with honest space inside its bounding sphere --
    /// the hole of the torus, the air under the platform.
    cored: bool,
    /// One list of triangles per face cell, `face * FACE_GRID * FACE_GRID`
    /// plus the cell within the face -- indices into the level's one shared
    /// triangle list. No floor/wall split, because on a planet that split
    /// depends on where the triangle is and cannot be decided when it is
    /// filed.
    cells: Vec<Vec<u32>>,
    /// Where copies of this world stand and how far each has turned.
    ///
    /// A second planet is not a second set of 786,432 triangles: it is the
    /// same geometry standing somewhere else, so every query maps its
    /// question into the nearest copy's frame, asks the one index that
    /// exists, and maps the answer back. What that costs is the assumption
    /// that no query spans two worlds at once -- and none does: the bodies
    /// sit hundreds of metres apart, and every probe in the game reaches
    /// metres.
    ///
    /// A frame and not just an offset, because the solar system's planets
    /// *spin*: [`crate::orbit::advance`] re-places the first world's list
    /// every tick, and the rotation is what lets one filed set of triangles
    /// be a turning world -- the query turns instead of the geometry.
    instances: Vec<PlanetFrame>,
}

/// The spatial index, one variant per [`Shape`].
enum Index {
    Flat {
        cells: Vec<CollisionCell>,
        /// World position of the low corner of cell `(0, 0)`.
        min: Vec2,
        /// Metres along each side of one cell.
        cell: Vec2,
    },
    Planet {
        /// The planet first, then any fixtures. Never empty.
        worlds: Vec<PlanetWorld>,
    },
}

/// Cells across the collision grid, in each of the two horizontal axes.
///
/// 64 puts the castle's cells at about 2.5 m on a side. It was 16 -- cells of
/// ten metres, holding an average of nine floor triangles each and up to
/// forty-one -- which was fine when the only things asking were the player and a
/// handful of slimes, and is not when a field of two thousand is asking several
/// times each per tick.
///
/// Measured against the real castle rather than guessed: 16 -> 64 takes the
/// average floor list from 9.1 triangles to 3.8 and the worst cell from 41 to
/// 28, for 17,569 index entries in place of 2,929 -- some seventy kilobytes,
/// which is nothing. 128 was also measured and is not worth it: it only reaches
/// 3.1 average, because this collision mesh is 879 large triangles and past a
/// point the finer cells simply hold the same big ones over again.
const GRID_WIDTH: usize = 64;
const WALL_GRID_MARGIN: f32 = 1.0;

/// The most face cells either way [`LevelData::faces_near`] will sweep on a
/// planet.
///
/// A cap on the overlay rather than a property of the world: thirteen cells
/// across is some sixty metres of terrain, which is already more wireframe
/// than can be read, and the arc a given `reach` works out to grows without
/// limit as a planet shrinks.
const DEBUG_FACE_STEPS: isize = 6;

/// The least a triangle may lean off vertical and still be filed as floor.
///
/// Barely a filter at all, and deliberately: the list it guards answers "what
/// stops a falling body here", and the only triangles that stop nothing are
/// the ones exactly on edge. Named because [`LevelData::face`] has to sort a
/// triangle into the same three boxes the filing does, and a second copy of
/// the number is a second answer waiting to happen.
const FLOOR_FILE_LEAN: f32 = 0.01;

/// How far off horizontal a triangle may lean and still be ground rather than
/// wall. Anything at or below this is something you are pushed out of instead
/// of something you stand on, which is the split [`LevelData::new`] has always
/// sorted the flat collision by; naming it lets [`LevelData::ground_at`] ask
/// the same question rather than a second, differently drawn one.
///
/// The `_Y` in the name is now half true. A flat level measures the lean
/// against `+Y` once, when the grid is built, because `+Y` is the same
/// everywhere on it. A planet measures the same number against the local up in
/// [`LevelData::walls_near`], because the same triangle is a floor on one side
/// of a world and a ceiling on the other. Same constant, different up.
pub const GROUND_NORMAL_Y: f32 = 0.7;

/// The steepest ground there is, as rise over run.
///
/// [`GROUND_NORMAL_Y`] said the same thing as a normal; this is it as a slope,
/// which is the form anything walking over the ground wants it in. A hair over
/// the exact conversion, so that ground the collision grid is happy to call
/// ground is never refused by something measuring it this way.
pub const WALKABLE_SLOPE: f32 = 1.1;

/// How far above where it is asked [`LevelData::ground_at`] will still hand
/// back a surface.
///
/// A body's feet are never exactly on the floor -- it is a tick behind, or on
/// the near side of a kerb -- so the query reaches up a little rather than
/// finding nothing. Named because [`LevelData::climbable`] has to reach up by a
/// controlled amount instead, and a caller cannot do that without knowing what
/// the query already adds.
pub const GROUND_REACH: f32 = 0.5;

/// How far apart [`LevelData::climbable`] takes its samples, in metres.
///
/// Short enough to catch the thing it is looking for: a lip half a metre tall
/// reads as a slope of one in one over half a metre and passes, and as two in
/// one over a quarter of a metre and does not. Long enough that a stride's
/// worth of walking is a handful of floor queries rather than a dozen.
const CLIMB_SAMPLE: f32 = 0.3;

/// Cells along one side of each of a planet's six cube-sphere faces.
///
/// 96 puts a cell at about five metres on the 300 m planet `planet_gen`
/// writes, which is three of its terrain triangles across -- the same ratio
/// [`GRID_WIDTH`] lands on for the castle, and for the same reason: fine enough
/// that a cell is a handful of triangles, coarse enough that filing a triangle
/// into it is one cell rather than nine.
const FACE_GRID: usize = 96;

/// How much arc one face cell spans, in radians. A face is a quarter turn
/// across, whatever the planet's radius is, so this is a property of the grid
/// and not of the planet.
const FACE_CELL_ANGLE: f32 = std::f32::consts::FRAC_PI_2 / FACE_GRID as f32;

/// How far below a point a planet's ground query looks before giving up.
///
/// It has to be bounded, and on a sphere that is not fussiness: a ray straight
/// down from the surface leaves through the core and hits the far side, so an
/// unbounded search finds the ground under someone's feet on the *other*
/// hemisphere. Generous against the +-40 m of terrain relief, and far short of
/// the 600 m to the antipode.
const PLANET_REACH: f32 = 200.0;

/// How far above a point that same query starts, so that standing exactly on
/// the ground still finds it. The flat grid's equivalent is the `+ 0.5` in
/// [`LevelData::highest_below`].
const GROUND_SKIN: f32 = 0.5;

/// How many times [`LevelData::resolve_walls`] re-asks the level where the
/// walls are.
///
/// One pass answers a flat wall exactly. More are needed only where leaving
/// one wall walks a body into another it was not touching when the pass
/// started -- the inside of a corner, a doorway's jamb -- and each pass is a
/// fresh grid query, so this is the price of the worst case rather than of the
/// common one. Four is comfortably past what the castle's tightest corners
/// need; the loop stops the moment a pass finds nothing to do, which for a
/// body walking in the open is the first one.
const WALL_RESOLVE_PASSES: usize = 4;

/// How far past merely touching a wall a body is put, in metres.
///
/// Landing exactly on the surface leaves the next frame's test to decide
/// whether a body a rounding error deep is inside or outside, which is how a
/// body sat against a wall ends up jittering. A millimetre is under the
/// tolerance of anything that looks at these positions and over the error in
/// reaching them.
const WALL_SKIN: f32 = 0.001;

#[derive(Clone, Copy)]
struct CollisionTriangle {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    normal: Vec3,
    min: Vec2,
    max: Vec2,
}

#[derive(Clone, Default)]
struct CollisionCell {
    floors: Vec<usize>,
    walls: Vec<usize>,
    all: Vec<usize>,
}

/// One axis-aligned body of water. Water is not part of the level mesh: the
/// collision data carries these boxes, gameplay swims inside them and the
/// renderer draws a sheet across the top of each.
#[derive(Clone, Copy, Debug)]
pub struct WaterBox {
    pub min_x: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_z: f32,
    pub surface_y: f32,
}

pub struct RenderLevel;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn count(&mut self) -> usize {
        let value = u32::from_le_bytes(self.bytes[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        value as usize
    }
    fn floats(&mut self, width: usize) -> Vec<Vec<f32>> {
        let count = self.count();
        (0..count)
            .map(|_| {
                (0..width)
                    .map(|_| {
                        let value = f32::from_le_bytes(
                            self.bytes[self.at..self.at + 4].try_into().unwrap(),
                        );
                        self.at += 4;
                        value
                    })
                    .collect()
            })
            .collect()
    }
    fn indices(&mut self, width: usize) -> Vec<Vec<u32>> {
        let count = self.count();
        (0..count)
            .map(|_| {
                (0..width)
                    .map(|_| {
                        let value = u32::from_le_bytes(
                            self.bytes[self.at..self.at + 4].try_into().unwrap(),
                        );
                        self.at += 4;
                        value
                    })
                    .collect()
            })
            .collect()
    }
}

pub fn load() -> (LevelData, RenderLevel) {
    let bytes = include_bytes!("../assets/bevy/castle.bin");
    assert_eq!(&bytes[..4], b"SBW1");
    let mut r = Reader { bytes, at: 4 };
    let _positions = r.floats(3);
    let _normals = r.floats(3);
    let _uvs = r.floats(2);
    let _colors = r.floats(4);
    let _triangles = r.indices(3);
    let collision_vertices = r
        .floats(3)
        .into_iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect();
    let collision_triangles = r
        .indices(3)
        .into_iter()
        .map(|v| [v[0], v[1], v[2]])
        .collect();
    // These triangles are the ones the castle is *drawn* with, exported from
    // `assets/bevy/castle_grounds.blend` by `tools/convert_level.py`. They
    // used to be the decomp's separate 879-triangle collision hull, so the
    // level shipped one mesh you could see and another you walked on, with
    // nothing anywhere comparing them -- and `collide_debug 2` exists partly
    // because that difference was invisible. Moving a wall in Blender now
    // moves what stops you, and the hull's invisible walls and death planes
    // are gone with it.
    //
    // The water is not in here either. It is two planes in
    // `assets/levels/castle.blend` that somebody can drag -- see
    // [`crate::furniture`].
    (
        LevelData::new(
            collision_vertices,
            collision_triangles,
            crate::furniture::castle().water_boxes(),
        ),
        RenderLevel,
    )
}

impl LevelData {
    /// Builds collision from raw geometry. Crate-visible so movement tests can
    /// stand a small room up without the castle in the way.
    pub(crate) fn new(
        vertices: Vec<Vec3>,
        indices: Vec<[u32; 3]>,
        water_boxes: Vec<WaterBox>,
    ) -> Self {
        let triangles: Vec<_> = indices
            .iter()
            .map(|tri| {
                let a = vertices[tri[0] as usize];
                let b = vertices[tri[1] as usize];
                let c = vertices[tri[2] as usize];
                CollisionTriangle {
                    a,
                    b,
                    c,
                    normal: (b - a).cross(c - a).normalize_or_zero(),
                    min: Vec2::new(a.x.min(b.x).min(c.x), a.z.min(b.z).min(c.z)),
                    max: Vec2::new(a.x.max(b.x).max(c.x), a.z.max(b.z).max(c.z)),
                }
            })
            .collect();
        let (grid_min, grid_max) = triangles.iter().fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(min, max), tri| (min.min(tri.min), max.max(tri.max)),
        );
        let grid_min = if grid_min.is_finite() {
            grid_min
        } else {
            Vec2::ZERO
        };
        let grid_max = if grid_max.is_finite() {
            grid_max
        } else {
            Vec2::ONE
        };
        let grid_cell = ((grid_max - grid_min) / GRID_WIDTH as f32).max(Vec2::splat(0.001));
        let mut cells = vec![CollisionCell::default(); GRID_WIDTH * GRID_WIDTH];
        let coords = |point: Vec2| -> (usize, usize) {
            let cell = ((point - grid_min) / grid_cell).floor();
            (
                (cell.x as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
                (cell.y as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
            )
        };
        for (index, tri) in triangles.iter().enumerate() {
            let all_min = coords(tri.min);
            let all_max = coords(tri.max);
            for z in all_min.1..=all_max.1 {
                for x in all_min.0..=all_max.0 {
                    cells[z * GRID_WIDTH + x].all.push(index);
                    // Filed by lean alone, with the sign of the normal
                    // thrown away: the converted castle mesh is not
                    // consistently wound, so a floor's normal is as likely to
                    // be its underside as its top, and the query above picks
                    // by height rather than by facing for the same reason.
                    if tri.normal.y.abs() > FLOOR_FILE_LEAN {
                        cells[z * GRID_WIDTH + x].floors.push(index);
                    }
                }
            }
            if tri.normal.y.abs() <= GROUND_NORMAL_Y {
                let margin = Vec2::splat(WALL_GRID_MARGIN);
                let wall_min = coords(tri.min - margin);
                let wall_max = coords(tri.max + margin);
                for z in wall_min.1..=wall_max.1 {
                    for x in wall_min.0..=wall_max.0 {
                        cells[z * GRID_WIDTH + x].walls.push(index);
                    }
                }
            }
        }
        Self {
            water_boxes,
            triangles,
            index: Index::Flat {
                cells,
                min: grid_min,
                cell: grid_cell,
            },
        }
    }

    /// Builds collision for a planet: the same triangles, filed by the
    /// cube-sphere face cells they cover.
    ///
    /// Two things are dropped on the way in, and both of them are the sort of
    /// thing a mesh acquires without anyone meaning it to.
    ///
    /// Triangles naming a vertex that does not exist, because collision read
    /// back out of a glTF is only as sound as the glTF, and an index past the
    /// end of the array is a panic in the middle of a level load rather than a
    /// tile that looks slightly wrong.
    ///
    /// Triangles with no area to speak of, because they are worse than
    /// useless: [`degenerate`] has the whole story, and the short version is
    /// that one of them is an invisible floor in the middle of the world.
    pub fn planet(
        vertices: &[Vec3],
        indices: &[[u32; 3]],
        centre: Vec3,
        radius: f32,
        sea: Option<f32>,
    ) -> Self {
        let triangles: Vec<_> = indices
            .iter()
            .filter(|tri| tri.iter().all(|&i| (i as usize) < vertices.len()))
            .map(|tri| {
                let a = vertices[tri[0] as usize];
                let b = vertices[tri[1] as usize];
                let c = vertices[tri[2] as usize];
                CollisionTriangle {
                    a,
                    b,
                    c,
                    normal: (b - a).cross(c - a).normalize_or_zero(),
                    min: Vec2::new(a.x.min(b.x).min(c.x), a.z.min(b.z).min(c.z)),
                    max: Vec2::new(a.x.max(b.x).max(c.x), a.z.max(b.z).max(c.z)),
                }
            })
            .filter(|tri| !degenerate(tri))
            .collect();
        let mut cells = vec![Vec::new(); 6 * FACE_GRID * FACE_GRID];
        let mut filed = Vec::new();
        for (index, tri) in triangles.iter().enumerate() {
            file_triangle(tri, centre, index as u32, &mut filed, &mut cells);
        }
        Self {
            water_boxes: Vec::new(),
            triangles,
            index: Index::Planet {
                worlds: vec![PlanetWorld {
                    centre,
                    radius,
                    sea,
                    cored: true,
                    cells,
                    instances: vec![PlanetFrame {
                        centre,
                        ..PlanetFrame::IDENTITY
                    }],
                }],
            },
        }
    }

    /// Files one more body's geometry into a planet level, standing at
    /// `stands_at` and never moving: how the `test_world` fixtures join the
    /// solar system without becoming a second collision resource. The
    /// vertices are the glb's own -- centred wherever it was authored, with
    /// `centre` their measured middle -- and the one instance maps them to
    /// where the body stands. `cored` says whether the inside of it is
    /// out-of-bounds the way a solid planet's core is; a torus's hole and a
    /// platform's underside are honest places to fly.
    ///
    /// A no-op on a flat level, which has no frame for another world to
    /// stand in.
    pub fn add_world(
        &mut self,
        vertices: &[Vec3],
        indices: &[[u32; 3]],
        centre: Vec3,
        radius: f32,
        cored: bool,
        stands_at: Vec3,
    ) {
        let Index::Planet { worlds } = &mut self.index else {
            return;
        };
        let base = self.triangles.len() as u32;
        let fresh: Vec<_> = indices
            .iter()
            .filter(|tri| tri.iter().all(|&i| (i as usize) < vertices.len()))
            .map(|tri| {
                let a = vertices[tri[0] as usize];
                let b = vertices[tri[1] as usize];
                let c = vertices[tri[2] as usize];
                CollisionTriangle {
                    a,
                    b,
                    c,
                    normal: (b - a).cross(c - a).normalize_or_zero(),
                    min: Vec2::new(a.x.min(b.x).min(c.x), a.z.min(b.z).min(c.z)),
                    max: Vec2::new(a.x.max(b.x).max(c.x), a.z.max(b.z).max(c.z)),
                }
            })
            .filter(|tri| !degenerate(tri))
            .collect();
        let mut cells = vec![Vec::new(); 6 * FACE_GRID * FACE_GRID];
        let mut filed = Vec::new();
        for (index, tri) in fresh.iter().enumerate() {
            file_triangle(tri, centre, base + index as u32, &mut filed, &mut cells);
        }
        self.triangles.extend(fresh);
        worlds.push(PlanetWorld {
            centre,
            radius,
            sea: None,
            cored,
            cells,
            instances: vec![PlanetFrame {
                centre,
                offset: stands_at,
                rotation: Quat::IDENTITY,
            }],
        });
    }

    /// How many round worlds are filed: the planet plus its fixtures, or zero
    /// on a flat level. What a test asks to know the fixtures arrived.
    pub fn world_count(&self) -> usize {
        match &self.index {
            Index::Planet { worlds } => worlds.len(),
            Index::Flat { .. } => 0,
        }
    }

    /// Stands a second copy of this planet at `offset` from the first.
    ///
    /// A builder on the finished collision rather than a parameter threaded
    /// through [`Self::planet`], because every caller but one wants one planet
    /// and a parameter they must all pass is a question they should not all
    /// have to answer.
    ///
    /// Re-places every copy of the planet at once: one `(centre, spin)` per
    /// world, in body order. Called every tick by [`crate::orbit::advance`],
    /// which is the only way a filed set of triangles gets to orbit a sun.
    ///
    /// Replaces rather than edits, so the caller's list *is* the truth and a
    /// body it stopped naming is a body that is gone.
    pub fn place_planets(&mut self, placed: &[(Vec3, Quat)]) {
        if let Index::Planet { worlds } = &mut self.index {
            let Some(planet) = worlds.first_mut() else {
                return;
            };
            let centre = planet.centre;
            planet.instances.clear();
            planet
                .instances
                .extend(placed.iter().map(|&(stands_at, rotation)| PlanetFrame {
                    centre,
                    offset: stands_at - centre,
                    rotation,
                }));
        }
    }

    /// The world copy nearest to `reference`, as the world itself and the
    /// frame it stands in: the pair a query asked around that point is
    /// answered by. `None` on a flat level.
    fn planet_at(&self, reference: Vec3) -> Option<(&PlanetWorld, PlanetFrame)> {
        let Index::Planet { worlds } = &self.index else {
            return None;
        };
        worlds
            .iter()
            .flat_map(|world| world.instances.iter().map(move |&frame| (world, frame)))
            .min_by(|(_, a), (_, b)| {
                (reference - (a.centre + a.offset))
                    .length_squared()
                    .total_cmp(&(reference - (b.centre + b.offset)).length_squared())
            })
    }

    /// What shape of world this is, and where its middle is if it has one.
    /// A level holding fixtures beside its planet answers for the planet:
    /// the first world is the one the level is *about*, and everything that
    /// asks here -- the visual unbend, the autopilot's approach radius, the
    /// orbital clockwork's filed centre -- means that one.
    pub fn shape(&self) -> Shape {
        match &self.index {
            Index::Flat { .. } => Shape::Flat,
            Index::Planet { worlds } => match worlds.first() {
                Some(world) => Shape::Planet {
                    centre: world.centre,
                    radius: world.radius,
                },
                None => Shape::Flat,
            },
        }
    }

    /// Whether `point` has left the world altogether and wants putting back.
    ///
    /// Below the castle's lowest floor on a flat level; inside the core on a
    /// planet, which is a hundred metres under the deepest terrain the
    /// generator writes and so cannot be reached by walking into a valley.
    pub fn out_of_bounds(&self, point: Vec3) -> bool {
        match &self.index {
            Index::Flat { .. } => point.y < -20.0,
            Index::Planet { .. } => match self.planet_at(point) {
                // A hollow fixture -- the torus, the platform -- has honest
                // space inside its bounding sphere, so nowhere near it is out
                // of the world.
                Some((world, frame)) => {
                    world.cored
                        && (point - (world.centre + frame.offset)).length()
                            < world.radius - PLANET_REACH * 0.5
                }
                None => false,
            },
        }
    }

    fn flat(&self) -> Option<(&[CollisionCell], Vec2, Vec2)> {
        match &self.index {
            Index::Flat { cells, min, cell } => Some((cells, *min, *cell)),
            Index::Planet { .. } => None,
        }
    }

    fn cell_coords(&self, point: Vec2) -> (usize, usize) {
        let Some((_, min, size)) = self.flat() else {
            return (0, 0);
        };
        let cell = ((point - min) / size).floor();
        (
            (cell.x as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
            (cell.y as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
        )
    }

    /// The flat grid's cell at `(x, z)`, or an empty one on a planet -- which
    /// is what makes every flat-only query answer "nothing here" there rather
    /// than answering wrongly. See [`Self::ground_below`] for the query the
    /// planet does answer.
    fn cell(&self, x: f32, z: f32) -> &CollisionCell {
        static NOTHING: CollisionCell = CollisionCell {
            floors: Vec::new(),
            walls: Vec::new(),
            all: Vec::new(),
        };
        let Some((cells, _, _)) = self.flat() else {
            return &NOTHING;
        };
        let (x, z) = self.cell_coords(Vec2::new(x, z));
        &cells[z * GRID_WIDTH + x]
    }

    /// The face cells a query at `point` could find a triangle in, on the
    /// world whose filed `centre` the point has already been mapped towards.
    ///
    /// The three-by-three patch around the point's own cell, clamped to its
    /// face, plus the cells one cell away along two tangent directions. Those
    /// last two are the seam: clamping stops at the edge of a face, and a
    /// player standing on one is a metre from triangles filed on the next face
    /// along, which no amount of arithmetic *within* a face will ever reach.
    /// Perturbing the direction and re-filing it lands on the neighbour
    /// whatever the neighbour happens to be, which is the same trick the
    /// generator's own neighbour table exists to avoid needing.
    fn near_cells(centre: Vec3, point: Vec3, out: &mut Vec<usize>) {
        out.clear();
        let radial = (point - centre).normalize_or(Vec3::Y);
        let (face, u, v) = face_uv(radial);
        let (column, row) = (cell_index(u) as isize, cell_index(v) as isize);
        let last = FACE_GRID as isize - 1;
        for down in -1..=1 {
            for across in -1..=1 {
                let x = (column + across).clamp(0, last) as usize;
                let y = (row + down).clamp(0, last) as usize;
                let cell = (face * FACE_GRID + y) * FACE_GRID + x;
                if !out.contains(&cell) {
                    out.push(cell);
                }
            }
        }
        let (along, across) = radial.any_orthonormal_pair();
        for offset in [along, -along, across, -across] {
            let cell = face_cell(radial + offset * FACE_CELL_ANGLE);
            if !out.contains(&cell) {
                out.push(cell);
            }
        }
    }

    /// The horizontal extent of the collision, as `(min, max)`.
    ///
    /// Read by [`crate::flow`] so the crowd's navigation grid covers exactly
    /// the ground that exists, rather than a rectangle picked by hand that
    /// would have to be corrected every time the level changed. A planet hands
    /// back its bounding square: nothing navigates one yet, and a zero-sized
    /// box would make the field's cell size meaningless rather than merely
    /// empty.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        match self.shape() {
            Shape::Flat => match self.index {
                Index::Flat { min, cell, .. } => (min, min + cell * GRID_WIDTH as f32),
                Index::Planet { .. } => (Vec2::ZERO, Vec2::ZERO),
            },
            Shape::Planet { centre, radius } => {
                let middle = Vec2::new(centre.x, centre.z);
                let reach = Vec2::splat(radius + PLANET_REACH * 0.25);
                (middle - reach, middle + reach)
            }
        }
    }

    /// How far the water's surface is from the middle of a planet, or `None`
    /// on a world with no sea. The sea that is drawn and the sea that is swum
    /// in are the same sphere, so the renderer asks for its radius here rather
    /// than measuring the mesh a second time and getting a second answer.
    pub fn sea_radius(&self) -> Option<f32> {
        match &self.index {
            Index::Planet { worlds } => worlds.first().and_then(|world| world.sea),
            Index::Flat { .. } => None,
        }
    }

    /// How far `point` is below the surface of the water it is in, or `None`
    /// where there is no water over that spot at all.
    ///
    /// A depth rather than a height, and that is what makes one question serve
    /// both worlds. On a flat level the surface is a `y` and being under it
    /// means a smaller `y`; on a planet the surface is a radius and being
    /// under it means a smaller radius. Measured as a depth, both are the same
    /// number: how far the point would have to rise, along its own up, to
    /// break the surface. Negative in open air, so a caller that wants "is
    /// this wet" asks for a positive depth rather than knowing which world it
    /// is standing on.
    pub fn water_depth(&self, point: Vec3) -> Option<f32> {
        if let Some((world, frame)) = self.planet_at(point) {
            let core = world.centre + frame.offset;
            return world.sea.map(|surface| surface - (point - core).length());
        }
        self.water_boxes
            .iter()
            .find(|water| {
                point.x >= water.min_x
                    && point.x <= water.max_x
                    && point.z >= water.min_z
                    && point.z <= water.max_z
            })
            .map(|water| water.surface_y - point.y)
    }
    pub fn floor_height(&self, point: Vec3) -> Option<f32> {
        self.highest_below(point, 0.0).map(|(height, _)| height)
    }

    /// The highest piece of *ground* below `point`, as its height and the
    /// direction it slopes.
    ///
    /// Not the same question as [`Self::floor_height`], and the difference is
    /// the whole reason this exists. That one answers "what stops a falling
    /// body here", and to do it it considers every triangle in the cell that is
    /// not exactly vertical -- the collision grid's floor list is filtered at
    /// `0.01`, which is to say barely filtered at all. Over the castle, seven
    /// per cent of points get their answer from a triangle leaning more than
    /// sixty degrees, and near a wall the winning surface is often the wall.
    ///
    /// That is harmless for physics, which only reads the height. It is not
    /// harmless for [`crate::shadow`], which also turns a disc to lie along the
    /// surface: turned onto a wall's normal the disc is edge-on to the ground,
    /// which is to say invisible, and it flips in and out as the caster walks
    /// across the point where the wall stops winning. So a shadow asks for
    /// ground and gets ground.
    pub fn ground_at(&self, point: Vec3) -> Option<(f32, Vec3)> {
        self.highest_below(point, GROUND_NORMAL_Y)
    }

    /// The highest surface at or a little above `point`, out of those leaning
    /// less than `min_normal_y` off horizontal.
    ///
    /// The returned normal is forced to point upwards. The converted castle
    /// mesh does not have consistent winding -- which is why the surface is
    /// selected by height and not by facing in the first place -- so a
    /// triangle's own normal is as likely to be the underside as the top, and
    /// what a caller wants is which way the ground slopes rather than which way
    /// the polygon happened to be wound. It is also why the lean is measured on
    /// the absolute value.
    fn highest_below(&self, point: Vec3, min_normal_y: f32) -> Option<(f32, Vec3)> {
        let mut best: Option<(f32, Vec3)> = None;
        for &index in &self.cell(point.x, point.z).floors {
            let tri = self.triangles[index];
            if tri.normal.y.abs() <= min_normal_y {
                continue;
            }
            if let Some(y) = surface_y(tri, point.x, point.z) {
                if y <= point.y + GROUND_REACH && best.is_none_or(|(height, _)| y > height) {
                    let up = if tri.normal.y < 0.0 {
                        -tri.normal
                    } else {
                        tri.normal
                    };
                    best = Some((y, up));
                }
            }
        }
        best
    }

    /// Whether a walker could follow the ground from one point to the other
    /// without meeting a step it cannot take.
    ///
    /// **The question every other test of this asks about the two ends, asked
    /// about what is in between.** A body a stride from the foot of a bank sees
    /// ground eighty centimetres up and a stride away -- half a metre of rise
    /// per metre of run, gentler than anything walkable ought to be refused for
    /// -- and walks straight into a knee-high lip at the bottom of it that
    /// [`Self::resolve_walls`] will not let it past. It then slides along that
    /// lip for the rest of the session, with a perfectly good route in hand and
    /// nothing anywhere reporting a problem. Averages hide steps; this marches.
    ///
    /// Sampled every [`CLIMB_SAMPLE`] along the way, each sample asking for the
    /// highest ground within a climbable step of where the walk has got to. A
    /// short steep face shows up as no ground being found at all -- the face
    /// itself is too steep to *be* ground, and what is on top of it is out of
    /// reach -- which is the answer.
    ///
    /// Falling is not climbing: ground far below is fine and is somebody else's
    /// question. See [`crate::squad::steer`], which prices a drop separately.
    pub fn climbable(&self, from: Vec3, to: Vec3) -> bool {
        let span = Vec2::new(to.x - from.x, to.z - from.z);
        let length = span.length();
        if length < 1e-4 {
            return true;
        }
        let samples = (length / CLIMB_SAMPLE).ceil().max(1.0);
        // The most the ground may rise between one sample and the next.
        let rise = length / samples * WALKABLE_SLOPE;
        let mut height = from.y;
        for step in 1..=samples as usize {
            let at = Vec2::new(from.x, from.z) + span * (step as f32 / samples);
            // Asked from `rise` above where the walk has got to, less what the
            // query reaches up by on its own -- so what comes back is the
            // highest ground that could actually be stepped onto, and nothing
            // higher.
            let ceiling = height + rise - GROUND_REACH;
            let Some((found, _)) = self.ground_at(Vec3::new(at.x, ceiling, at.y)) else {
                return false;
            };
            height = found;
        }
        true
    }

    /// Returns the lowest horizontal surface above `point`, or `None` when
    /// there is open sky over it.
    ///
    /// Deliberately geometric rather than reading the triangle's facing. The
    /// converted castle mesh does not have consistent winding -- which is why
    /// [`Self::floor_height`] selects by height and not by normal -- so a
    /// ceiling here is simply "the nearest near-horizontal surface overhead",
    /// and the same triangle can serve as one actor's floor and another's
    /// ceiling. `clearance` is how far above the point the search starts, so a
    /// floor the actor is standing on is never mistaken for a ceiling.
    pub fn ceiling_height(&self, point: Vec3, clearance: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for &index in &self.cell(point.x, point.z).floors {
            if let Some(y) = surface_y(self.triangles[index], point.x, point.z) {
                if y >= point.y + clearance && best.is_none_or(|old| y < old) {
                    best = Some(y);
                }
            }
        }
        best
    }

    /// The ground under `from`, as the point it is at and the way it slopes.
    ///
    /// The query [`Self::floor_height`] is on a flat level, asked in a way a
    /// planet can answer too: `up` is handed in rather than assumed, the
    /// answer is a position rather than a height, and the search runs along
    /// `up` rather than down a column of `(x, z)`.
    ///
    /// The two shapes reach the same answer by different routes and that is
    /// deliberate. A flat level looks up the column of `(x, z)` the point
    /// stands in, which is one grid cell and a handful of triangles. A planet
    /// casts a ray from [`GROUND_SKIN`] above the point to [`PLANET_REACH`]
    /// below it, because a column is exactly the thing a sphere does not have.
    /// `the_castle_answers_the_general_query_the_way_it_answers_the_old_one`
    /// is the test that keeps the two routes agreeing where both apply.
    pub fn ground_below(&self, from: Vec3, up: Vec3) -> Option<(Vec3, Vec3)> {
        match self.index {
            Index::Flat { .. } => self
                .highest_below(from, 0.0)
                .map(|(height, normal)| (Vec3::new(from.x, height, from.z), normal)),
            Index::Planet { .. } => {
                // The raw first hit, wall-steep or not. This once tried to
                // walk past steep faces so the feet could never settle onto a
                // face the wall resolution was pushing out of -- but a steep
                // face lies nearly along the ray, so the re-cast from just
                // past the hit met the same triangle again, burned its
                // retries, and answered "no ground" over every steep patch:
                // vanishing shadows, flickering grounding, worse than the
                // fight it fixed. The arbitration lives with the caller now:
                // `player::movement` declines to *stand* on a wall-class hit,
                // and every other reader wants the surface as it is.
                let start = from + up * GROUND_SKIN;
                self.surface_hit(start, from - up * PLANET_REACH)
            }
        }
    }

    /// The lowest thing overhead, as a point, or `None` when there is open sky.
    ///
    /// [`Self::ceiling_height`] with the same generalisation
    /// [`Self::ground_below`] applies to the floor. A planet's terrain is a
    /// heightfield wrapped onto a sphere and so has no overhangs today; the
    /// cast is done anyway rather than returning `None`, so that the day one
    /// is authored the player's head stops on it instead of passing through.
    pub fn ceiling_above(&self, from: Vec3, up: Vec3, clearance: f32) -> Option<Vec3> {
        match self.index {
            Index::Flat { .. } => self
                .ceiling_height(from, clearance)
                .map(|height| Vec3::new(from.x, height, from.z)),
            Index::Planet { .. } => {
                let start = from + up * clearance;
                self.surface_hit(start, start + up * PLANET_REACH * 0.05)
                    .map(|(point, _)| point)
            }
        }
    }

    /// Pushes a vertical capsule out of the walls it has ended up inside.
    ///
    /// Collision is deliberately independent of Bevy's renderer so movement
    /// can be exercised in headless tests. The body is a real capsule -- the
    /// segment from `radius` above the feet to `radius` below the head, swept
    /// by `radius` -- and each wall is measured against the whole of that
    /// segment rather than against a few sample spheres strung along it. A
    /// triangle can slip between two sample spheres and leave a body standing
    /// in a wall it plainly touches; a segment cannot be straddled, and it is
    /// no more expensive to test.
    ///
    /// The answer does not depend on the order the collision mesh happens to
    /// be filed in. Every overlap in a pass is measured against the same
    /// starting position and the pushes are then combined deepest first, so
    /// shuffling the triangles moves nobody --
    /// `wall_resolution_does_not_depend_on_triangle_order` is the test that
    /// says so. Applying each push the moment it is found, which is what a
    /// plain loop over the candidate list does, makes where a body ends up a
    /// function of how the level was built.
    pub fn resolve_walls(&self, position: Vec3, up: Vec3, radius: f32, height: f32) -> Vec3 {
        // The whole resolution runs in the nearest copy's frame, against that
        // copy's own world. Chosen once rather than per pass: the passes move
        // a body centimetres, and the copies stand hundreds of metres apart.
        // The up is carried into that frame too -- on a turned world
        // "vertical" is turned with it.
        let placed = self.planet_at(position);
        let frame = placed
            .map(|(_, frame)| frame)
            .unwrap_or(PlanetFrame::IDENTITY);
        let world = placed.map(|(world, _)| world);
        let up = frame.direction_to_local(up);
        let mut result = frame.to_local(position);
        let mut candidates = Vec::new();
        let mut cells = Vec::new();
        let mut contacts: Vec<Contact> = Vec::new();
        // Whether any wall actually did anything. The answer has to be the
        // caller's own `position`, bit for bit, when none did: the round trip
        // through a placed copy's frame loses the last float bit, and on a
        // planet five thousand metres out that bit is half a millimetre --
        // over the movement code's touch threshold. Handed back as if it were
        // a push, its direction is rounding noise flattened into a unit
        // vector, and cancelling velocity along it scrubbed up to two thirds
        // of a flyer's speed in clear air, every tick, everywhere the frame
        // was not the identity. The felt symptom was constant "wall push"
        // jerks on the moving planets and none on the castle or the static
        // test bodies, whose frames round-trip exactly.
        let mut pushed = false;
        for _ in 0..WALL_RESOLVE_PASSES {
            self.walls_near(result, up, world, &mut cells, &mut candidates);
            contacts.clear();
            // The capsule's spine. A body shorter than it is wide -- which no
            // actor here is, but a caller may yet ask for -- degenerates to a
            // sphere rather than to a segment pointing the wrong way.
            let foot = result + up * radius;
            let head = result + up * (height - radius).max(radius);
            for &index in &candidates {
                if let Some(contact) = self.contact(index, foot, head, up, radius) {
                    contacts.push(contact);
                }
            }
            if contacts.is_empty() {
                break;
            }
            // A contact's depth is never zero -- [`WALL_SKIN`] is baked into
            // it -- so a pass that found any is a pass that moved the body.
            pushed = true;
            // Deepest first, with ties broken on the direction itself so that
            // two equally deep walls -- the two faces of a corner, usually --
            // are always taken in the same order however the mesh was built.
            contacts.sort_by(|a, b| {
                b.depth.total_cmp(&a.depth).then_with(|| {
                    a.direction
                        .to_array()
                        .partial_cmp(&b.direction.to_array())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            });
            let mut push = Vec3::ZERO;
            for contact in &contacts {
                // What is left of this wall's demand once the pushes already
                // taken are counted against it. A corner needs both of its
                // faces and gets both; the same wall met twice, as a triangle
                // spanning two grid cells is, needs only the deeper of them.
                let remaining = contact.depth - push.dot(contact.direction);
                if remaining > 0.0 {
                    push += contact.direction * remaining;
                }
            }
            result += push;
        }
        if !pushed {
            return position;
        }
        frame.to_world(result)
    }

    /// What one wall does to the capsule running from `foot` to `head`, or
    /// `None` where the two are clear of each other.
    ///
    /// Its own function rather than the body of the loop above because
    /// [`Self::wall_contacts`] has to give the same answer -- an overlay that
    /// measures the push its own way is an overlay that disagrees with the bug
    /// it was turned on to explain.
    fn contact(
        &self,
        index: u32,
        foot: Vec3,
        head: Vec3,
        up: Vec3,
        radius: f32,
    ) -> Option<Contact> {
        let tri = self.triangles[index as usize];
        let (on_capsule, on_wall) =
            closest_points_segment_triangle(foot, head, tri.a, tri.b, tri.c);
        let offset = on_capsule - on_wall;
        // "Horizontal" is whatever lies flat against the ground here, which on
        // a planet is a different plane at every point. The push stays in that
        // plane because a wall should stop a body, not lift it over itself.
        let climb = offset.dot(up);
        let flat = offset - up * climb;
        let distance = flat.length();
        // How far apart the two have to be *horizontally* to be clear, given
        // how far apart they already are vertically. This is the whole of the
        // difference between a capsule and a column, and leaving it out is
        // what put invisible walls on the bridges: a moat face five metres
        // below the deck is directly under the edge you are walking along, so
        // its horizontal distance is nearly nothing, and a body clear of it by
        // five metres was being shoved a metre and a half sideways by it.
        //
        // Where the contact is level with the body this is the radius and
        // nothing has changed. Where it is a radius or more above the head or
        // below the feet there is no overlap at all and the wall is not a wall
        // to this body.
        let square = radius * radius - climb * climb;
        if square <= 0.0 {
            return None;
        }
        let clearance = square.sqrt();
        if distance >= clearance {
            return None;
        }
        let direction = if distance > 1e-5 {
            flat / distance
        } else {
            // Dead in the wall's plane: the offset says nothing about which
            // side to leave by, so the triangle's own facing decides,
            // flattened against the local ground.
            (tri.normal - up * tri.normal.dot(up)).normalize_or_zero()
        };
        if direction == Vec3::ZERO {
            return None;
        }
        Some(Contact {
            direction,
            depth: clearance - distance + WALL_SKIN,
        })
    }

    /// The triangles near `at` that count as wall rather than floor.
    ///
    /// A flat level decided that once, when the grid was built, because `+Y` is
    /// the same everywhere and so a triangle is a wall or it is not. On a
    /// planet the same triangle is a floor on one side of the world and a
    /// ceiling on the other, so the question is asked here, against the `up`
    /// that holds where the query is.
    fn walls_near(
        &self,
        at: Vec3,
        up: Vec3,
        world: Option<&PlanetWorld>,
        cells: &mut Vec<usize>,
        out: &mut Vec<u32>,
    ) {
        out.clear();
        match world {
            None => out.extend(
                self.cell(at.x, at.z)
                    .walls
                    .iter()
                    .map(|&index| index as u32),
            ),
            Some(world) => {
                Self::near_cells(world.centre, at, cells);
                for &cell in cells.iter() {
                    for &index in &world.cells[cell] {
                        if self.triangles[index as usize].normal.dot(up).abs() <= GROUND_NORMAL_Y {
                            out.push(index);
                        }
                    }
                }
            }
        }
    }

    /// Returns the first collision point along a segment. Used to prevent the
    /// third-person camera from passing through castle geometry.
    pub fn segment_hit(&self, start: Vec3, end: Vec3) -> Option<Vec3> {
        self.surface_hit(start, end).map(|(point, _)| point)
    }

    /// The first surface a segment meets, as the point it was met at and the
    /// way that surface faces.
    ///
    /// Unlike the floor and ceiling queries this one asks about a direction
    /// rather than about a column, which is what anything moving over the level
    /// rather than falling through it needs: a wall and a ceiling are surfaces
    /// here, not exceptions to be filtered out.
    ///
    /// The normal is turned to point back along the segment rather than taken
    /// as the triangle has it. The castle mesh is not consistently wound --
    /// which is why [`Self::highest_below`] picks by height and not by facing
    /// -- so a polygon's own normal is as likely to be its back as its front,
    /// and what a caller probing for a surface wants is the side it arrived
    /// from.
    pub fn surface_hit(&self, start: Vec3, end: Vec3) -> Option<(Vec3, Vec3)> {
        // Into the nearest copy's frame -- and against that copy's world --
        // judged from the segment's middle so a probe reaching down towards a
        // planet answers to the planet it is reaching for. The identity for
        // the castle and for a lone planet.
        let placed = self.planet_at((start + end) * 0.5);
        let frame = placed
            .map(|(_, frame)| frame)
            .unwrap_or(PlanetFrame::IDENTITY);
        let (start, end) = (frame.to_local(start), frame.to_local(end));
        let direction = end - start;
        let mut nearest = 1.0_f32;
        let mut hit = None;
        let min = Vec2::new(start.x.min(end.x), start.z.min(end.z));
        let max = Vec2::new(start.x.max(end.x), start.z.max(end.z));
        let cell_min = self.cell_coords(min);
        let cell_max = self.cell_coords(max);
        // A triangle spanning several of the cells this walks is tested once per
        // cell, and that is deliberate. There used to be a `[bool; 2048]` mark
        // table here to stop it -- two kilobytes of stack, zeroed on entry, to
        // save re-running an intersection test costing a few dozen flops
        // against a candidate list that on this castle averages eleven
        // triangles. The bookkeeping cost more than the work it saved, and it
        // was paid on every call: several per enemy per tick, times a field of
        // two thousand.
        //
        // Dropping it also drops the reason for a fallback. The table was
        // fixed-size, so a mesh with more triangles than it had slots had to go
        // through a brute-force scan of every triangle in the level instead;
        // with nothing to overflow, the grid path is now simply the path.
        //
        // Correctness never depended on it either way: the nearest hit is a
        // minimum over the candidates, and testing one of them twice returns
        // the same answer twice.
        match &self.index {
            Index::Flat { cells, .. } => {
                for z in cell_min.1..=cell_max.1 {
                    for x in cell_min.0..=cell_max.0 {
                        for &index in &cells[z * GRID_WIDTH + x].all {
                            let tri = self.triangles[index];
                            if let Some(t) = segment_triangle_time(start, direction, tri) {
                                if t < nearest {
                                    nearest = t;
                                    hit = Some((
                                        start + direction * t,
                                        facing_back(tri.normal, direction),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // A planet's cells are patches of surface rather than columns, so
            // the segment is walked rather than boxed: one step per cell's
            // worth of arc, each contributing the patch around where it landed.
            // Stepping is what keeps a ray fired along the ground -- a bullet,
            // the camera's boom -- from having to open a box the size of the
            // hemisphere it crosses.
            Index::Planet { .. } => {
                let (world, _) = placed?;
                let (centre, radius, filed) = (world.centre, world.radius, &world.cells);
                let arc = (FACE_CELL_ANGLE * radius).max(0.001);
                let steps = ((direction.length() / arc).ceil() as usize + 1).min(PLANET_RAY_STEPS);
                let mut cells_here = Vec::new();
                let mut walked = Vec::new();
                for step in 0..=steps {
                    let along = start + direction * (step as f32 / steps as f32);
                    // Points at the centre itself have no direction to file
                    // under; the ray is still tested against everything the
                    // steps either side of it turn up.
                    if (along - centre).length_squared() < 1e-6 {
                        continue;
                    }
                    Self::near_cells(centre, along, &mut cells_here);
                    for &cell in &cells_here {
                        if walked.contains(&cell) {
                            continue;
                        }
                        walked.push(cell);
                        for &index in &filed[cell] {
                            let tri = self.triangles[index as usize];
                            if let Some(t) = segment_triangle_time(start, direction, tri) {
                                if t < nearest {
                                    nearest = t;
                                    hit = Some((
                                        start + direction * t,
                                        facing_back(tri.normal, direction),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        // And back out of the copy's frame, so the caller's answer is where
        // the surface actually is -- the normal turned with it, or a turned
        // world's ground would push bodies along last hour's vertical.
        hit.map(|(point, normal)| (frame.to_world(point), frame.direction_to_world(normal)))
    }

    /// Every triangle in the level, tested in order. Kept as what
    /// [`Self::surface_hit`] is checked *against* rather than as a path the game
    /// takes: `castle_grid_matches_brute_force_queries` is the test that says
    /// the grid has not quietly started missing surfaces.
    #[cfg(test)]
    fn segment_hit_brute_force(&self, start: Vec3, end: Vec3) -> Option<(Vec3, Vec3)> {
        let direction = end - start;
        let mut nearest = 1.0_f32;
        let mut hit = None;
        for &tri in &self.triangles {
            if let Some(t) = segment_triangle_time(start, direction, tri) {
                if t < nearest {
                    nearest = t;
                    hit = Some((start + direction * t, facing_back(tri.normal, direction)));
                }
            }
        }
        hit
    }
}

/// The most steps a ray is walked in when it crosses a planet.
///
/// A cap rather than a limit anyone should reach: a hundred and twenty-eight
/// steps is most of the way round the 300 m planet at one cell of arc each. It
/// exists so that a ray fired at the sky -- which every aim probe is, some of
/// the time -- costs a bounded amount rather than one step per cell out to
/// wherever it was pointed.
const PLANET_RAY_STEPS: usize = 128;

/// Which of the six cube faces a direction points through, and where on that
/// face it lands, with `u` and `v` each running -1 to 1.
///
/// The plain gnomonic mapping, without the tangent warp
/// `experimental/planet_gen` applies when it *places* vertices. The two need
/// not agree: this decides which drawer a triangle is filed in, and any
/// mapping that is one-to-one and continuous within a face files and finds it
/// alike. Matching the generator's warp would buy slightly more even cells and
/// a second place for the two to drift apart.
fn face_uv(direction: Vec3) -> (usize, f32, f32) {
    let size = direction.abs();
    if size.x >= size.y && size.x >= size.z {
        let scale = size.x.max(f32::MIN_POSITIVE).recip();
        if direction.x > 0.0 {
            (0, direction.z * scale, direction.y * scale)
        } else {
            (1, -direction.z * scale, direction.y * scale)
        }
    } else if size.y >= size.z {
        let scale = size.y.max(f32::MIN_POSITIVE).recip();
        if direction.y > 0.0 {
            (2, direction.x * scale, direction.z * scale)
        } else {
            (3, direction.x * scale, -direction.z * scale)
        }
    } else {
        let scale = size.z.max(f32::MIN_POSITIVE).recip();
        if direction.z > 0.0 {
            (4, -direction.x * scale, direction.y * scale)
        } else {
            (5, direction.x * scale, direction.y * scale)
        }
    }
}

/// Whether a triangle is a sliver: so much longer than it is wide that it is
/// not a surface, and cannot be intersected against without lying.
///
/// Not the same test as "zero area", and the difference is the whole reason
/// this is a function. A UV sphere's pole is 96 vertices that ought to be the
/// same point and, in `f32` at 300 metres out, are not: `sin(PI)` is 8.7e-8, so
/// they are scattered over a few hundredths of a millimetre. The triangles
/// between them are twenty metres long and a micron wide. Their normal
/// normalises perfectly well, so a zero test does not see them, and the
/// determinant in [`segment_triangle_time`] -- which exists to reject exactly
/// this -- comes out thousands of times its own epsilon. What follows is a ray
/// straight down the axis of the planet reporting a hit two thirds of the way
/// to the core, on a surface that is not there.
///
/// Measured against the triangle's own size rather than an absolute area, so
/// it means the same thing on a 300 m planet and in a 3 m test room. The ratio
/// it rejects is a width under about a millionth of the length, which is
/// rounding noise rather than geometry anybody authored.
fn degenerate(tri: &CollisionTriangle) -> bool {
    let cross = (tri.b - tri.a).cross(tri.c - tri.a);
    let longest = (tri.b - tri.a)
        .length_squared()
        .max((tri.c - tri.b).length_squared())
        .max((tri.a - tri.c).length_squared());
    cross.length() <= longest * 1e-6
}

/// Files one triangle into every face cell it can be found in.
///
/// By sampling the triangle rather than by its corners alone. The corners are
/// enough for the planet the generator writes -- its terrain triangles are
/// under two metres across against a cell of five -- and that is exactly the
/// sort of thing that is true until someone changes [`FACE_GRID`], or exports
/// at a coarser depth, or drops in an authored tile with one big flat face on
/// it. Then a triangle spans cells with none of its corners in them, and what
/// that looks like from inside the game is a patch of ground the player falls
/// through.
///
/// So the sample spacing is derived from the triangle's own angular size
/// instead: half a cell between samples, which is close enough that any cell
/// the triangle crosses has a sample somewhere inside it. A triangle smaller
/// than a cell samples its three corners and nothing more, which is what makes
/// this free in the case that actually occurs.
fn file_triangle(
    tri: &CollisionTriangle,
    centre: Vec3,
    index: u32,
    filed: &mut Vec<usize>,
    cells: &mut [Vec<u32>],
) {
    let corners = [tri.a - centre, tri.b - centre, tri.c - centre];
    let directions = corners.map(|corner| corner.normalize_or(Vec3::Y));
    let widest = [(0, 1), (1, 2), (2, 0)]
        .into_iter()
        .map(|(from, to)| directions[from].dot(directions[to]).clamp(-1.0, 1.0).acos())
        .fold(0.0_f32, f32::max);
    let steps = ((widest * 2.0 / FACE_CELL_ANGLE).ceil() as usize).clamp(1, MAX_FILE_STEPS);
    filed.clear();
    for i in 0..=steps {
        for j in 0..=(steps - i) {
            let k = steps - i - j;
            // Direction only, so there is no need to normalise: a barycentric
            // blend of three points around the planet points at the patch of
            // sphere between them.
            let at = corners[0] * i as f32 + corners[1] * j as f32 + corners[2] * k as f32;
            let cell = face_cell(at);
            if filed.contains(&cell) {
                continue;
            }
            filed.push(cell);
            cells[cell].push(index);
        }
    }
}

/// The most samples one triangle is filed by, along each of its edges.
///
/// A cap on the pathological case rather than a limit real geometry reaches:
/// at 32 a single triangle covers a fifth of a face, and filing it exactly
/// would cost more than testing it from a neighbouring cell occasionally.
const MAX_FILE_STEPS: usize = 32;

/// Where a face coordinate in -1..1 falls among [`FACE_GRID`] cells.
fn cell_index(value: f32) -> usize {
    (((value + 1.0) * 0.5 * FACE_GRID as f32) as isize).clamp(0, FACE_GRID as isize - 1) as usize
}

/// The single face cell a direction points at.
fn face_cell(direction: Vec3) -> usize {
    let (face, u, v) = face_uv(direction);
    (face * FACE_GRID + cell_index(v)) * FACE_GRID + cell_index(u)
}

/// Height of a triangle's plane directly over `(x, z)`, when that column
/// passes through the triangle at all. Shared by the floor and ceiling
/// queries, which differ only in which of the hits they keep.
fn surface_y(tri: CollisionTriangle, x: f32, z: f32) -> Option<f32> {
    let (a, b, c) = (tri.a, tri.b, tri.c);
    let v0 = Vec2::new(b.x - a.x, b.z - a.z);
    let v1 = Vec2::new(c.x - a.x, c.z - a.z);
    let v2 = Vec2::new(x - a.x, z - a.z);
    let den = v0.x * v1.y - v1.x * v0.y;
    if den.abs() < 1e-7 {
        return None;
    }
    let u = (v2.x * v1.y - v1.x * v2.y) / den;
    let v = (v0.x * v2.y - v2.x * v0.y) / den;
    if u >= -0.001 && v >= -0.001 && u + v <= 1.001 {
        Some(a.y + u * (b.y - a.y) + v * (c.y - a.y))
    } else {
        None
    }
}

/// A triangle's normal, flipped when needed so that it points back the way a
/// probe came from. See [`LevelData::surface_hit`] for why it cannot be trusted
/// as it is stored.
fn facing_back(normal: Vec3, direction: Vec3) -> Vec3 {
    if normal.dot(direction) > 0.0 {
        -normal
    } else {
        normal
    }
}

fn segment_triangle_time(start: Vec3, direction: Vec3, tri: CollisionTriangle) -> Option<f32> {
    let (a, b, c) = (tri.a, tri.b, tri.c);
    let edge1 = b - a;
    let edge2 = c - a;
    let p = direction.cross(edge2);
    let determinant = edge1.dot(p);
    if determinant.abs() < 1e-7 {
        return None;
    }
    let inverse = determinant.recip();
    let tvec = start - a;
    let u = tvec.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = tvec.cross(edge1);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = edge2.dot(q) * inverse;
    (0.0..1.0).contains(&t).then_some(t)
}

/// One wall pressing on a capsule: which way it wants the body to go, and how
/// far.
///
/// A plane constraint rather than a triangle. Once the closest points are
/// known that is all the shape a wall has left, and reducing it here is what
/// lets [`LevelData::resolve_walls`] combine several of them without caring
/// which triangle each came from.
#[derive(Clone, Copy)]
struct Contact {
    /// Unit vector, flat against the local ground, pointing out of the wall.
    direction: Vec3,
    /// Metres along `direction` the body has to move to be clear.
    depth: f32,
}

/// The closest pair of points between a segment and a triangle: one on the
/// segment, one on the triangle.
///
/// This is the capsule test with the radius left out. A capsule is a segment
/// swept by a radius, so the distance between its spine and a triangle,
/// compared against that radius, is exactly whether the two overlap and by how
/// much.
///
/// The answer is either at an end of the segment, along one of the triangle's
/// edges, or -- when the segment runs through the face -- at the crossing
/// itself. Testing those cases and keeping the nearest is the whole method.
fn closest_points_segment_triangle(p: Vec3, q: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Vec3) {
    // A segment that passes through the face touches it, and no candidate
    // taken from the boundary would say so: both ends can be well clear of
    // every edge while the middle is through the middle.
    let normal = (b - a).cross(c - a);
    let denominator = normal.dot(q - p);
    if denominator.abs() > 1e-12 {
        let time = normal.dot(a - p) / denominator;
        if (0.0..=1.0).contains(&time) {
            let at = p + (q - p) * time;
            if closest_point_on_triangle(at, a, b, c).distance_squared(at) <= 1e-10 {
                return (at, at);
            }
        }
    }
    let pairs = [
        (p, closest_point_on_triangle(p, a, b, c)),
        (q, closest_point_on_triangle(q, a, b, c)),
        closest_points_on_segments(p, q, a, b),
        closest_points_on_segments(p, q, b, c),
        closest_points_on_segments(p, q, c, a),
    ];
    let mut best = pairs[0];
    let mut nearest = f32::INFINITY;
    for (on_segment, on_triangle) in pairs {
        let distance = on_segment.distance_squared(on_triangle);
        if distance < nearest {
            nearest = distance;
            best = (on_segment, on_triangle);
        }
    }
    best
}

/// The closest pair of points between two segments.
///
/// Real-Time Collision Detection, Christer Ericson, section 5.1.9. Parallel
/// segments have a whole interval of equally close pairs and the degenerate
/// branch below picks one end of it, which is all a depth query needs.
fn closest_points_on_segments(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    const TINY: f32 = 1e-12;
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let squared1 = d1.dot(d1);
    let squared2 = d2.dot(d2);
    let along2 = d2.dot(r);
    if squared1 <= TINY && squared2 <= TINY {
        return (p1, p2);
    }
    let (s, t);
    if squared1 <= TINY {
        s = 0.0;
        t = (along2 / squared2).clamp(0.0, 1.0);
    } else {
        let along1 = d1.dot(r);
        if squared2 <= TINY {
            t = 0.0;
            s = (-along1 / squared1).clamp(0.0, 1.0);
        } else {
            let between = d1.dot(d2);
            let denominator = squared1 * squared2 - between * between;
            let first = if denominator > TINY {
                ((between * along2 - along1 * squared2) / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let second = (between * first + along2) / squared2;
            if second < 0.0 {
                t = 0.0;
                s = (-along1 / squared1).clamp(0.0, 1.0);
            } else if second > 1.0 {
                t = 1.0;
                s = ((between - along1) / squared1).clamp(0.0, 1.0);
            } else {
                t = second;
                s = first;
            }
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

fn closest_point_on_triangle(point: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Real-Time Collision Detection, Christer Ericson, section 5.1.5.
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = point - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = point - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denominator = (va + vb + vc).recip();
    a + ab * (vb * denominator) + ac * (vc * denominator)
}

/// What the collision grid has decided a triangle is.
///
/// Not a property of the triangle so much as of the *filing*: which of the
/// grid's two lists it went into, which is what settles whether a body stands
/// on it, is held off it, or goes straight through. The two lists overlap, and
/// nearly every "I fell through the floor" and "it is standing inside the
/// wall" is about a face on the wrong side of one of the two lines below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaceKind {
    /// Leaning less than [`GROUND_NORMAL_Y`] off flat: something to stand on,
    /// and something [`LevelData::resolve_walls`] will never push a body out
    /// of. Ground is only ever under you.
    Ground,
    /// Steeper than ground, not quite on edge: filed as a wall *and* as a
    /// floor. It shoves a body sideways, and it is also what a falling body
    /// can land on -- so a body pressed into one may be resting on a face that
    /// is at the same time trying to eject it.
    Ledge,
    /// On edge to within [`FLOOR_FILE_LEAN`], and so a wall and nothing else.
    /// Nothing stands on one of these: get past it and there is no floor
    /// underneath, which is the shape of most of the falling through.
    Wall,
}

/// One triangle of collision, as the overlay reads it.
pub struct DebugFace {
    pub corners: [Vec3; 3],
    /// The triangle's own normal. The converted castle mesh is not wound
    /// consistently, so this points out of the floor about as often as it
    /// points into it -- which is why every query here measures its lean on
    /// the absolute value and so does [`LevelData::face`].
    pub normal: Vec3,
    pub kind: FaceKind,
}

/// One wall a body is being resolved against, and what that wall is doing to
/// it at this instant.
pub struct DebugWall {
    pub face: DebugFace,
    /// Where on the triangle the body's spine comes nearest it.
    pub nearest: Vec3,
    /// The push this wall is making right now, as direction times depth, or
    /// zero where the two are clear of each other. A body standing still with
    /// a push on it is a body the resolution has given up on.
    pub push: Vec3,
}

/// One cell of the flat collision grid.
pub struct DebugCell {
    /// The low corner, in world `(x, z)`.
    pub min: Vec2,
    pub size: Vec2,
    /// How many triangles a floor query in this cell walks. Zero is a hole:
    /// nothing here can hold anything up.
    pub floors: usize,
    /// How many a wall resolution walks.
    pub walls: usize,
}

/// What the collision looks like from outside, for the overlay in
/// [`crate::collide`].
///
/// Deliberately not a second set of queries. Everything here either hands back
/// the very list a real query walks, or runs the real test -- because the only
/// use for any of it is answering "why did the collision do that", and a view
/// that works the answer out its own way cannot.
impl LevelData {
    /// Every triangle filed within `reach` of `at`, with each face reported
    /// where it stands -- which on a copied planet is the copy's frame, not
    /// the filed geometry's.
    pub fn faces_near(&self, at: Vec3, reach: f32, out: &mut Vec<DebugFace>) {
        out.clear();
        let placed = self.planet_at(at);
        let frame = placed
            .map(|(_, frame)| frame)
            .unwrap_or(PlanetFrame::IDENTITY);
        let at = frame.to_local(at);
        let mut found: Vec<u32> = Vec::new();
        match placed {
            None => {
                let Index::Flat { cells, .. } = &self.index else {
                    return;
                };
                let low = self.cell_coords(Vec2::new(at.x - reach, at.z - reach));
                let high = self.cell_coords(Vec2::new(at.x + reach, at.z + reach));
                for z in low.1..=high.1 {
                    for x in low.0..=high.0 {
                        found.extend(cells[z * GRID_WIDTH + x].all.iter().map(|&i| i as u32));
                    }
                }
            }
            Some((world, _)) => {
                // A patch of face cells around the one the point is over. The
                // sweep is in angle rather than in metres because that is what
                // a face cell is measured in; `reach` becomes an arc on the
                // way in and a handful of cells on the way out.
                let radial = (at - world.centre).normalize_or(Vec3::Y);
                let (along, across) = radial.any_orthonormal_pair();
                let arc = reach / world.radius.max(1.0) / FACE_CELL_ANGLE;
                let steps = (arc.ceil() as isize).clamp(1, DEBUG_FACE_STEPS);
                for down in -steps..=steps {
                    for right in -steps..=steps {
                        let towards = radial
                            + along * (right as f32 * FACE_CELL_ANGLE)
                            + across * (down as f32 * FACE_CELL_ANGLE);
                        found.extend(&world.cells[face_cell(towards)]);
                    }
                }
            }
        }
        // One triangle spans several cells and is filed in every one of them.
        found.sort_unstable();
        found.dedup();
        let centre = placed.map(|(world, _)| world.centre);
        for index in found {
            let tri = self.triangles[index as usize];
            let near = match centre {
                // The filing is by `(x, z)` and so is the range: a triangle is
                // in reach if its own footprint is. Without this the one big
                // floor slab under a hall drags its whole outline in from
                // wherever it happens to start.
                None => {
                    tri.max.x >= at.x - reach
                        && tri.min.x <= at.x + reach
                        && tri.max.y >= at.z - reach
                        && tri.min.y <= at.z + reach
                }
                // A face cell is already a patch of surface, so anything filed
                // in the ones sampled above is near by construction.
                Some(_) => true,
            };
            if near {
                let mut face = Self::face(tri, centre);
                for corner in &mut face.corners {
                    *corner = frame.to_world(*corner);
                }
                face.normal = frame.direction_to_world(face.normal);
                out.push(face);
            }
        }
    }

    /// The walls a body standing at `at` is resolved against, each with the
    /// push it is making.
    ///
    /// The candidate list [`Self::resolve_walls`] walks, run through the same
    /// test it uses -- so a triangle listed here with no push is one the
    /// resolution looked at and let be, and that distinction is the whole
    /// question when a body is standing in something.
    pub fn wall_contacts(
        &self,
        at: Vec3,
        up: Vec3,
        radius: f32,
        height: f32,
        out: &mut Vec<DebugWall>,
    ) {
        out.clear();
        // The same frame and world [`Self::resolve_walls`] works in, or the
        // overlay disagrees with the resolution it exists to explain.
        let placed = self.planet_at(at);
        let frame = placed
            .map(|(_, frame)| frame)
            .unwrap_or(PlanetFrame::IDENTITY);
        let world = placed.map(|(world, _)| world);
        let at = frame.to_local(at);
        let up = frame.direction_to_local(up);
        let mut cells = Vec::new();
        let mut candidates = Vec::new();
        self.walls_near(at, up, world, &mut cells, &mut candidates);
        let foot = at + up * radius;
        let head = at + up * (height - radius).max(radius);
        for index in candidates {
            let tri = self.triangles[index as usize];
            let (_, nearest) = closest_points_segment_triangle(foot, head, tri.a, tri.b, tri.c);
            let push = self
                .contact(index, foot, head, up, radius)
                .map_or(Vec3::ZERO, |contact| contact.direction * contact.depth);
            let mut face = Self::face(tri, world.map(|world| world.centre));
            for corner in &mut face.corners {
                *corner = frame.to_world(*corner);
            }
            face.normal = frame.direction_to_world(face.normal);
            out.push(DebugWall {
                face,
                nearest: frame.to_world(nearest),
                push: frame.direction_to_world(push),
            });
        }
    }

    /// The grid cell a query at `point` is answered from, or `None` on a
    /// planet, which files by face cell and has no such square.
    pub fn cell_footprint(&self, point: Vec3) -> Option<DebugCell> {
        let (cells, min, size) = self.flat()?;
        let (x, z) = self.cell_coords(Vec2::new(point.x, point.z));
        let cell = &cells[z * GRID_WIDTH + x];
        Some(DebugCell {
            min: min + size * Vec2::new(x as f32, z as f32),
            size,
            floors: cell.floors.len(),
            walls: cell.walls.len(),
        })
    }

    /// One triangle, sorted into the same three boxes the filing sorts it
    /// into -- measured against the up that holds where the triangle is:
    /// `+Y` on a flat level, and away from its own world's `centre` on a
    /// planet, a different direction for every one of them.
    fn face(tri: CollisionTriangle, centre: Option<Vec3>) -> DebugFace {
        let middle = (tri.a + tri.b + tri.c) / 3.0;
        let up = match centre {
            None => Vec3::Y,
            Some(centre) => (middle - centre).normalize_or(Vec3::Y),
        };
        let lean = tri.normal.dot(up).abs();
        DebugFace {
            corners: [tri.a, tri.b, tri.c],
            normal: tri.normal,
            kind: match lean {
                _ if lean > GROUND_NORMAL_Y => FaceKind::Ground,
                _ if lean > FLOOR_FILE_LEAN => FaceKind::Ledge,
                _ => FaceKind::Wall,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(vertices: &[Vec3], triangles: &[[u32; 3]]) -> LevelData {
        LevelData::new(vertices.to_vec(), triangles.to_vec(), Vec::new())
    }

    #[test]
    fn floor_chooses_highest_surface_below_probe() {
        let data = level(
            &[
                Vec3::new(-2., 0., -2.),
                Vec3::new(2., 0., -2.),
                Vec3::new(0., 0., 2.),
                Vec3::new(-2., 2., -2.),
                Vec3::new(2., 2., -2.),
                Vec3::new(0., 2., 2.),
            ],
            &[[0, 1, 2], [3, 4, 5]],
        );
        assert_eq!(data.floor_height(Vec3::new(0., 3., 0.)), Some(2.0));
        assert_eq!(data.floor_height(Vec3::new(0., 1., 0.)), Some(0.0));
    }

    #[test]
    fn wall_resolution_pushes_capsule_out() {
        let data = level(
            &[
                Vec3::new(0., 0., -2.),
                Vec3::new(0., 3., -2.),
                Vec3::new(0., 0., 2.),
            ],
            &[[0, 1, 2]],
        );
        let corrected = data.resolve_walls(Vec3::new(0.2, 0., 0.), Vec3::Y, 0.5, 1.8);
        assert!(corrected.x >= 0.5);
    }

    #[test]
    fn wall_resolution_clears_a_wall_the_middle_of_the_body_meets() {
        // A parapet from waist to shoulder: nothing at the feet, nothing over
        // the head. Sample spheres strung along the body can miss a band like
        // this between two of them; the capsule's spine runs through it.
        let data = level(
            &[
                Vec3::new(0., 0.8, -2.),
                Vec3::new(0., 1.2, -2.),
                Vec3::new(0., 0.8, 2.),
            ],
            &[[0, 1, 2]],
        );
        let corrected = data.resolve_walls(Vec3::new(0.2, 0., 0.), Vec3::Y, 0.5, 1.8);
        assert!(
            corrected.x >= 0.5,
            "left standing in the parapet at {corrected}"
        );
    }

    #[test]
    fn a_face_below_the_deck_is_not_a_wall_on_it() {
        // The shape of every bridge over the castle's moat: something to stand
        // on, and a sheer face dropping away under its edge. A body on the
        // deck has that face metres below its feet, near enough underneath the
        // edge that its *horizontal* distance is almost nothing -- so a test
        // that asks only how far away it is sideways finds a wall right there
        // and shoves the body off the middle of the bridge. That is an
        // invisible wall, and it is what a capsule is meant not to have.
        let data = level(
            &[
                // The deck, four metres square about the origin.
                Vec3::new(-2., 0., -2.),
                Vec3::new(2., 0., -2.),
                Vec3::new(2., 0., 2.),
                Vec3::new(-2., 0., 2.),
                // The face under its western edge, dropping eight metres.
                Vec3::new(-2., -1., -2.),
                Vec3::new(-2., -1., 2.),
                Vec3::new(-2., -9., -2.),
                Vec3::new(-2., -9., 2.),
            ],
            &[[0, 1, 2], [0, 2, 3], [4, 5, 6], [5, 7, 6]],
        );
        // Standing on the deck a foot from that edge, and then right on it.
        for x in [-1.0, -1.5, -1.9] {
            let at = Vec3::new(x, 0., 0.);
            let shoved = data.resolve_walls(at, Vec3::Y, 0.5, 1.8);
            assert!(
                (shoved - at).length() < 1e-3,
                "a face {} m below the feet moved a body standing at {at} to {shoved}",
                1.0,
            );
        }
        // And the same face, brought up to stand across the deck instead, is a
        // wall again -- the rule is where the thing is, not what it is.
        let parapet = level(
            &[
                Vec3::new(-2., 0., -2.),
                Vec3::new(2., 0., -2.),
                Vec3::new(2., 0., 2.),
                Vec3::new(-2., 0., 2.),
                Vec3::new(-1., 0., -2.),
                Vec3::new(-1., 0., 2.),
                Vec3::new(-1., 2., -2.),
                Vec3::new(-1., 2., 2.),
            ],
            &[[0, 1, 2], [0, 2, 3], [4, 5, 6], [5, 7, 6]],
        );
        let at = Vec3::new(-0.8, 0., 0.);
        let shoved = parapet.resolve_walls(at, Vec3::Y, 0.5, 1.8);
        assert!(shoved.x >= -0.5, "walked through the parapet to {shoved}");
    }

    #[test]
    fn wall_resolution_does_not_depend_on_triangle_order() {
        // The inside of a corner, so that resolving one face walks the body
        // into the other and the order the two are taken in could matter.
        let vertices = [
            Vec3::new(0., 0., -2.),
            Vec3::new(0., 3., -2.),
            Vec3::new(0., 0., 2.),
            Vec3::new(-2., 0., 0.),
            Vec3::new(-2., 3., 0.),
            Vec3::new(2., 0., 0.),
        ];
        let forwards = level(&vertices, &[[0, 1, 2], [3, 4, 5]]);
        let backwards = level(&vertices, &[[3, 4, 5], [0, 1, 2]]);
        let start = Vec3::new(0.2, 0., 0.2);
        let first = forwards.resolve_walls(start, Vec3::Y, 0.5, 1.8);
        let second = backwards.resolve_walls(start, Vec3::Y, 0.5, 1.8);
        assert_eq!(first, second, "the mesh's build order moved the body");
        // And it is out of both walls, not merely consistently in one of them.
        assert!(first.x >= 0.5, "still in the first wall at {first}");
        assert!(first.z >= 0.5, "still in the second wall at {first}");
    }

    #[test]
    fn ceiling_finds_the_lowest_surface_overhead() {
        let data = level(
            &[
                Vec3::new(-2., 0., -2.),
                Vec3::new(2., 0., -2.),
                Vec3::new(0., 0., 2.),
                Vec3::new(-2., 3., -2.),
                Vec3::new(2., 3., -2.),
                Vec3::new(0., 3., 2.),
                Vec3::new(-2., 6., -2.),
                Vec3::new(2., 6., -2.),
                Vec3::new(0., 6., 2.),
            ],
            &[[0, 1, 2], [3, 4, 5], [6, 7, 8]],
        );
        // From the ground floor the middle slab is the ceiling, not the roof.
        assert_eq!(data.ceiling_height(Vec3::new(0., 0., 0.), 0.5), Some(3.0));
        // Standing on the middle slab, that same slab is now the floor and the
        // roof above becomes the ceiling.
        assert_eq!(data.ceiling_height(Vec3::new(0., 3., 0.), 0.5), Some(6.0));
        assert_eq!(data.ceiling_height(Vec3::new(0., 6., 0.), 0.5), None);
        // Outside the triangles' footprint there is nothing overhead.
        assert_eq!(data.ceiling_height(Vec3::new(9., 0., 0.), 0.5), None);
    }

    #[test]
    fn segment_hit_returns_nearest_intersection() {
        let data = level(
            &[
                Vec3::new(-2., -2., 0.),
                Vec3::new(2., -2., 0.),
                Vec3::new(0., 2., 0.),
            ],
            &[[0, 1, 2]],
        );
        let hit = data
            .segment_hit(Vec3::new(0., 0., -2.), Vec3::new(0., 0., 2.))
            .unwrap();
        assert!(hit.z.abs() < 1e-5);
        // And the surface it hit faces back at whatever asked, whichever way
        // round the triangle happens to be wound.
        let (_, normal) = data
            .surface_hit(Vec3::new(0., 0., -2.), Vec3::new(0., 0., 2.))
            .unwrap();
        assert!(normal.z < -0.99, "{normal:?}");
        let (_, normal) = data
            .surface_hit(Vec3::new(0., 0., 2.), Vec3::new(0., 0., -2.))
            .unwrap();
        assert!(normal.z > 0.99, "{normal:?}");
    }

    #[test]
    fn water_box_lookup_respects_bounds() {
        let mut data = level(&[], &[]);
        data.water_boxes.push(WaterBox {
            min_x: -2.0,
            min_z: -3.0,
            max_x: 4.0,
            max_z: 5.0,
            surface_y: 1.25,
        });
        assert_eq!(data.water_depth(Vec3::new(0.0, 0.25, 0.0)), Some(1.0));
        assert_eq!(data.water_depth(Vec3::new(0.0, 3.0, 0.0)), Some(-1.75));
        assert_eq!(data.water_depth(Vec3::new(5.0, 0.0, 0.0)), None);
    }

    /// The sea covers a whole planet at one radius, so "how deep" is the same
    /// subtraction wherever it is asked and there is no box to be inside of.
    #[test]
    fn a_planet_measures_its_depth_from_the_sea_radius() {
        let planet = LevelData::planet(&[], &[], Vec3::ZERO, 300.0, Some(300.0));
        assert_eq!(planet.water_depth(Vec3::new(0.0, 295.0, 0.0)), Some(5.0));
        // The same five metres under, a quarter of the way round the world --
        // the thing a flat water box cannot say.
        assert_eq!(planet.water_depth(Vec3::new(295.0, 0.0, 0.0)), Some(5.0));
        // Land pokes through the sea, and that reads as a negative depth
        // rather than as no water: there is sea here, this point is over it.
        assert_eq!(planet.water_depth(Vec3::new(0.0, 320.0, 0.0)), Some(-20.0));
        let dry = LevelData::planet(&[], &[], Vec3::ZERO, 300.0, None);
        assert_eq!(dry.water_depth(Vec3::new(0.0, 295.0, 0.0)), None);
    }

    fn brute_floor(data: &LevelData, point: Vec3) -> Option<f32> {
        let mut best = None;
        for tri in &data.triangles {
            let v0 = Vec2::new(tri.b.x - tri.a.x, tri.b.z - tri.a.z);
            let v1 = Vec2::new(tri.c.x - tri.a.x, tri.c.z - tri.a.z);
            let v2 = Vec2::new(point.x - tri.a.x, point.z - tri.a.z);
            let den = v0.x * v1.y - v1.x * v0.y;
            if den.abs() < 1e-7 {
                continue;
            }
            let u = (v2.x * v1.y - v1.x * v2.y) / den;
            let v = (v0.x * v2.y - v2.x * v0.y) / den;
            if u >= -0.001 && v >= -0.001 && u + v <= 1.001 {
                let y = tri.a.y + u * (tri.b.y - tri.a.y) + v * (tri.c.y - tri.a.y);
                if y <= point.y + 0.5 && best.is_none_or(|old| y > old) {
                    best = Some(y);
                }
            }
        }
        best
    }

    #[test]
    fn castle_grid_matches_brute_force_queries() {
        let (data, _) = load();
        for z in (-80..=80).step_by(8) {
            for x in (-80..=80).step_by(8) {
                let point = Vec3::new(x as f32, 20.0, z as f32);
                assert_eq!(data.floor_height(point), brute_floor(&data, point));
            }
        }
        let rays = [
            (Vec3::new(-13., 5., 46.), Vec3::new(-13., 8., 58.)),
            (Vec3::new(0., 4., 0.), Vec3::new(12., 9., 11.)),
            (Vec3::new(-50., 5., -40.), Vec3::new(-42., 10., -52.)),
        ];
        for (start, end) in rays {
            assert_eq!(
                data.surface_hit(start, end),
                data.segment_hit_brute_force(start, end)
            );
        }
    }

    #[test]
    fn castle_grid_reduces_floor_candidates() {
        let (data, _) = load();
        let Some((cells, _, _)) = data.flat() else {
            panic!("the castle is not a flat level any more");
        };
        let total: usize = cells.iter().map(|cell| cell.floors.len()).sum();
        let average = total as f32 / cells.len() as f32;
        assert!(
            average < data.triangles.len() as f32 * 0.2,
            "average {average}"
        );
    }

    /// A ball of terrain: a low-detail sphere of `radius`, wound outwards, with
    /// every triangle small enough that the face grid files it the way a real
    /// planet's is filed.
    fn ball(radius: f32) -> LevelData {
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        // A UV sphere rather than a cube-sphere on purpose: the index must not
        // depend on the mesh having been built by the same mapping it files by.
        let (rings, segments) = (48usize, 96usize);
        for ring in 0..=rings {
            let pitch = std::f32::consts::PI * ring as f32 / rings as f32;
            for segment in 0..=segments {
                let yaw = std::f32::consts::TAU * segment as f32 / segments as f32;
                vertices.push(
                    Vec3::new(
                        pitch.sin() * yaw.cos(),
                        pitch.cos(),
                        pitch.sin() * yaw.sin(),
                    ) * radius,
                );
            }
        }
        let at = |ring: usize, segment: usize| (ring * (segments + 1) + segment) as u32;
        for ring in 0..rings {
            for segment in 0..segments {
                indices.push([
                    at(ring, segment),
                    at(ring + 1, segment),
                    at(ring, segment + 1),
                ]);
                indices.push([
                    at(ring, segment + 1),
                    at(ring + 1, segment),
                    at(ring + 1, segment + 1),
                ]);
            }
        }
        LevelData::planet(&vertices, &indices, Vec3::ZERO, radius, None)
    }

    /// The claim the whole face grid exists to support: a planet answers "what
    /// is under my feet" everywhere on it, and the answer is the surface right
    /// there rather than the one on the far side.
    #[test]
    fn a_planet_has_ground_under_every_point_of_it() {
        let radius = 300.0;
        let planet = ball(radius);
        assert_eq!(
            planet.shape(),
            Shape::Planet {
                centre: Vec3::ZERO,
                radius
            }
        );
        // Directions spread over the sphere rather than over one face, so the
        // eight cube corners and all twelve seams are crossed.
        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        for step in 0..400 {
            let height = 1.0 - 2.0 * (step as f32 + 0.5) / 400.0;
            let ring = (1.0 - height * height).max(0.0).sqrt();
            let yaw = golden * step as f32;
            let up = Vec3::new(ring * yaw.cos(), height, ring * yaw.sin()).normalize();
            let standing = up * (radius + 1.0);
            let found = planet.ground_below(standing, up);
            let Some((point, normal)) = found else {
                panic!("no ground under {up} -- the face grid has a hole in it");
            };
            assert!(
                (point.length() - radius).abs() < 1.0,
                "{up}: ground at {} rather than {radius}",
                point.length()
            );
            assert!(
                normal.dot(up) > 0.8,
                "{up}: the ground there faces {normal}, which is not upwards"
            );
        }
    }

    /// The far side of the world is not the floor. This is the failure the flat
    /// `(x, z)` grid would produce on a planet, and the reason there is a
    /// second index at all.
    #[test]
    fn a_planet_does_not_answer_with_the_other_hemisphere() {
        let radius = 300.0;
        let planet = ball(radius);
        let up = Vec3::Y;
        let (point, _) = planet
            .ground_below(up * (radius + 2.0), up)
            .expect("no ground at the north pole");
        assert!(point.y > 0.0, "the north pole stood on the south pole");
        // And nothing is found from deep inside, where the surface is further
        // away than the search reaches.
        assert_eq!(planet.ground_below(Vec3::ZERO, up), None);
        assert!(planet.out_of_bounds(Vec3::ZERO));
        assert!(!planet.out_of_bounds(up * radius));
    }

    /// A smooth ball, genuinely spinning the way [`crate::orbit::advance`]
    /// spins a planet, holds nothing against a body riding twenty metres over
    /// it: no wall ever pushes, and the ground below is always the surface
    /// straight down. This is the moving-frame machinery tested with the
    /// terrain taken out of the experiment -- a push here is a query mapping
    /// through the wrong rotation, not a mountainside.
    #[test]
    fn a_spinning_ball_holds_nothing_against_clear_air() {
        let radius = 300.0;
        let mut planet = ball(radius);
        let centre = Vec3::new(5200.0, 0.0, 1700.0);
        let spin_step = 1.2_f32.to_radians() / 30.0;
        // Seats spread from pole to equator, each carried exactly with the
        // ground the way a full-grip rider is.
        let seats = [
            Vec3::new(0.05, 1.0, -0.08).normalize(),
            Vec3::new(0.5, 0.7, 0.2).normalize(),
            Vec3::new(1.0, 0.05, 0.0).normalize(),
        ];
        for tick in 0..600 {
            let rotation = Quat::from_rotation_y(spin_step * tick as f32);
            planet.place_planets(&[(centre, rotation)]);
            for seat in seats {
                let at = centre + rotation * (seat * (radius + 20.0));
                let up = (at - centre).normalize();
                // Exactly, to the bit: a body no wall touched must come back
                // untouched. A tolerance here once hid the frame round-trip
                // losing its last float bit, which the movement code read as
                // a wall push and paid for in velocity.
                let resolved = planet.resolve_walls(at, up, 0.42, 1.75);
                assert!(
                    resolved == at,
                    "tick {tick}, seat {seat}: twenty metres of clear air pushed {} m",
                    (resolved - at).length()
                );
                let (ground, _) = planet
                    .ground_below(at, up)
                    .unwrap_or_else(|| panic!("tick {tick}, seat {seat}: no ground below"));
                let clearance = (at - ground).dot(up);
                assert!(
                    (clearance - 20.0).abs() < 1.5,
                    "tick {tick}, seat {seat}: the ground stands {clearance} m down"
                );
            }
        }
    }

    /// Walls on a planet are steep against the local up rather than against
    /// `+Y`, so the same capsule is pushed out of the same slope wherever on
    /// the planet that slope is.
    #[test]
    fn a_planet_pushes_a_capsule_out_of_its_own_walls() {
        let radius = 300.0;
        let planet = ball(radius);
        for up in [
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::X,
            Vec3::new(1., 1., 1.).normalize(),
        ] {
            // Half a metre under the surface: the sphere's own facets are the
            // wall here, and the capsule has to come back out along the local
            // horizontal.
            let sunk = up * (radius - 0.5);
            let out = planet.resolve_walls(sunk, up, 0.42, 1.75);
            assert!(
                (out - sunk).length() < 1.0,
                "{up}: pushed {} metres, which is not a nudge",
                (out - sunk).length()
            );
        }
    }

    /// A copied planet is the same world standing somewhere else: every query
    /// asked beside the copy is answered by the copy, in the copy's own
    /// coordinates, and the original is untouched by its existence.
    #[test]
    fn a_copied_planet_answers_where_it_stands() {
        let radius = 300.0;
        let offset = Vec3::new(1100.0, 0.0, 0.0);
        let mut planet = ball(radius);
        planet.place_planets(&[(Vec3::ZERO, Quat::IDENTITY), (offset, Quat::IDENTITY)]);

        // Ground under a point over the copy is the copy's surface.
        let up = Vec3::Y;
        let standing = offset + up * (radius + 1.0);
        let (point, normal) = planet
            .ground_below(standing, up)
            .expect("no ground on the second planet");
        assert!(
            ((point - offset).length() - radius).abs() < 1.0,
            "the copy's ground is {} from its middle",
            (point - offset).length()
        );
        assert!(normal.dot(up) > 0.8);

        // And the original still answers exactly as it did alone.
        let (home, _) = ball(radius)
            .ground_below(up * (radius + 1.0), up)
            .expect("the lone planet lost its ground");
        let (still, _) = planet
            .ground_below(up * (radius + 1.0), up)
            .expect("the first planet lost its ground to the copy");
        assert!((home - still).length() < 1e-4);

        // Wall resolution beside the copy is the original's answer, moved.
        // Same algorithm, same triangles, frames apart by exactly `offset` --
        // so the two must agree to the bit, wherever on the world the capsule
        // stands and whether or not anything there pushes.
        let lone = ball(radius);
        for local_up in [Vec3::Y, Vec3::X, Vec3::new(1., 1., 1.).normalize()] {
            let local = local_up * (radius - 0.5);
            let by_the_copy = planet.resolve_walls(local + offset, local_up, 0.42, 1.75) - offset;
            let alone = lone.resolve_walls(local, local_up, 0.42, 1.75);
            assert!(
                (by_the_copy - alone).length() < 1e-4,
                "{local_up}: the copy resolved to {by_the_copy}, the original to {alone}"
            );
        }

        // The copy's core is out of bounds and the space between is not.
        assert!(planet.out_of_bounds(offset));
        assert!(!planet.out_of_bounds(Vec3::X * 550.0));
    }

    /// The copy's sea sits round the copy: depth is measured from whichever
    /// planet the point is nearest, so both shorelines are wet and the space
    /// between the worlds is very, very dry.
    #[test]
    fn a_copied_planet_carries_its_sea_with_it() {
        let radius = 300.0;
        let offset = Vec3::new(1100.0, 0.0, 0.0);
        let mut planet = LevelData::planet(&[], &[], Vec3::ZERO, radius, Some(radius));
        planet.place_planets(&[(Vec3::ZERO, Quat::IDENTITY), (offset, Quat::IDENTITY)]);
        let depth_home = planet.water_depth(Vec3::X * (radius - 2.0)).unwrap();
        let depth_copy = planet
            .water_depth(offset + Vec3::Y * (radius - 2.0))
            .unwrap();
        assert!((depth_home - 2.0).abs() < 1e-3, "{depth_home}");
        assert!((depth_copy - 2.0).abs() < 1e-3, "{depth_copy}");
        let space = planet.water_depth(Vec3::X * 550.0).unwrap();
        assert!(space < -200.0, "space is {space} m deep");
    }

    /// A turned planet is the same world mid-spin: a query asked in world
    /// space goes through the turn, reaches the filed geometry, and comes back
    /// turned -- point and normal both. This is the whole of what
    /// [`LevelData::place_planets`] promises the orbit, and it is checked as
    /// an equivalence against the lone planet rather than against numbers:
    /// same triangles, one frame apart.
    #[test]
    fn a_turned_planet_answers_through_its_own_spin() {
        let radius = 300.0;
        let lone = ball(radius);
        let stands_at = Vec3::new(2600.0, 0.0, -400.0);
        let spin = Quat::from_rotation_y(1.1);
        let mut placed = ball(radius);
        placed.place_planets(&[(stands_at, spin)]);
        // Directions off the test ball's seams and poles: a probe *on* one
        // lands exactly on a triangle edge, where which facet answers is a
        // float coin-toss the equivalence must not depend on.
        for local_up in [
            Vec3::new(0.3, 0.9, 0.2).normalize(),
            Vec3::new(0.9, 0.2, -0.4).normalize(),
            Vec3::new(-0.5, -0.7, 0.6).normalize(),
        ] {
            let (point, normal) = lone
                .ground_below(local_up * (radius + 2.0), local_up)
                .expect("the lone planet has no ground");
            let world_up = spin * local_up;
            let (turned, turned_normal) = placed
                .ground_below(stands_at + world_up * (radius + 2.0), world_up)
                .expect("the turned planet has no ground");
            assert!(
                (turned - (stands_at + spin * point)).length() < 1e-2,
                "{local_up}: the spin left the ground at {turned} rather than {}",
                stands_at + spin * point
            );
            // A facet's worth of slack: the round-tripped probe can land a
            // whisker into the neighbouring triangle, whose normal differs by
            // the tessellation's step and no more.
            assert!(
                (turned_normal - spin * normal).length() < 0.12,
                "{local_up}: the ground's facing did not turn with the world"
            );
            // The capsule resolution goes through the same frame.
            let sunk_local = local_up * (radius - 0.5);
            let alone = lone.resolve_walls(sunk_local, local_up, 0.42, 1.75);
            let by_the_turn =
                placed.resolve_walls(stands_at + spin * sunk_local, world_up, 0.42, 1.75);
            assert!(
                (by_the_turn - (stands_at + spin * alone)).length() < 1e-2,
                "{local_up}: the turned capsule came out somewhere else"
            );
        }
        // And the bounds went with it: the core is where the planet now is,
        // and where it used to be is open space.
        assert!(placed.out_of_bounds(stands_at));
        assert!(!placed.out_of_bounds(stands_at + Vec3::Y * radius));
        assert!(!placed.out_of_bounds(Vec3::ZERO));
    }

    /// The flat level must come through the generalisation unchanged: the same
    /// question asked the new way gets the old answer.
    #[test]
    fn the_castle_answers_the_general_query_the_way_it_answers_the_old_one() {
        let (data, _) = load();
        for z in (-80..=80).step_by(11) {
            for x in (-80..=80).step_by(11) {
                let point = Vec3::new(x as f32, 20.0, z as f32);
                let height = data.floor_height(point);
                let general = data.ground_below(point, Vec3::Y).map(|(at, _)| at.y);
                assert_eq!(height, general, "at {point}");
            }
        }
    }
}
