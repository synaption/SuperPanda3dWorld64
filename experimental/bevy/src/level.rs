use bevy::prelude::*;

#[derive(Resource)]
pub struct LevelData {
    pub water_boxes: Vec<WaterBox>,
    triangles: Vec<CollisionTriangle>,
    cells: Vec<CollisionCell>,
    grid_min: Vec2,
    grid_cell: Vec2,
}

const GRID_WIDTH: usize = 16;
const WALL_GRID_MARGIN: f32 = 1.0;
const MAX_MARKED_TRIANGLES: usize = 2048;

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

pub struct RenderLevel {
    pub trees: Vec<Vec3>,
}

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
    let bytes = include_bytes!("../assets/castle.bin");
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
    let trees = r
        .floats(3)
        .into_iter()
        .map(|v| Vec3::new(v[0], v[1], v[2]))
        .collect();
    let water_boxes = r
        .floats(5)
        .into_iter()
        .map(|v| WaterBox {
            min_x: v[0].min(v[2]),
            min_z: v[1].min(v[3]),
            max_x: v[0].max(v[2]),
            max_z: v[1].max(v[3]),
            surface_y: v[4],
        })
        .collect();
    (
        LevelData::new(collision_vertices, collision_triangles, water_boxes),
        RenderLevel { trees },
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
        let mut level = Self {
            water_boxes,
            triangles,
            cells: vec![CollisionCell::default(); GRID_WIDTH * GRID_WIDTH],
            grid_min,
            grid_cell,
        };
        level.build_grid();
        level
    }

    fn build_grid(&mut self) {
        for (index, tri) in self.triangles.iter().enumerate() {
            let all_min = self.cell_coords(tri.min);
            let all_max = self.cell_coords(tri.max);
            for z in all_min.1..=all_max.1 {
                for x in all_min.0..=all_max.0 {
                    self.cells[z * GRID_WIDTH + x].all.push(index);
                    // floor_height historically accepted either winding and
                    // selects by height, so retain that behavior here.
                    if tri.normal.y.abs() > 0.01 {
                        self.cells[z * GRID_WIDTH + x].floors.push(index);
                    }
                }
            }
            if tri.normal.y.abs() <= 0.7 {
                let margin = Vec2::splat(WALL_GRID_MARGIN);
                let wall_min = self.cell_coords(tri.min - margin);
                let wall_max = self.cell_coords(tri.max + margin);
                for z in wall_min.1..=wall_max.1 {
                    for x in wall_min.0..=wall_max.0 {
                        self.cells[z * GRID_WIDTH + x].walls.push(index);
                    }
                }
            }
        }
    }

    fn cell_coords(&self, point: Vec2) -> (usize, usize) {
        let cell = ((point - self.grid_min) / self.grid_cell).floor();
        (
            (cell.x as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
            (cell.y as isize).clamp(0, GRID_WIDTH as isize - 1) as usize,
        )
    }

    fn cell(&self, x: f32, z: f32) -> &CollisionCell {
        let (x, z) = self.cell_coords(Vec2::new(x, z));
        &self.cells[z * GRID_WIDTH + x]
    }

    pub fn water_level(&self, x: f32, z: f32) -> Option<f32> {
        self.water_boxes
            .iter()
            .find(|water| {
                x >= water.min_x && x <= water.max_x && z >= water.min_z && z <= water.max_z
            })
            .map(|water| water.surface_y)
    }
    pub fn floor_height(&self, point: Vec3) -> Option<f32> {
        let mut best = None;
        for &index in &self.cell(point.x, point.z).floors {
            if let Some(y) = surface_y(self.triangles[index], point.x, point.z) {
                if y <= point.y + 0.5 && best.is_none_or(|old| y > old) {
                    best = Some(y);
                }
            }
        }
        best
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

    /// Pushes a vertical player capsule out of steep collision triangles.
    ///
    /// Collision is deliberately independent of Bevy's renderer so movement
    /// can be exercised in headless tests.  The capsule is represented by a
    /// line segment and a radius; testing three spheres along that segment is
    /// sufficient for the castle's triangulated walls and is much cheaper than
    /// bringing a general-purpose physics engine into the port.
    pub fn resolve_walls(&self, position: Vec3, radius: f32, height: f32) -> Vec3 {
        let mut result = position;
        for _ in 0..3 {
            let mut changed = false;
            for &index in &self.cell(result.x, result.z).walls {
                let tri = self.triangles[index];
                let (a, b, c, normal) = (tri.a, tri.b, tri.c, tri.normal);
                for y in [radius, height * 0.5, height - radius] {
                    let center = result + Vec3::Y * y;
                    let closest = closest_point_on_triangle(center, a, b, c);
                    let offset = center - closest;
                    let horizontal = Vec3::new(offset.x, 0.0, offset.z);
                    let distance = horizontal.length();
                    if distance < radius && offset.y.abs() < radius * 1.25 {
                        let direction = if distance > 1e-5 {
                            horizontal / distance
                        } else {
                            Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero()
                        };
                        result += direction * (radius - distance + 0.001);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        result
    }

    /// Returns the first collision point along a segment. Used to prevent the
    /// third-person camera from passing through castle geometry.
    pub fn segment_hit(&self, start: Vec3, end: Vec3) -> Option<Vec3> {
        let direction = end - start;
        let mut nearest = 1.0_f32;
        let mut hit = None;
        if self.triangles.len() > MAX_MARKED_TRIANGLES {
            return self.segment_hit_brute_force(start, end);
        }
        let min = Vec2::new(start.x.min(end.x), start.z.min(end.z));
        let max = Vec2::new(start.x.max(end.x), start.z.max(end.z));
        let cell_min = self.cell_coords(min);
        let cell_max = self.cell_coords(max);
        // The fixed-size mark table avoids allocating a HashSet on every
        // rendered camera frame while still deduplicating triangles spanning
        // multiple cells.
        let mut visited = [false; MAX_MARKED_TRIANGLES];
        for z in cell_min.1..=cell_max.1 {
            for x in cell_min.0..=cell_max.0 {
                for &index in &self.cells[z * GRID_WIDTH + x].all {
                    if visited[index] {
                        continue;
                    }
                    visited[index] = true;
                    let tri = self.triangles[index];
                    if let Some(t) = segment_triangle_time(start, direction, tri) {
                        if t < nearest {
                            nearest = t;
                            hit = Some(start + direction * t);
                        }
                    }
                }
            }
        }
        hit
    }

    fn segment_hit_brute_force(&self, start: Vec3, end: Vec3) -> Option<Vec3> {
        let direction = end - start;
        let mut nearest = 1.0_f32;
        let mut hit = None;
        for &tri in &self.triangles {
            if let Some(t) = segment_triangle_time(start, direction, tri) {
                if t < nearest {
                    nearest = t;
                    hit = Some(start + direction * t);
                }
            }
        }
        hit
    }
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
        let corrected = data.resolve_walls(Vec3::new(0.2, 0., 0.), 0.5, 1.8);
        assert!(corrected.x >= 0.5);
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
        assert_eq!(data.water_level(0.0, 0.0), Some(1.25));
        assert_eq!(data.water_level(5.0, 0.0), None);
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
                data.segment_hit(start, end),
                data.segment_hit_brute_force(start, end)
            );
        }
    }

    #[test]
    fn castle_grid_reduces_floor_candidates() {
        let (data, _) = load();
        let total: usize = data.cells.iter().map(|cell| cell.floors.len()).sum();
        let average = total as f32 / data.cells.len() as f32;
        assert!(
            average < data.triangles.len() as f32 * 0.2,
            "average {average}"
        );
    }
}
