use bevy::prelude::*;

#[derive(Resource)]
pub struct LevelData {
    pub collision_vertices: Vec<Vec3>,
    pub collision_triangles: Vec<[u32; 3]>,
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
    (
        LevelData {
            collision_vertices,
            collision_triangles,
        },
        RenderLevel { trees },
    )
}

impl LevelData {
    pub fn floor_height(&self, point: Vec3) -> Option<f32> {
        let mut best = None;
        for tri in &self.collision_triangles {
            let a = self.collision_vertices[tri[0] as usize];
            let b = self.collision_vertices[tri[1] as usize];
            let c = self.collision_vertices[tri[2] as usize];
            let v0 = Vec2::new(b.x - a.x, b.z - a.z);
            let v1 = Vec2::new(c.x - a.x, c.z - a.z);
            let v2 = Vec2::new(point.x - a.x, point.z - a.z);
            let den = v0.x * v1.y - v1.x * v0.y;
            if den.abs() < 1e-7 {
                continue;
            }
            let u = (v2.x * v1.y - v1.x * v2.y) / den;
            let v = (v0.x * v2.y - v2.x * v0.y) / den;
            if u >= -0.001 && v >= -0.001 && u + v <= 1.001 {
                let y = a.y + u * (b.y - a.y) + v * (c.y - a.y);
                if y <= point.y + 0.5 && best.map_or(true, |old| y > old) {
                    best = Some(y);
                }
            }
        }
        best
    }
}
