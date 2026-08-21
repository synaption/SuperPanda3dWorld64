//! How a crowd knows which way the player is, without any of it asking.
//!
//! The expensive part of an enemy is not drawing it, it is the questions it
//! asks the level: where is the floor here, is there a wall in the way, what is
//! under my feet after that step. Those are grid lookups against the collision
//! mesh, several per enemy per tick, and two thousand enemies asking them is a
//! simulation that costs more than the frame it is in.
//!
//! A flow field answers all of them once, for everybody, in advance.
//!
//! The castle is divided into square cells. Each cell is asked *once*, at
//! startup, what the ground under it is and whether anything can stand there.
//! Then a few times a second a breadth-first sweep runs out from whichever cell
//! the player is standing in, and every cell records how many steps it is from
//! him and which way its neighbour with the shorter walk lies.
//!
//! An enemy reading that does no work at all: one array index gives it the
//! direction to walk and the height to walk at. No ray casts, no floor queries,
//! no neighbours to consider. That is what makes a crowd of thousands affordable
//! -- and it is *better* behaviour rather than merely cheaper, because a field
//! built by walking outwards over connected ground flows round the moat and up
//! the ramps instead of marching into the water.
//!
//! What it deliberately does not do is push enemies apart or stop them
//! overlapping. Something that far away is a few pixels tall and the crowd
//! reads as a crowd; [`crate::enemy::spread`] keeps its work for the units near
//! enough for it to matter.

use crate::level::LevelData;
use bevy::prelude::*;

/// Cells along each side of the field.
///
/// 96 puts the castle's cells at roughly 1.7 m, which is about an enemy's own
/// width -- fine enough that a crowd streams round a wall rather than through
/// its corner, and coarse enough that a whole sweep is nine thousand cells and
/// costs well under a millisecond.
const WIDTH: usize = 96;

/// How far a step may climb or drop before it stops being a step.
///
/// The same idea as [`crate::enemy::STEP_UP`] but measured between cell
/// centres, which are further apart than a walking step: this is what makes the
/// field refuse to route a crowd off the castle wall or up a cliff. Without it
/// the sweep happily walks up sheer faces and the flow points a thousand
/// goombas at a wall.
const CLIMB: f32 = 1.2;

/// How often the sweep is rerun, in seconds.
///
/// The field only changes when the player moves, and a crowd walking a
/// quarter-second-old route is a crowd that looks exactly like a crowd. Ten
/// times a second is far more than the eye asks for and still nothing next to
/// what it replaces.
const REBUILD: f32 = 0.1;

/// The height a cell's ground query is asked from, above everything in the map.
const SKY: f32 = 90.0;

/// Unreachable, or not yet reached by the sweep.
const FAR: u32 = u32::MAX;

/// The navigation grid, and the last sweep run over it.
#[derive(Resource)]
pub struct FlowField {
    /// World position of the low corner of cell `(0, 0)`.
    origin: Vec2,
    /// Metres along one side of a cell.
    cell: f32,
    /// Ground height per cell, and whether there is any.
    ground: Vec<f32>,
    walkable: Vec<bool>,
    /// Steps from the player's cell, [`FAR`] where the sweep never arrived.
    steps: Vec<u32>,
    /// Which way to walk to get one step closer. Zero where there is nowhere to
    /// go, which is both the player's own cell and every unreachable one.
    flow: Vec<Vec2>,
    /// Seconds until the next sweep.
    due: f32,
    /// Where the player was when the last sweep ran, so a stationary player
    /// does not pay for a sweep that would produce the same field.
    swept_from: Option<usize>,
}

/// What a cell knows, handed to an enemy standing in it.
#[derive(Clone, Copy, Debug)]
pub struct Guidance {
    /// Which way the player is, as a unit vector in the horizontal plane, or
    /// zero if there is no route.
    pub towards: Vec2,
    /// The ground height here.
    pub ground: f32,
    /// How many cells of walking away the player is, or `None` if unreachable.
    pub steps: Option<u32>,
    /// Whether the survey found anything to stand on here. The crowd checks it
    /// before stepping, which is its whole substitute for collision.
    pub walkable: bool,
}

impl FlowField {
    /// Surveys the level once. Every floor query the crowd will ever need is
    /// asked here, and none of them again.
    pub fn new(level: &LevelData) -> Self {
        let (low, high) = level.bounds();
        let span = high - low;
        let cell = (span.x.max(span.y) / WIDTH as f32).max(0.001);
        let mut ground = vec![0.0; WIDTH * WIDTH];
        let mut walkable = vec![false; WIDTH * WIDTH];
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                let at = low + Vec2::new(x as f32 + 0.5, z as f32 + 0.5) * cell;
                // `ground_at` rather than `floor_height`: a crowd should be
                // routed over things it could actually stand on, and the floor
                // query happily answers with the side of a wall.
                if let Some((height, _)) = level.ground_at(Vec3::new(at.x, SKY, at.y)) {
                    ground[z * WIDTH + x] = height;
                    walkable[z * WIDTH + x] = true;
                }
            }
        }
        Self {
            origin: low,
            cell,
            ground,
            walkable,
            steps: vec![FAR; WIDTH * WIDTH],
            flow: vec![Vec2::ZERO; WIDTH * WIDTH],
            due: 0.0,
            swept_from: None,
        }
    }

    /// The cell a world position falls in, clamped to the grid.
    fn index(&self, at: Vec3) -> usize {
        let local = (Vec2::new(at.x, at.z) - self.origin) / self.cell;
        let x = (local.x as isize).clamp(0, WIDTH as isize - 1) as usize;
        let z = (local.y as isize).clamp(0, WIDTH as isize - 1) as usize;
        z * WIDTH + x
    }

    /// What the crowd standing at `at` should do.
    ///
    /// Four array lookups and a pair of lerps. This is the whole of what a
    /// distant enemy costs, against the several collision-grid queries the near
    /// tier spends.
    ///
    /// The direction and the step count are taken from the cell the point falls
    /// in, because both are decisions and a decision does not want smoothing.
    /// The **height is interpolated**, and that is not a nicety. Taking the
    /// cell's own height puts every enemy in a cell at the height of that
    /// cell's centre, so on a slope half of them stand buried and the other
    /// half float, by up to half the drop across a cell. Measured on the
    /// castle that was 0.46 m of average error against a goomba 0.9 m tall --
    /// enemies sinking into hillsides and popping back out, which reads exactly
    /// like the crowd flickering in and out of existence.
    pub fn at(&self, at: Vec3) -> Guidance {
        let index = self.index(at);
        Guidance {
            towards: self.flow[index],
            ground: self.height(at),
            steps: match self.steps[index] {
                FAR => None,
                steps => Some(steps),
            },
            walkable: self.walkable[index],
        }
    }

    /// The ground height at a point, interpolated between the four cell centres
    /// around it.
    ///
    /// Unwalkable cells are left out of the average rather than counted as
    /// zero, and their weight is redistributed over the neighbours that do have
    /// ground. Without that, standing near the edge of the lawn would average
    /// the lawn against a cell holding nothing and drag the whole crowd down
    /// into the ground as it approached any boundary.
    fn height(&self, at: Vec3) -> f32 {
        // Cell centres sit half a cell in, so shifting by half puts the point
        // into a space where the centres are at the integers.
        let local = (Vec2::new(at.x, at.z) - self.origin) / self.cell - Vec2::splat(0.5);
        let base = local.floor();
        let frac = local - base;
        let mut total = 0.0;
        let mut weight = 0.0;
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let x = base.x as isize + dx;
            let z = base.y as isize + dz;
            if x < 0 || z < 0 || x >= WIDTH as isize || z >= WIDTH as isize {
                continue;
            }
            let index = z as usize * WIDTH + x as usize;
            if !self.walkable[index] {
                continue;
            }
            let wx = if dx == 0 { 1.0 - frac.x } else { frac.x };
            let wz = if dz == 0 { 1.0 - frac.y } else { frac.y };
            total += self.ground[index] * wx * wz;
            weight += wx * wz;
        }
        if weight > 1e-6 {
            total / weight
        } else {
            // Nothing walkable anywhere near: fall back to the cell itself,
            // which is what the caller's `walkable` flag is about to refuse
            // anyway.
            self.ground[self.index(at)]
        }
    }

    /// How far a cell's worth of walking is, so callers can talk in metres
    /// rather than in steps.
    pub fn cell_size(&self) -> f32 {
        self.cell
    }

}

/// Reruns the sweep when it is due and the player has moved to a new cell.
///
/// Breadth-first over the eight neighbours, refusing any step that climbs or
/// drops more than [`CLIMB`]. That refusal is the whole of the pathing: a field
/// swept over connected ground cannot route anybody through a wall, because the
/// sweep never got there.
pub fn rebuild(
    time: Res<Time>,
    mut field: ResMut<FlowField>,
    player: Query<&Transform, With<crate::player::Player>>,
) {
    field.due -= time.delta_secs();
    if field.due > 0.0 {
        return;
    }
    let Ok(player) = player.single() else {
        return;
    };
    field.due = REBUILD;
    let from = field.index(player.translation);
    // A player who has not left his cell would get the same field back.
    if field.swept_from == Some(from) {
        return;
    }
    field.swept_from = Some(from);

    let FlowField {
        ground,
        walkable,
        steps,
        flow,
        ..
    } = &mut *field;
    steps.fill(FAR);

    // A plain queue rather than a priority queue: every step costs one, which
    // makes diagonals slightly cheap and is invisible on a crowd. A real metric
    // would buy nothing here and cost a heap.
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    if walkable[from] {
        steps[from] = 0;
        queue.push_back(from);
    } else {
        // The player is somewhere the survey called unwalkable -- in the air,
        // in the water, on a ledge between cells. Seed from whatever walkable
        // cells touch his, so the crowd still comes for him.
        for neighbour in neighbours(from) {
            if walkable[neighbour] {
                steps[neighbour] = 0;
                queue.push_back(neighbour);
            }
        }
    }
    while let Some(here) = queue.pop_front() {
        let next = steps[here] + 1;
        for neighbour in neighbours(here) {
            if !walkable[neighbour] || steps[neighbour] != FAR {
                continue;
            }
            if (ground[neighbour] - ground[here]).abs() > CLIMB {
                continue;
            }
            steps[neighbour] = next;
            queue.push_back(neighbour);
        }
    }

    // And the directions, from the finished distances: point at whichever
    // neighbour is nearest the player.
    for here in 0..WIDTH * WIDTH {
        flow[here] = Vec2::ZERO;
        if steps[here] == FAR || steps[here] == 0 {
            continue;
        }
        let mut best = steps[here];
        let mut towards = Vec2::ZERO;
        for neighbour in neighbours(here) {
            if steps[neighbour] >= best {
                continue;
            }
            // The same refusal the sweep made, applied again here. Without it a
            // cell at the foot of a wall will happily point at the top of that
            // wall, because the top is genuinely fewer steps from the player --
            // it was simply reached the long way round. The sweep never crossed
            // that edge and neither may the flow.
            //
            // There is always something left after this filter: a cell got its
            // step count *from* a neighbour one closer, across an edge that
            // passed the test, and the test is symmetric.
            if (ground[neighbour] - ground[here]).abs() > CLIMB {
                continue;
            }
            best = steps[neighbour];
            let (hx, hz) = ((here % WIDTH) as f32, (here / WIDTH) as f32);
            let (nx, nz) = ((neighbour % WIDTH) as f32, (neighbour / WIDTH) as f32);
            towards = Vec2::new(nx - hx, nz - hz);
        }
        flow[here] = towards.normalize_or_zero();
    }
}

/// The up-to-eight cells touching this one, without wrapping round the edge of
/// the grid -- which would send the west of the map walking off the east.
fn neighbours(here: usize) -> impl Iterator<Item = usize> {
    let (x, z) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
    [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ]
    .into_iter()
    .filter_map(move |(dx, dz)| {
        let (nx, nz) = (x + dx, z + dz);
        // `then` rather than `then_some`: the latter evaluates its argument
        // whatever the condition says, and off the edge of the grid `nz` is -1,
        // so the index is computed before it is rejected. In a debug build that
        // is an overflow panic. In a release build it is worse -- the
        // arithmetic wraps to some other perfectly valid cell, and the sweep
        // quietly treats the far edge of the map as next door.
        (nx >= 0 && nx < WIDTH as isize && nz >= 0 && nz < WIDTH as isize)
            .then(|| nz as usize * WIDTH + nx as usize)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The survey has to find real ground over the castle, or every enemy the
    /// field guides walks at a height of zero.
    #[test]
    fn the_survey_finds_the_castle_grounds() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let found = field.walkable.iter().filter(|walkable| **walkable).count();
        assert!(
            found > WIDTH * WIDTH / 8,
            "only {found} of {} cells found any ground",
            WIDTH * WIDTH
        );
        // And the heights it found are the ones the collision reports.
        for (index, walkable) in field.walkable.iter().enumerate() {
            if !walkable {
                continue;
            }
            let (x, z) = ((index % WIDTH) as f32, (index / WIDTH) as f32);
            let at = field.origin + Vec2::new(x + 0.5, z + 0.5) * field.cell;
            let asked = level
                .ground_at(Vec3::new(at.x, SKY, at.y))
                .expect("a cell the survey called walkable has no ground")
                .0;
            assert!((asked - field.ground[index]).abs() < 1e-4);
        }
    }

    /// Builds a world holding the real castle and a player at `at`, sweeps it,
    /// and hands the field back.
    fn swept(at: Vec3) -> FlowField {
        let (level, _) = crate::level::load();
        let mut world = World::new();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(Time::<()>::default());
        world.spawn((crate::player::Player, Transform::from_translation(at)));
        world.run_system_once(rebuild).expect("the sweep failed");
        world.remove_resource::<FlowField>().unwrap()
    }

    /// The whole point: standing anywhere on the lawn, the flow walks you to
    /// the player rather than into the scenery.
    #[test]
    fn following_the_flow_arrives_at_the_player() {
        let player = Vec3::new(-13.28, 3.0, 46.64);
        let field = swept(player);
        let mut tested = 0;
        let mut arrived = 0;
        for index in 0..WIDTH * WIDTH {
            if field.steps[index] == FAR || field.steps[index] == 0 {
                continue;
            }
            tested += 1;
            // Walk the field cell by cell. A field with a loop or a dead end in
            // it fails here rather than in a build somebody is playing.
            let mut here = index;
            let mut hops = 0;
            while field.steps[here] > 0 && hops < WIDTH * 4 {
                let towards = field.flow[here];
                if towards == Vec2::ZERO {
                    break;
                }
                let (x, z) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
                let next = (z + towards.y.round() as isize) * WIDTH as isize
                    + (x + towards.x.round() as isize);
                here = next as usize;
                hops += 1;
            }
            if field.steps[here] == 0 {
                arrived += 1;
            }
        }
        assert!(tested > 500, "only {tested} cells were reachable at all");
        assert_eq!(
            arrived, tested,
            "{} of {tested} cells lead somewhere other than the player",
            tested - arrived
        );
    }

    /// The flow points *downhill* in step count everywhere, which is the
    /// property that makes following it terminate.
    #[test]
    fn every_step_of_the_flow_gets_closer() {
        let field = swept(Vec3::new(-13.28, 3.0, 46.64));
        for index in 0..WIDTH * WIDTH {
            let towards = field.flow[index];
            if towards == Vec2::ZERO {
                continue;
            }
            let (x, z) = ((index % WIDTH) as isize, (index / WIDTH) as isize);
            let next = ((z + towards.y.round() as isize) * WIDTH as isize
                + (x + towards.x.round() as isize)) as usize;
            assert!(
                field.steps[next] < field.steps[index],
                "cell {index} at {} steps points at {next} at {} steps",
                field.steps[index],
                field.steps[next]
            );
        }
    }

    /// A route the sweep produces never climbs a cliff, because it never
    /// crossed one. This is what keeps a crowd out of the moat and off the
    /// castle walls.
    #[test]
    fn the_flow_never_routes_over_a_cliff() {
        let field = swept(Vec3::new(-13.28, 3.0, 46.64));
        for index in 0..WIDTH * WIDTH {
            let towards = field.flow[index];
            if towards == Vec2::ZERO {
                continue;
            }
            let (x, z) = ((index % WIDTH) as isize, (index / WIDTH) as isize);
            let next = ((z + towards.y.round() as isize) * WIDTH as isize
                + (x + towards.x.round() as isize)) as usize;
            let rise = (field.ground[next] - field.ground[index]).abs();
            assert!(rise <= CLIMB, "a step of {rise} metres at cell {index}");
        }
    }

    /// Moving the player moves the target: two sweeps from different places
    /// must not produce the same field.
    #[test]
    fn the_field_follows_the_player() {
        let near = swept(Vec3::new(-13.28, 3.0, 46.64));
        let far = swept(Vec3::new(40.0, 3.0, -30.0));
        let differing = (0..WIDTH * WIDTH)
            .filter(|index| near.flow[*index] != far.flow[*index])
            .count();
        assert!(
            differing > WIDTH * WIDTH / 20,
            "moving the player across the map changed only {differing} cells"
        );
    }
}
