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
//! startup, what the ground under it is, whether anything can stand there, and
//! which of its neighbours have a wall or a fence in the way -- that last one
//! being the difference between a crowd that streams round the castle and a
//! crowd that walks through it. Then a few times a second a breadth-first sweep
//! runs out from whichever cell
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

/// How high off the ground the survey looks for something in the way.
///
/// The same height [`crate::enemy::walk`] probes at, and for the same reason: a
/// kerb the enemy could step up passes under the probe, a fence does not. One
/// idea of what a wall is, shared between the tier that ray-casts for them and
/// the tier that reads them out of a table.
const KNEE: f32 = crate::enemy::STEP_UP;

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

/// How fast word gets around, in cells a second.
///
/// At the castle's cell size this is roughly thirty metres a second, so an
/// alarm raised at one end of the grounds has the far end coming for you in
/// about five seconds. Fast enough to feel like a horde reacting, slow enough
/// that you can watch it happen and run.
const ALARM_SPREAD: f32 = 18.0;

/// Where the alarm stops growing: comfortably past the far corner of any grid,
/// so it saturates rather than counting up forever.
const MAX_ALARM: f32 = (WIDTH * 2) as f32;

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
    /// Which of a cell's eight neighbours have something solid in the way, one
    /// bit each in [`STEPS`] order.
    ///
    /// Ground under both ends of a step is not the same question as a step
    /// being takeable, and leaving out the difference is what let a crowd walk
    /// through the castle's fences. A fence is a thin thing standing *between*
    /// two patches of perfectly good lawn: both cells survey as walkable, both
    /// are at the same height, so every test the field had to offer said yes.
    /// Measured on the castle, 134 of the 23,223 walkable edges have a wall
    /// across them -- few, but they are exactly the edges a player is standing
    /// next to and watching.
    ///
    /// Surveyed once, with the same knee-height cast and the same steepness
    /// threshold the near tier uses, so the two tiers disagree about a fence
    /// only where the grid is too coarse to hold one.
    blocked: Vec<u8>,
    /// Steps from the player's cell, [`FAR`] where the sweep never arrived.
    steps: Vec<u32>,
    /// Which way to walk to get one step closer. Zero where there is nowhere to
    /// go, which is both the player's own cell and every unreachable one.
    flow: Vec<Vec2>,
    /// How far out the alarm has spread, in cells of walking.
    ///
    /// The cheap tier's stand-in for [`crate::enemy::alert`]'s shouting chain.
    /// The chain itself is a spatial grid and a flood fill over the crowd, and
    /// in a field of two thousand packed enemies it reaches essentially all of
    /// them within a single tick -- which is why a fully simulated field ends up
    /// converging on the player entirely. Something has to reproduce that, or
    /// the far crowd stands about while the near crowd charges, and the field
    /// looks like it has lost most of its enemies.
    ///
    /// A radius that grows with time reproduces it for the price of one number.
    /// It is also better to watch: the horde turns and comes for you in a wave
    /// spreading outward rather than all at once.
    alarm: f32,
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
        let mut field = Self {
            origin: low,
            cell,
            ground,
            walkable,
            blocked: vec![0; WIDTH * WIDTH],
            steps: vec![FAR; WIDTH * WIDTH],
            flow: vec![Vec2::ZERO; WIDTH * WIDTH],
            alarm: 0.0,
            due: 0.0,
            swept_from: None,
        };
        field.survey_walls(level);
        field
    }

    /// Records what has something solid standing across it, once the ground is
    /// known.
    ///
    /// Only the second half of [`STEPS`] is walked, and what it finds is written
    /// to both ends. Every undirected edge is therefore surveyed exactly once --
    /// half the casts, and no chance of the two directions across one fence
    /// disagreeing because a ray happened to catch an edge from one side and
    /// miss it from the other.
    fn survey_walls(&mut self, level: &LevelData) {
        for here in 0..WIDTH * WIDTH {
            if !self.walkable[here] {
                continue;
            }
            for step in STEPS.len() / 2..STEPS.len() {
                let Some(there) = neighbour(here, step) else {
                    continue;
                };
                if !self.walkable[there] || !self.wall_between(level, here, there) {
                    continue;
                }
                self.blocked[here] |= 1 << step;
                self.blocked[there] |= 1 << opposite(step);
            }
        }
    }

    /// Is there something between these two cell centres that a walker could not
    /// step over?
    ///
    /// The cast runs between the two cells' own *ground* heights rather than at
    /// a fixed altitude, so it follows a slope up instead of ploughing into it,
    /// and it is raised by [`KNEE`] at both ends so a kerb passes underneath.
    /// The steepness test is [`crate::level::GROUND_NORMAL_Y`] -- the same
    /// threshold the collision grid sorts walls by and the same one
    /// [`crate::enemy::walk`] refuses a step on, because a walker's idea of a
    /// wall and the level's had better be one idea.
    fn wall_between(&self, level: &LevelData, here: usize, there: usize) -> bool {
        let at = |index: usize| {
            let (x, z) = ((index % WIDTH) as f32, (index / WIDTH) as f32);
            let flat = self.origin + Vec2::new(x + 0.5, z + 0.5) * self.cell;
            Vec3::new(flat.x, self.ground[index] + KNEE, flat.y)
        };
        level
            .surface_hit(at(here), at(there))
            .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y)
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

    /// Whether a body standing in `here` may move into the cell `step` away
    /// from it.
    ///
    /// The one rule, in one place. It is asked three times over -- by the sweep
    /// deciding where the crowd *can* go, by the pass that turns step counts
    /// into directions, and by each enemy as it takes an actual step -- and the
    /// three must agree, or the field routes a crowd somewhere its own members
    /// then refuse to walk and the whole stream jams against an invisible line.
    fn passable(&self, here: usize, step: usize, there: usize) -> bool {
        self.walkable[there]
            && (self.ground[there] - self.ground[here]).abs() <= CLIMB
            && self.blocked[here] & (1 << step) == 0
    }

    /// Whether two points fall in the same cell, and so whether the grid has
    /// any opinion at all about the step between them.
    ///
    /// Only [`crate::enemy`]'s end-to-end walk asks, and only so that it can
    /// separate what the field got wrong from what it cannot see.
    #[cfg(test)]
    pub fn same_cell(&self, from: Vec3, to: Vec3) -> bool {
        self.index(from) == self.index(to)
    }

    /// Whether something at `from` may walk to `to`.
    ///
    /// The crowd's whole substitute for collision, and it costs three array
    /// reads. Within one cell there is nothing to decide -- the grid has no
    /// opinion finer than itself -- and across a boundary it is the same
    /// question the sweep asked when it built the route.
    ///
    /// Without this the tier checked only that the far side had *ground*, which
    /// is true on top of the castle wall as well as at the foot of it: an
    /// enemy ambling toward a wander goal would step from the lawn onto the
    /// parapet and then rise to meet it at [`crate::enemy::CLIMB_SPEED`],
    /// which is the "impossibly steep" climb. Nearly a tenth of the castle's
    /// walkable edges -- 2,261 of 23,223 -- are cliffs of that kind.
    pub fn clear(&self, from: Vec3, to: Vec3) -> bool {
        let here = self.index(from);
        let there = self.index(to);
        if here == there {
            return true;
        }
        let (hx, hz) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
        let (tx, tz) = ((there % WIDTH) as isize, (there / WIDTH) as isize);
        match STEPS.iter().position(|&d| d == (tx - hx, tz - hz)) {
            Some(step) => self.passable(here, step, there),
            // Further than a neighbour, which a step at a walking pace across a
            // cell this size is not. Answered on what can be answered rather
            // than waved through.
            None => {
                self.walkable[there]
                    && (self.ground[there] - self.ground[here]).abs() <= CLIMB
            }
        }
    }

    /// Advances the alarm.
    ///
    /// `roused` is whether anything in the world has the player's scent at all.
    /// While it does the alarm spreads outward; when the last thing chasing him
    /// is gone it collapses, so that clearing the field and spawning a fresh one
    /// starts the crowd off calm rather than already at your throat.
    pub fn rouse(&mut self, roused: bool, dt: f32) {
        self.alarm = if roused {
            (self.alarm + ALARM_SPREAD * dt).min(MAX_ALARM)
        } else {
            0.0
        };
    }

    /// Whether the alarm has reached a cell this many steps out.
    pub fn alarmed(&self, steps: u32) -> bool {
        steps as f32 <= self.alarm
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

    field.steps.fill(FAR);

    // A plain queue rather than a priority queue: every step costs one, which
    // makes diagonals slightly cheap and is invisible on a crowd. A real metric
    // would buy nothing here and cost a heap.
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    if field.walkable[from] {
        field.steps[from] = 0;
        queue.push_back(from);
    } else {
        // The player is somewhere the survey called unwalkable -- in the air,
        // in the water, on a ledge between cells. Seed from whatever walkable
        // cells touch his, so the crowd still comes for him.
        for (_, neighbour) in neighbours(from) {
            if field.walkable[neighbour] {
                field.steps[neighbour] = 0;
                queue.push_back(neighbour);
            }
        }
    }
    while let Some(here) = queue.pop_front() {
        let next = field.steps[here] + 1;
        for (step, neighbour) in neighbours(here) {
            if field.steps[neighbour] != FAR || !field.passable(here, step, neighbour) {
                continue;
            }
            field.steps[neighbour] = next;
            queue.push_back(neighbour);
        }
    }

    // And the directions, from the finished distances: point at whichever
    // neighbour is nearest the player.
    for here in 0..WIDTH * WIDTH {
        field.flow[here] = Vec2::ZERO;
        if field.steps[here] == FAR || field.steps[here] == 0 {
            continue;
        }
        let mut best = field.steps[here];
        let mut towards = Vec2::ZERO;
        for (step, neighbour) in neighbours(here) {
            if field.steps[neighbour] >= best {
                continue;
            }
            // The same refusal the sweep made, applied again here. Without it a
            // cell on one side of a fence will happily point at the cell on the
            // other side of it, because that cell is genuinely fewer steps from
            // the player -- it was simply reached the long way round. The sweep
            // never crossed that edge and neither may the flow.
            //
            // There is always something left after this filter: a cell got its
            // step count *from* a neighbour one closer, across an edge that
            // passed the test, and the test is symmetric.
            if !field.passable(here, step, neighbour) {
                continue;
            }
            best = field.steps[neighbour];
            let (hx, hz) = ((here % WIDTH) as f32, (here / WIDTH) as f32);
            let (nx, nz) = ((neighbour % WIDTH) as f32, (neighbour / WIDTH) as f32);
            towards = Vec2::new(nx - hx, nz - hz);
        }
        field.flow[here] = towards.normalize_or_zero();
    }
}

/// The eight directions a body may step, and the order the bits of
/// [`FlowField::blocked`] are in.
///
/// Arranged so that a direction and its opposite are mirrored about the middle
/// of the list, which is what makes [`opposite`] arithmetic rather than a table
/// -- and what lets the wall survey visit each edge once and write both ends.
const STEPS: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// The way back along a step.
fn opposite(step: usize) -> usize {
    STEPS.len() - 1 - step
}

/// The cell `step` away from `here`, or `None` off the edge of the grid --
/// which if left to wrap would send the west of the map walking off the east.
fn neighbour(here: usize, step: usize) -> Option<usize> {
    let (dx, dz) = STEPS[step];
    let (nx, nz) = ((here % WIDTH) as isize + dx, (here / WIDTH) as isize + dz);
    // `then` rather than `then_some`: the latter evaluates its argument whatever
    // the condition says, and off the edge of the grid `nz` is -1, so the index
    // is computed before it is rejected. In a debug build that is an overflow
    // panic. In a release build it is worse -- the arithmetic wraps to some
    // other perfectly valid cell, and the sweep quietly treats the far edge of
    // the map as next door.
    (nx >= 0 && nx < WIDTH as isize && nz >= 0 && nz < WIDTH as isize)
        .then(|| nz as usize * WIDTH + nx as usize)
}

/// The up-to-eight cells touching this one, each with the step that reaches it.
fn neighbours(here: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..STEPS.len()).filter_map(move |step| neighbour(here, step).map(|to| (step, to)))
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
            arrived,
            tested,
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

    /// The castle has fences, and a crowd walking through one is the whole of
    /// what the wall survey is for.
    ///
    /// Before it, 35 of the field's 4,390 routes crossed something the near
    /// tier would have been stopped by -- few in the count, but each one is a
    /// stream of goombas walking through a fence somebody is standing next to.
    #[test]
    fn the_flow_never_routes_through_a_wall() {
        let (level, _) = crate::level::load();
        let field = swept(Vec3::new(-13.28, 3.0, 46.64));
        let mut routes = 0;
        let mut through = Vec::new();
        for index in 0..WIDTH * WIDTH {
            let towards = field.flow[index];
            if towards == Vec2::ZERO {
                continue;
            }
            routes += 1;
            let (x, z) = ((index % WIDTH) as isize, (index / WIDTH) as isize);
            let next = ((z + towards.y.round() as isize) * WIDTH as isize
                + (x + towards.x.round() as isize)) as usize;
            if field.wall_between(&level, index, next) {
                through.push((index, next));
            }
        }
        // Still a field: refusing those edges must not have cut the map in two.
        assert!(routes > 4_000, "only {routes} cells lead anywhere at all");
        assert!(
            through.is_empty(),
            "{} of {routes} routes cross a wall, e.g. {:?}",
            through.len(),
            &through[..through.len().min(4)]
        );
    }

    /// And what an enemy is refused when it takes the step itself, which is a
    /// separate question from what the sweep routed it along: an enemy weaving
    /// across its route, or ambling to a wander goal with no route at all, walks
    /// over edges the flow never chose.
    ///
    /// Both directions are asserted. A rule that refuses everything would pass
    /// the half of this that matters and strand the crowd where it stood.
    #[test]
    fn a_step_over_a_cliff_or_through_a_fence_is_refused() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let centre = |index: usize| {
            let (x, z) = ((index % WIDTH) as f32, (index / WIDTH) as f32);
            let flat = field.origin + Vec2::new(x + 0.5, z + 0.5) * field.cell;
            Vec3::new(flat.x, field.ground[index], flat.y)
        };
        let (mut refused, mut allowed) = (0, 0);
        for here in 0..WIDTH * WIDTH {
            if !field.walkable[here] {
                continue;
            }
            for (_, there) in neighbours(here) {
                if !field.walkable[there] || there < here {
                    continue;
                }
                let cliff = (field.ground[there] - field.ground[here]).abs() > CLIMB;
                let wall = field.wall_between(&level, here, there);
                let clear = field.clear(centre(here), centre(there));
                if cliff || wall {
                    refused += 1;
                    assert!(
                        !clear,
                        "cell {here} may step to {there} across a {} of {:.2} m",
                        if wall { "fence" } else { "cliff" },
                        (field.ground[there] - field.ground[here]).abs()
                    );
                } else {
                    allowed += 1;
                    assert!(clear, "cell {here} may not step to open ground at {there}");
                }
            }
        }
        assert!(refused > 1_000, "only {refused} edges were refused");
        assert!(allowed > 10_000, "only {allowed} edges were walkable");
    }

    /// Stepping about inside one cell is nobody's business: the grid has no
    /// opinion finer than itself, and pretending otherwise would have the crowd
    /// stopped by rounding.
    #[test]
    fn a_step_that_stays_in_its_cell_is_always_clear() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let at = Vec3::new(-13.28, 3.0, 46.64);
        assert!(field.clear(at, at + Vec3::new(0.01, 0.0, 0.01)));
    }

    /// How long the one-off survey takes, since it now casts as well as probes.
    #[test]
    #[ignore]
    fn bench_survey() {
        let (level, _) = crate::level::load();
        let start = std::time::Instant::now();
        let field = FlowField::new(&level);
        let took = start.elapsed();
        let barred: usize = field.blocked.iter().map(|bits| bits.count_ones() as usize).sum();
        println!("survey took {took:?}; {barred} blocked edge-ends, {} cells touch one",
            field.blocked.iter().filter(|bits| **bits != 0).count());
    }
}
