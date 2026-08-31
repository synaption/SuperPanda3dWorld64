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
pub const WIDTH: usize = 96;

/// How far a step may climb or drop before it stops being a step.
///
/// The same idea as [`crate::enemy::STEP_UP`] but measured between cell
/// centres, which are further apart than a walking step: this is what makes the
/// field refuse to route a crowd off the castle wall or up a cliff. Without it
/// the sweep happily walks up sheer faces and the flow points a thousand
/// slimes at a wall.
///
/// Public because it is also the line that decides who this grid is any use
/// to: a body that can climb more than this is a body every answer here is
/// wrong for. See [`crate::enemy::navigate`].
pub const CLIMB: f32 = 1.2;

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

/// Unreachable, or not yet reached by the sweep. The sweep is
/// [`crate::route::flood`] and this is its own name for the same value, so the
/// grid can be read without the graph module in hand.
const FAR: u32 = crate::route::UNREACHED;

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

/// One end of a way across the map that is not a step.
///
/// A portal, in every case that exists today, and stated here in terms of the
/// grid rather than in terms of portals: the field's business is that two cells
/// which are nowhere near each other are one move apart, and it has no opinion
/// about what put them that way. See [`crate::portal`], and
/// [`FlowField::set_warp`] for what the grid does with a pair of these.
#[derive(Clone, Copy, Debug)]
pub struct Warp {
    /// Where a body stands to use it: a spot on real ground, a stride out from
    /// the opening, which is what the survey has to be able to vouch for.
    pub stand: Vec3,
    /// The point a body actually walks at, which is a little way *through* the
    /// opening rather than in front of it.
    ///
    /// The two are different on purpose and the difference is the whole of
    /// whether a route works. `stand` is a place on the grid, so it is what the
    /// search reasons about; walking to it and stopping is a body standing in
    /// front of a portal admiring it. The last leg has to be somewhere on the
    /// far side of the plane, or nothing ever crosses.
    pub mouth: Vec3,
}

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
    /// Whether a body standing on this cell's ground would be out of its depth.
    ///
    /// Surveyed once beside the ground, from the same query [`crate::squad`]
    /// asks per step, and it exists for [`FlowField::route`]: a route that
    /// prices the moat goes round it, and a route that cannot see the moat at
    /// all is a route that sends the squad swimming across the middle of the
    /// map because that happened to be the straight line. The crowd tier does
    /// not read it -- a slime is welcome in the water -- so it costs one query
    /// per cell at load and nothing afterwards.
    wet: Vec<bool>,
    /// How many cells of daylight there are between this one and the nearest
    /// thing a body could scrape against, capped at [`ROOM_CAP`].
    ///
    /// **This is what stops a route running with its shoulder along a fence.**
    /// A* over cells with no idea of room takes the cheapest chain there is,
    /// and the cheapest chain round a corner clips it as tightly as the grid
    /// allows -- so a squad routed past the moat railings walks *touching* the
    /// railings, in single file, because there is exactly one cheapest line and
    /// they all have it. Priced by [`Tolls::hug`], the same route bends a cell
    /// or two out into the open, which costs a stride and leaves room for
    /// twelve bodies to walk abreast.
    ///
    /// Surveyed once, by the same breadth-first walk [`crate::route::flood`]
    /// sweeps everything else in this game with -- sourced at every cell that
    /// touches something impassable, spreading outward over the ones that do
    /// not.
    clearance: Vec<u8>,
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
    /// The ways across that are not steps, each end once. Empty almost always;
    /// two entries while a pair of portals is open. See [`Self::set_warp`].
    warps: Vec<Link>,
}

/// One end of a warp, as the grid holds it: which cell it is used from, which
/// cell it lands in, and the two points a route puts on either side of the
/// crossing.
#[derive(Clone, Copy, Debug)]
struct Link {
    cell: usize,
    exit: usize,
    /// The point through the opening, which is the leg a body walks at.
    mouth: Vec3,
    /// Where a body walks *to* on this side, for tautening the run up to it.
    stand: Vec3,
    /// Where it comes out, which is where the next stretch of the route is
    /// pulled taut from.
    landing: Vec3,
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

/// What the survey knows about one cell, for anything drawing the grid.
#[derive(Clone, Copy, Debug)]
pub struct Survey {
    pub walkable: bool,
    pub wet: bool,
    /// Cells of daylight to the nearest thing a body could scrape, capped at
    /// [`ROOM_CAP`]. Nought means this cell touches something.
    pub room: u8,
    /// Which of the eight neighbours have something solid in the way, one bit
    /// each in the order [`FlowField::step_offset`] counts in.
    pub blocked: u8,
    /// Cells of walking from the player, or `None` where the sweep never got.
    pub steps: Option<u32>,
}

/// What a route is willing to pay to keep away from things, in metres of
/// detour.
///
/// Both are *prices* rather than rules, which is the same shape every other
/// preference in this game takes: a Mario given nowhere dry still swims, and a
/// Mario given nowhere roomy still squeezes through the gate. What the numbers
/// buy is which way it goes when it has a choice.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tolls {
    /// What a cell of deep water is worth going round. Scaled per body by
    /// [`crate::goap::Goal::caution`], so an order wades where an errand walks
    /// to the bridge.
    pub wet: f32,
    /// What a cell pressed up against a wall is worth going round.
    ///
    /// Small on purpose. This is not "avoid walls" -- a route that would not go
    /// near one could not go through a gate -- it is "do not walk along one
    /// when there is open ground a stride to the left". At the castle's cell
    /// size, stepping out one cell and back in costs about a metre and a half
    /// of extra diagonal, so a toll of a metre pays for itself after two cells
    /// of wall and never buys a detour worth noticing.
    pub hug: f32,
}

/// How much room a cell can be counted as having, in cells.
///
/// Two is the whole of what a route needs to tell apart: touching something,
/// one clear of it, or out in the open. Beyond that the price is zero and
/// counting further would only make [`Tolls::hug`] harder to reason about.
const ROOM_CAP: u8 = 2;

/// A route, and what finding it cost.
#[derive(Clone, Debug)]
pub struct Routed {
    /// Points to walk at in turn, the destination last. Never empty.
    pub legs: Vec<Vec3>,
    /// How many cells the search settled to find it.
    pub settled: usize,
    /// Whether it stops short of where it was asked to go. See
    /// [`crate::route::Found::partial`].
    pub partial: bool,
    /// Whether it stops short because there is no way there at all rather than
    /// because the search ran out of budget. See
    /// [`crate::route::Found::exhausted`].
    pub exhausted: bool,
}

/// How far out [`FlowField::nearest_walkable`] looks for standable ground
/// before it gives up, in cells.
///
/// Seven is about twelve metres on the castle's grid: enough to find the bank
/// from the middle of the moat, or the lawn from inside the castle's own
/// footprint, and short enough that asking for a route to the far side of the
/// sea is refused rather than answered with somewhere else entirely.
const SNAP_RINGS: usize = 7;

impl FlowField {
    /// Surveys the level once. Every floor query the crowd will ever need is
    /// asked here, and none of them again.
    pub fn new(level: &LevelData) -> Self {
        let (low, high) = level.bounds();
        let span = high - low;
        let cell = (span.x.max(span.y) / WIDTH as f32).max(0.001);
        let mut ground = vec![0.0; WIDTH * WIDTH];
        let mut walkable = vec![false; WIDTH * WIDTH];
        let mut wet = vec![false; WIDTH * WIDTH];
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                let at = low + Vec2::new(x as f32 + 0.5, z as f32 + 0.5) * cell;
                // `ground_at` rather than `floor_height`: a crowd should be
                // routed over things it could actually stand on, and the floor
                // query happily answers with the side of a wall.
                if let Some((height, _)) = level.ground_at(Vec3::new(at.x, SKY, at.y)) {
                    ground[z * WIDTH + x] = height;
                    walkable[z * WIDTH + x] = true;
                    // Measured at the ground rather than at the sky the query
                    // was asked from, which is the whole of the difference
                    // between the bed of the moat and the air above it.
                    wet[z * WIDTH + x] = level
                        .water_depth(Vec3::new(at.x, height, at.y))
                        .is_some_and(|depth| depth > crate::squad::SWIMMING_DEPTH);
                }
            }
        }
        let mut field = Self {
            origin: low,
            cell,
            ground,
            walkable,
            wet,
            clearance: vec![0; WIDTH * WIDTH],
            blocked: vec![0; WIDTH * WIDTH],
            steps: vec![FAR; WIDTH * WIDTH],
            flow: vec![Vec2::ZERO; WIDTH * WIDTH],
            alarm: 0.0,
            due: 0.0,
            swept_from: None,
            warps: Vec::new(),
        };
        field.survey_walls(level);
        // After the walls, and not before: what counts as being up against
        // something is partly which edges have a fence across them, and that is
        // what the pass above has just worked out.
        field.survey_room();
        field
    }

    /// Works out how much room each cell has, once the walls are known.
    ///
    /// A cell is up against something if any of the eight directions out of it
    /// is a step it could not take -- off the grid, onto ground nothing can
    /// stand on, or through a fence. Those are the sources, at nought; the
    /// sweep spreads out from them over everything else, so a cell's number is
    /// how many cells of walking it is from the nearest edge of the walkable
    /// world.
    ///
    /// Capped at [`ROOM_CAP`] because the middle of a lawn is the middle of a
    /// lawn: what a route needs to know is "up against it", "one clear" or
    /// "plenty", and counting to fifty across an open field would only make the
    /// number harder to price.
    fn survey_room(&mut self) {
        let against: Vec<usize> = (0..WIDTH * WIDTH)
            .filter(|&here| {
                self.walkable[here]
                    && (0..STEPS.len()).any(|step| match neighbour(here, step) {
                        None => true,
                        Some(there) => !self.passable(here, step, there),
                    })
            })
            .collect();
        let swept = {
            let grid = &*self;
            crate::route::flood(WIDTH * WIDTH, against, |here| {
                neighbours(here)
                    .filter(move |&(step, there)| grid.passable(here, step, there))
                    .map(|(_, there)| there)
            })
        };
        for here in 0..WIDTH * WIDTH {
            self.clearance[here] = match swept.steps(here) {
                Some(steps) => steps.min(ROOM_CAP as u32) as u8,
                // Never reached, which for a walkable cell means an island with
                // no edge at all -- there is nothing to scrape against on it.
                None => ROOM_CAP,
            };
        }
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
        let (ground_here, ground_there) = (at(here), at(there));
        // **The ground between them, before anything is cast at all.** A cell
        // boundary can hold a knee-high lip -- the bottom of a bank, the edge of
        // a path -- that is too steep to *be* ground and too tall to step over,
        // and neither end of the edge knows anything about it: both cells are
        // good lawn, the rise between their middles is well inside [`CLIMB`],
        // and a ray at knee height following the ground sails over the top of
        // it. A body meets it head on, `resolve_walls` refuses, and it slides
        // along the lip for the rest of the session with a perfectly good route
        // in hand. Uphill only, because falling off one is not climbing it.
        //
        // Ties broken on the cell number rather than left to the argument
        // order, so that asking about an edge from one end and from the other
        // gives one answer. Two cells at exactly the same height are common --
        // any two on a flat lawn -- and a march that started from a different
        // end of the same pair would sample different points along it and could
        // disagree about a lip in the middle.
        let uphill = (ground_here.y, here) <= (ground_there.y, there);
        let (low, high) = match uphill {
            true => (ground_here, ground_there),
            false => (ground_there, ground_here),
        };
        if !level.climbable(low - Vec3::Y * KNEE, high - Vec3::Y * KNEE) {
            return true;
        }
        let (from, to) = (ground_here, ground_there);
        // **A body's width, not a line.** One ray down the middle of a step
        // answers whether a *point* could take it, and nothing in this game is
        // a point: a Mario is held a radius clear of every wall by
        // `LevelData::resolve_walls`, so a gap the centre line slips through
        // between two fence posts is a gap it cannot walk. On this castle 325
        // of the 23,223 walkable edges are like that -- seventy per cent again
        // on top of the 468 a single ray finds -- and every one of them is a
        // place the search will happily route somebody straight into a fence
        // and leave them pressed against it.
        //
        // Three rays a radius apart rather than a swept capsule, because the
        // thing being caught is a post or a jamb standing *in* the gap, and
        // something a body's width across cannot hide between three lines that
        // span a body's width. Paid once, at load, on an edge that also gets a
        // floor query -- see the survey timing in `bench_survey`.
        let along = (to - from).normalize_or_zero();
        let across = Vec3::new(-along.z, 0.0, along.x) * crate::player::PLAYER_RADIUS;
        [Vec3::ZERO, across, -across].iter().any(|offset| {
            level
                .surface_hit(from + *offset, to + *offset)
                .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y)
        })
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
    /// castle that was 0.46 m of average error against a slime 0.7 m tall --
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

    /// The world position of a cell's centre, at the ground the survey found
    /// there.
    pub fn centre_of(&self, cell: usize) -> Vec3 {
        let (x, z) = ((cell % WIDTH) as f32, (cell / WIDTH) as f32);
        let flat = self.origin + Vec2::new(x + 0.5, z + 0.5) * self.cell;
        Vec3::new(flat.x, self.ground[cell.min(self.ground.len() - 1)], flat.y)
    }

    /// The cell a world position falls in, clamped to the grid.
    pub fn cell_at(&self, at: Vec3) -> usize {
        self.index(at)
    }

    /// Everything the survey knows about one cell, for anything drawing the
    /// grid rather than walking it. See [`crate::path::draw`].
    pub fn survey_of(&self, cell: usize) -> Survey {
        Survey {
            walkable: self.walkable[cell],
            wet: self.wet[cell],
            room: self.clearance[cell],
            blocked: self.blocked[cell],
            steps: match self.steps[cell] {
                FAR => None,
                steps => Some(steps),
            },
        }
    }

    /// The nearest spot to `at` something could actually stand on, or `None`
    /// when there is nothing standable anywhere near it.
    ///
    /// The public face of [`Self::nearest_walkable`], for anything *placing* a
    /// body rather than routing one. [`crate::squad::Squad::send`] lays a
    /// cluster of spots out with it, and the difference between a spot the
    /// survey will vouch for and one it will not is the difference between an
    /// order a Mario can carry out and an order to go and stand in mid-air.
    pub fn standable(&self, at: Vec3) -> Option<Vec3> {
        self.nearest_walkable(at).map(|cell| self.centre_of(cell))
    }

    /// The nearest cell to `at` that something could actually stand on.
    ///
    /// Wanted at both ends of a route and for the same reason: the thing asking
    /// is very often *not* standing on the grid's idea of ground. A Mario in
    /// the moat, a ball on a ledge between two cells, a slot in a cluster that
    /// landed inside the castle's footprint -- each of them indexes to a cell
    /// the sweep will not enter, and a route from or to one of those is a route
    /// that fails for a reason the player would call "it is right there".
    ///
    /// Rings outward rather than sweeping the grid, so the common answer -- the
    /// cell itself -- costs one array read and the uncommon one costs the few
    /// cells actually nearby. `None` past [`SNAP_RINGS`], which over this grid
    /// is a body a dozen metres from anything walkable and is genuinely
    /// somewhere a route cannot start.
    fn nearest_walkable(&self, at: Vec3) -> Option<usize> {
        let here = self.index(at);
        if self.walkable[here] {
            return Some(here);
        }
        let (cx, cz) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
        for ring in 1..=SNAP_RINGS as isize {
            let mut best: Option<(f32, usize)> = None;
            for dz in -ring..=ring {
                for dx in -ring..=ring {
                    // Only the rim of the square, so a cell is considered once
                    // -- on the ring it first falls on, which is the one that
                    // orders the search by distance.
                    if dx.abs() != ring && dz.abs() != ring {
                        continue;
                    }
                    let (x, z) = (cx + dx, cz + dz);
                    if x < 0 || z < 0 || x >= WIDTH as isize || z >= WIDTH as isize {
                        continue;
                    }
                    let cell = z as usize * WIDTH + x as usize;
                    if !self.walkable[cell] {
                        continue;
                    }
                    // Nearest by true distance rather than by ring, so the
                    // corner of a ring never beats the middle of the same one.
                    let range = self.centre_of(cell).distance_squared(at);
                    if best.is_none_or(|(shortest, _)| range < shortest) {
                        best = Some((range, cell));
                    }
                }
            }
            if let Some((_, cell)) = best {
                return Some(cell);
            }
        }
        None
    }

    /// A walkable route from one point to another, as a short list of points to
    /// walk at in turn.
    ///
    /// **This is the answer to a body that walks into the wrong side of a wall
    /// and stays there.** Steering -- [`crate::squad::steer`] -- can only see a
    /// stride and a half ahead, so it bends round a fence and drives straight
    /// into a courtyard; it has no way to know that the way to the far side of
    /// the castle is back the way it came. A route knows, because it is a search
    /// over the whole grid, and the steering then has nothing harder to do than
    /// get to the next corner it can see.
    ///
    /// Priced rather than counted. A diagonal step costs what a diagonal step is
    /// -- so a route across open lawn is a straight line rather than a staircase
    /// with the same number of hops -- and a step into deep water costs `wet`
    /// metres extra, which is how "prefer not to swim" is said to a search. Pass
    /// a small `wet` for a body that has been ordered somewhere and a large one
    /// for a body with nothing better to do; it is the same preference
    /// [`crate::goap::Goal::caution`] expresses to the steering, one scale up.
    ///
    /// `budget` is handed straight to [`crate::route::astar`] and is the promise
    /// that this cannot eat a frame. What comes back when it runs out is a route
    /// to the nearest the search got, which is a body walking usefully while it
    /// waits to be asked again.
    ///
    /// The points are **corners, not cells**. A* answers in cells and a cell
    /// chain walked literally is a body shuffling from one square's middle to
    /// the next; what comes back here is that chain pulled taut against the
    /// walls, so a run across the lawn is two points and a route round the
    /// castle is one per corner. See [`FlowField::clear`], which is the same
    /// test the sweep built the route with.
    /// Hangs a way across the map on the grid, or takes the last one off.
    ///
    /// **One edge, and every answer the grid gives changes.** The sweep that
    /// tells the crowd which way the player is walks over it, so a horde on the
    /// far side of the castle comes through the portal rather than round the
    /// long way; the A* a Mario runs over it, so an errand across the map is
    /// suddenly worth taking; and the tautening knows to break the route there
    /// rather than draw a straight line between two points a hundred metres
    /// apart. That is the whole reason a portal is a fact about the *field*
    /// rather than a thing the follower checks for as it walks: a body that
    /// discovers a shortcut when it is standing on it has already walked the
    /// long way.
    ///
    /// The two ends are given as [`Warp`]s and are snapped to whatever the
    /// survey says is standable near them, which is what stops an opening on a
    /// wall two metres above a ledge from wiring the crowd into thin air. An
    /// end with nothing standable near it wires nothing at all: a one-way edge
    /// is a route the crowd walks into and cannot come back out of.
    ///
    /// The last sweep is thrown away rather than kept, because it was made over
    /// a different graph -- a field that still remembers the old edge is a field
    /// pointing a crowd at a portal that is not there any more.
    pub fn set_warp(&mut self, pair: Option<(Warp, Warp)>) {
        self.warps.clear();
        if let Some((first, second)) = pair {
            if let (Some(here), Some(there)) = (
                self.nearest_walkable(first.stand),
                self.nearest_walkable(second.stand),
            ) {
                // Refused when the two ends land in the same cell, which is a
                // pair of portals close enough together to be the same place.
                // The edge would be a self-loop, and a self-loop with a cost of
                // nearly nothing is a search that settles the same cell over
                // and over.
                if here != there {
                    self.warps.push(Link {
                        cell: here,
                        exit: there,
                        mouth: first.mouth,
                        stand: first.stand,
                        landing: second.stand,
                    });
                    self.warps.push(Link {
                        cell: there,
                        exit: here,
                        mouth: second.mouth,
                        stand: second.stand,
                        landing: first.stand,
                    });
                }
            }
        }
        // Whatever it was, the graph has changed.
        self.swept_from = None;
        self.due = 0.0;
    }

    /// The warp used from this cell, if there is one.
    ///
    /// A scan over a list that holds two entries at most, which is why it is a
    /// list rather than an array the width of the grid: this is asked once per
    /// node expansion, beside eight neighbour lookups, and two compares against
    /// nine thousand words of cache is the cheaper of the two shapes.
    fn warp_at(&self, here: usize) -> Option<&Link> {
        self.warps.iter().find(|link| link.cell == here)
    }

    /// Where a step into this cell's warp comes out.
    fn warp_exit(&self, here: usize) -> Option<usize> {
        self.warp_at(here).map(|link| link.exit)
    }

    /// Whether the grid currently has any way across that is not a step.
    pub fn warped(&self) -> bool {
        !self.warps.is_empty()
    }

    pub fn route(
        &self,
        search: &mut crate::route::Search,
        from: Vec3,
        to: Vec3,
        budget: usize,
        tolls: Tolls,
    ) -> Option<Routed> {
        let start = self.nearest_walkable(from)?;
        let goal = self.nearest_walkable(to)?;
        let cell = self.cell;
        // Octile distance in metres: the exact cost of walking from one cell to
        // another over empty ground, and never more than the cost of walking it
        // over real ground.
        let octile = |from: usize, to: usize| {
            let (fx, fz) = ((from % WIDTH) as f32, (from / WIDTH) as f32);
            let (tx, tz) = ((to % WIDTH) as f32, (to / WIDTH) as f32);
            let (dx, dz) = ((fx - tx).abs(), (fz - tz).abs());
            (dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)) * cell
        };
        // What makes the first route found the cheapest one is that the estimate
        // never exceeds the real remaining cost. Both tolls only ever add, so
        // neither can break that -- **but a warp can**, and this is the one
        // place in the search that has to know portals exist. Walking distance
        // is no longer a lower bound on the way there once there is an edge
        // that crosses the map for nothing: a cell fifty metres from the goal
        // and two from a portal whose far end is next to it is two metres away,
        // and an estimate of fifty would let the search settle on a longer route
        // and stop. So the estimate is the best of going there and going *via*
        // each end of each warp, which is a lower bound again -- and is two
        // extra octile distances on a list that is empty in almost every game.
        let heuristic = |node: usize| {
            let direct = octile(node, goal);
            self.warps.iter().fold(direct, |best: f32, link| {
                best.min(octile(node, link.cell) + octile(link.exit, goal))
            })
        };
        let found = crate::route::astar(
            search,
            WIDTH * WIDTH,
            start,
            goal,
            budget,
            |here| {
                neighbours(here)
                    .filter(move |&(step, there)| self.passable(here, step, there))
                    .map(move |(step, there)| {
                        let (dx, dz) = STEPS[step];
                        let stride = if dx != 0 && dz != 0 {
                            cell * std::f32::consts::SQRT_2
                        } else {
                            cell
                        };
                        (there, stride + self.toll(there, tolls))
                    })
                    // Stepping through is free of ground covered and is *not*
                    // free: a cost of nothing makes every route through a
                    // portal tie with every other, and a search full of ties
                    // settles cells in whatever order the queue happens to pop.
                    // A stride is what it actually costs -- one step, like any
                    // other -- and it keeps the ordering meaningful.
                    .chain(
                        self.warp_at(here)
                            .map(|link| (link.exit, cell + self.toll(link.exit, tolls))),
                    )
            },
            heuristic,
        )?;
        Some(Routed {
            legs: self.thread(from, &found.nodes, to, found.partial),
            settled: found.settled,
            partial: found.partial,
            exhausted: found.exhausted,
        })
    }

    /// Pulls a chain of cells taut, breaking it wherever it goes through a
    /// warp.
    ///
    /// [`Self::pull`] straightens a run of cells by asking whether the body
    /// could simply walk from one to a later one, and across a warp that
    /// question has the wrong answer twice over: the two cells are a hundred
    /// metres apart, so the honest answer is no and the pull stops -- and where
    /// two portals happen to be strung out along one open line of sight it is
    /// *yes*, and the pull quietly replaces the shortcut with the walk it was
    /// supposed to save. Neither is a route.
    ///
    /// So the chain is cut at every warp edge and each piece pulled on its own.
    /// The piece before a crossing gets a leg **through** the opening rather
    /// than in front of it -- see [`Warp::mouth`] -- and the piece after it is
    /// pulled from where the body will actually be standing when it arrives.
    fn thread(&self, from: Vec3, cells: &[usize], to: Vec3, partial: bool) -> Vec<Vec3> {
        if self.warps.is_empty() {
            return self.pull(from, cells, to, partial);
        }
        let mut legs = Vec::new();
        let mut anchor = from;
        let mut start = 0;
        for index in 0..cells.len().saturating_sub(1) {
            let (here, next) = (cells[index], cells[index + 1]);
            // A warp edge is one the grid could not have taken: the exit of the
            // warp standing in this cell, and not a cell next door. Both halves
            // are needed -- a portal whose two ends landed in neighbouring cells
            // is refused by `set_warp`, but a route may still step from one of
            // them to the other the ordinary way, and cutting the chain there
            // would put a leg through a wall that is not between them.
            let crossed = self
                .warp_at(here)
                .filter(|link| link.exit == next && !adjacent(here, next));
            let Some(link) = crossed else {
                continue;
            };
            let mut piece = self.pull(anchor, &cells[start..=index], link.stand, false);
            // And then the opening itself, past the surface. Nothing drops this
            // leg by reaching it -- it is on the far side of a wall, which is
            // exactly what [`crate::path::Route::turned`] refuses to count as
            // turned -- because nothing has to: a body that walks at it is
            // carried through by [`crate::portal::transit`] on the tick it
            // crosses, and arrives with its route replanned from the other side.
            piece.push(link.mouth);
            legs.append(&mut piece);
            anchor = link.landing;
            start = index + 1;
        }
        legs.extend(self.pull(anchor, &cells[start..], to, partial));
        legs
    }

    /// What stepping into a cell costs on top of the stride, in metres.
    ///
    /// The room half is graded rather than a threshold: a cell that touches
    /// something pays the whole toll, one with a cell of daylight pays a third
    /// of it, and anything roomier pays nothing. Graded because a threshold
    /// would make the route indifferent between scraping a wall and standing
    /// one clear of it -- which is most of what this is for -- and because a
    /// cliff in the cost is a place for two routes to swap every time the
    /// search runs.
    fn toll(&self, cell: usize, tolls: Tolls) -> f32 {
        let wet = if self.wet[cell] { tolls.wet } else { 0.0 };
        let room = match self.clearance[cell] {
            0 => tolls.hug,
            1 => tolls.hug / 3.0,
            _ => 0.0,
        };
        wet + room
    }

    /// Whether a body could walk straight from one point to the other *without
    /// scraping anything on the way*.
    ///
    /// [`Self::clear`] with one line added, and the line is what stops the pull
    /// undoing the routing. A* is what puts a route a cell or two out from a
    /// wall; pulling the result taut is what would put it straight back --
    /// straightening a dog-leg round a corner is exactly the operation that
    /// takes the corner as tightly as the geometry allows. So the pull is not
    /// allowed to straighten *through* the cells that touch things.
    ///
    /// **The bar is what the route already managed**, rather than a number of
    /// its own, and that is the whole of making this safe. A route through a
    /// gate runs through cells with no room by definition; a pull that insisted
    /// on room there would refuse to straighten anything near one and hand back
    /// the raw staircase. Asked instead to keep whatever the chain it is
    /// replacing had, the pull can shorten a route and cannot tighten it --
    /// which is the only property it needs.
    fn roomy(&self, from: Vec3, to: Vec3, wanted: u8) -> bool {
        self.along(from, to, |here, step, there| {
            let passable = match step {
                Some(step) => self.passable(here, step, there),
                None => {
                    self.walkable[there] && (self.ground[there] - self.ground[here]).abs() <= CLIMB
                }
            };
            passable && self.clearance[there] >= wanted
        })
    }

    /// Pulls a chain of cells taut, so what comes back is corners rather than
    /// squares.
    ///
    /// Greedy and forward-only: hold an anchor, run along the chain while the
    /// cell after next is still in sight of it, and commit the last one that
    /// was. That is one visibility test per cell rather than the one per *pair*
    /// a best-possible smoothing would want, and on a route of a hundred cells
    /// the difference is a hundred tests against ten thousand.
    ///
    /// The real destination is put back on the end, and the real start is where
    /// the pull begins -- so a body half way across a cell is not first asked to
    /// walk back to that cell's middle. A partial route has no destination to
    /// put back: it ends where the search ran out, and the last cell centre is
    /// the honest answer for where to walk to next.
    fn pull(&self, from: Vec3, cells: &[usize], to: Vec3, partial: bool) -> Vec<Vec3> {
        let mut legs = Vec::new();
        let mut anchor = from;
        let mut index = 1;
        while index < cells.len() {
            let mut furthest = index;
            // The least room anything on the stretch being replaced had --
            // counting only what the straight line would cut *out*, not the two
            // ends it keeps. A route that starts or finishes against a wall
            // genuinely does start or finish against a wall, and holding the
            // straight line to that would be holding it to nothing. What is
            // worth protecting is the room the chain went out of its way to
            // find in between. See [`Self::roomy`].
            let mut worst = ROOM_CAP;
            while furthest + 1 < cells.len() {
                // Extending past `furthest` is what makes that cell an interior
                // one, so that is the moment its room joins the bar.
                let wanted = worst.min(self.clearance[cells[furthest]]);
                if !self.roomy(anchor, self.centre_of(cells[furthest + 1]), wanted) {
                    break;
                }
                worst = wanted;
                furthest += 1;
            }
            let corner = self.centre_of(cells[furthest]);
            legs.push(corner);
            anchor = corner;
            index = furthest + 1;
        }
        if partial {
            return legs;
        }
        // The point actually asked for, rather than the middle of the cell it
        // fell in -- **but only where the field agrees it can be walked to.**
        // A destination is very often a little off the grid's idea of ground: a
        // ball on a ledge, a slot in a cluster that landed inside a wall, a spot
        // the player pointed at across a step. Substituting one of those in
        // unconditionally puts a leg on the end of the route that the search
        // never proved and the sweep would refuse, which is the follower walking
        // into the thing the route was supposed to get it round.
        //
        // Replacing the last corner rather than following it, where the corner
        // is the one the destination sits in: they are the same place to within
        // half a cell, and two waypoints that close read as a stutter at the end
        // of every walk.
        let anchor = match legs.len() {
            0 | 1 => from,
            len => legs[len - 2],
        };
        if self.clear(anchor, to) {
            match legs.last_mut() {
                Some(last) => *last = to,
                None => legs.push(to),
            }
        } else if legs.last().is_some_and(|last| self.clear(*last, to)) {
            legs.push(to);
        }
        legs
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
        self.along(from, to, |here, step, there| match step {
            Some(step) => self.passable(here, step, there),
            // Two cells that are neither the same nor neighbours, which the
            // sampling in [`Self::along`] only leaves when the segment runs off
            // the grid and `index` clamps both ends. Answered on what can be
            // answered rather than waved through.
            None => self.walkable[there] && (self.ground[there] - self.ground[here]).abs() <= CLIMB,
        })
    }

    /// Whether anything solid stands between two points -- a fence, a wall, the
    /// face of a step too tall to climb -- *without* asking whether the far end
    /// is ground anybody could stand on.
    ///
    /// The distinction is the whole reason this exists beside [`Self::clear`],
    /// and it is the one [`crate::enemy::walk`] already draws in the near tier:
    /// the question "may my feet go there" and the question "is my body about
    /// to be inside a wall" have different answers and belong at different
    /// distances. A walker's feet are tested where they land; its body is
    /// tested a radius further on, and out there the *height* of the ground is
    /// no longer any of its business. Asking `clear` at arm's length instead --
    /// which is to say asking [`CLIMB`] out there too -- held the whole crowd a
    /// body's width back from every hummock on the lawn, and cost a fifth of
    /// the distance the field covered.
    ///
    /// Two things count as a wall here, and the second is the one the castle
    /// needed. [`Self::blocked`] is a fence: something thin standing between
    /// two patches of good ground. A cell that is not walkable at all is the
    /// other -- the castle's own footprint, a cliff face, the far side of the
    /// grid -- and the survey never records an edge *into* one of those as
    /// blocked, because both ends of a blocked edge are walkable by
    /// construction. So the wall of the castle was, to a body probe that looked
    /// only at fences, not there: an ant would put its centre on the last legal
    /// cell of lawn and stand with two and a half metres of itself inside the
    /// stonework.
    pub fn walled(&self, from: Vec3, to: Vec3) -> bool {
        !self.along(from, to, |here, step, there| match step {
            Some(step) => self.blocked[here] & (1 << step) == 0 && self.walkable[there],
            None => self.walkable[there],
        })
    }

    /// Walks the segment from cell to cell, handing every boundary it crosses
    /// to `edge` as `(here, which of [`STEPS`] it took, there)`, and stops at
    /// the first one that answers false.
    ///
    /// Sampled at no more than half a cell, so that two consecutive samples are
    /// always either the same cell or two neighbouring ones -- which is the only
    /// shape [`Self::passable`] can answer, and the shape the sweep asked its
    /// own question in. A step at a walking pace is shorter than a cell, so the
    /// common caller pays one sample and three array reads exactly as it did
    /// before there was a walk here at all.
    ///
    /// What made the walk necessary is the callers that ask about a *body*
    /// rather than a point: [`crate::enemy::crowd_step`] probes a radius past
    /// where its feet land, and an ant's radius is half again the width of a
    /// cell. Looking only at the two ends of that probe is looking everywhere
    /// except where the wall it is meant to find actually is.
    fn along<F>(&self, from: Vec3, to: Vec3, mut edge: F) -> bool
    where
        F: FnMut(usize, Option<usize>, usize) -> bool,
    {
        let span = to - from;
        let reach = Vec2::new(span.x, span.z).length();
        let samples = (reach / (self.cell * 0.5)).ceil().max(1.0) as usize;
        let mut here = self.index(from);
        for sample in 1..=samples {
            let there = self.index(from + span * (sample as f32 / samples as f32));
            if there == here {
                continue;
            }
            let (hx, hz) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
            let (tx, tz) = ((there % WIDTH) as isize, (there / WIDTH) as isize);
            let step = STEPS.iter().position(|&d| d == (tx - hx, tz - hz));
            if !edge(here, step, there) {
                return false;
            }
            here = there;
        }
        true
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

    // Seeded from the player's own cell, or -- when he is somewhere the survey
    // called unwalkable, which is to say in the air, in the water, or on a
    // ledge between cells -- from whatever walkable cells touch it, so the
    // crowd still comes for him.
    let sources: Vec<usize> = if field.walkable[from] {
        vec![from]
    } else {
        neighbours(from)
            .filter(|&(_, cell)| field.walkable[cell])
            .map(|(_, cell)| cell)
            .collect()
    };
    // The sweep itself is [`crate::route::flood`], which is the same walk the
    // pylon network runs over its masts. What is local to a flow field is the
    // *edges* -- which is what `passable` is -- and those stay here.
    let swept = {
        // Borrowed for the sweep and no longer: `flood` writes into its own
        // array and hands it back, so the field is only mutated once that
        // borrow is over.
        let grid = &*field;
        crate::route::flood(WIDTH * WIDTH, sources, |here| {
            neighbours(here)
                .filter(move |&(step, there)| grid.passable(here, step, there))
                .map(|(_, there)| there)
                // And the way across that is not a step. One line, and it is
                // the whole of a crowd of two thousand knowing that a portal
                // exists: the sweep walks it like any other edge, so the cells
                // on the far side of it count their distance to the player
                // through the opening and the flow they hand out points at it.
                .chain(grid.warp_exit(here))
        })
    };
    field.steps = swept.steps;

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
        // And the warp, after the eight, because it is the one edge out of this
        // cell that is not one of them. A cell holding a portal mouth whose far
        // side is nearer the player points **at the opening** rather than at a
        // neighbouring cell -- the direction is in metres of world rather than
        // in cells, which normalising makes the same thing -- and the crowd
        // walks into it and is carried through. Without this the cell is still
        // *counted* through the portal by the sweep above, so the crowd knows
        // it is close, and then walks at whichever neighbour is next nearest:
        // a horde standing in front of an open portal shuffling sideways.
        if let Some(link) = field.warp_at(here) {
            if field.steps[link.exit] < best {
                let centre = field.centre_of(here);
                towards = Vec2::new(link.mouth.x - centre.x, link.mouth.z - centre.z);
            }
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

impl FlowField {
    /// Which way one of the eight steps points, in cells. The order the bits of
    /// [`Survey::blocked`] are in, so a caller drawing blocked edges can turn a
    /// bit back into a direction.
    pub fn step_offset(step: usize) -> IVec2 {
        let (dx, dz) = STEPS[step % STEPS.len()];
        IVec2::new(dx as i32, dz as i32)
    }

    /// How many steps there are, which is how many bits [`Survey::blocked`] has.
    pub fn step_count() -> usize {
        STEPS.len()
    }
}

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

/// Whether two cells touch, which is the same thing as one being reachable from
/// the other in a single step of the grid.
///
/// Stated on the coordinates rather than by searching [`STEPS`], so it stays
/// one subtraction and two compares in the middle of a loop over a route.
fn adjacent(here: usize, there: usize) -> bool {
    let (hx, hz) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
    let (tx, tz) = ((there % WIDTH) as isize, (there / WIDTH) as isize);
    (hx - tx).abs() <= 1 && (hz - tz).abs() <= 1 && here != there
}

/// The up-to-eight cells touching this one, each with the step that reaches it.
fn neighbours(here: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..STEPS.len()).filter_map(move |step| neighbour(here, step).map(|to| (step, to)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Every leg of a route is a walk the field itself would allow.
    ///
    /// This is the property the whole thing rests on: [`FlowField::pull`] takes
    /// a chain of cells the sweep proved passable and throws most of them away,
    /// and a pull that cuts a corner through a fence has produced a route that
    /// looks shorter and cannot be walked. `clear` is the same test the sweep
    /// used, asked again of the taut version.
    fn every_leg_is_walkable(field: &FlowField, from: Vec3, routed: &Routed) {
        let mut anchor = from;
        for (index, leg) in routed.legs.iter().enumerate() {
            assert!(
                field.clear(anchor, *leg),
                "leg {index} of {} runs through something: {anchor:?} -> {leg:?}",
                routed.legs.len()
            );
            anchor = *leg;
        }
    }

    #[test]
    fn a_route_across_open_lawn_is_a_straight_line() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        // Two points on the flat lawn in front of the castle, twenty metres
        // apart, with nothing between them.
        let from = Vec3::new(-13.28, 2.6, 38.64);
        let to = Vec3::new(-13.28, 2.6, 50.64);
        let routed = field
            .route(&mut search, from, to, 4000, Tolls::default())
            .expect("no route across the lawn");
        assert!(!routed.partial, "{routed:?}");
        // Pulled taut, an unobstructed walk is one leg: the destination.
        assert_eq!(routed.legs.len(), 1, "{routed:?}");
        assert!(routed.legs[0].distance(to) < 0.01);
        // And the search did not settle the whole grid to work that out.
        assert!(
            routed.settled < WIDTH * WIDTH / 4,
            "settled {} of {} cells for a straight line",
            routed.settled,
            WIDTH * WIDTH
        );
        every_leg_is_walkable(&field, from, &routed);
    }

    /// **The case a beeline cannot answer.** Two points with the castle between
    /// them: the straight line runs through the building, and the only way there
    /// is round -- which is a thing no amount of looking a stride ahead can work
    /// out.
    #[test]
    fn a_route_round_the_castle_goes_round_it() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let from = Vec3::new(-13.28, 2.6, 46.64);
        // Straight through the castle and out the back.
        let to = Vec3::new(-13.28, 2.6, -30.0);
        let Some(routed) = field.route(&mut search, from, to, 8000, Tolls::default()) else {
            return; // nowhere to stand at the far end on this castle
        };
        every_leg_is_walkable(&field, from, &routed);
        if routed.partial {
            return; // the far side is genuinely cut off, which is a fine answer
        }
        // A way round is longer than the way through, and that difference is
        // the whole value of the search.
        let walked: f32 = routed
            .legs
            .iter()
            .fold((from, 0.0), |(last, total), leg| {
                (*leg, total + last.distance(*leg))
            })
            .1;
        assert!(
            walked > from.distance(to),
            "the route is shorter than the straight line, so it went through: {routed:?}"
        );
        assert!(
            routed.legs.len() > 1,
            "one leg through a castle: {routed:?}"
        );
    }

    /// The two cells furthest apart the survey can offer, and a portal pair
    /// joining them.
    ///
    /// Chosen by the survey rather than named by hand: the castle moves and a
    /// test that writes coordinates into itself is a test that quietly stops
    /// testing anything.
    fn across_the_map(field: &FlowField) -> (Vec3, Vec3) {
        let walkable: Vec<usize> = (0..WIDTH * WIDTH)
            .filter(|&at| field.walkable[at])
            .collect();
        let span = |x: usize, y: usize| {
            ((x % WIDTH) as f32 - (y % WIDTH) as f32).hypot((x / WIDTH) as f32 - (y / WIDTH) as f32)
        };
        let (near, far) = walkable
            .iter()
            .flat_map(|&a| walkable.iter().map(move |&b| (a, b)))
            .max_by(|(al, ar), (bl, br)| span(*al, *ar).total_cmp(&span(*bl, *br)))
            .expect("the castle has no walkable ground");
        (field.centre_of(near), field.centre_of(far))
    }

    /// A mouth standing at a spot, whose opening is half a metre east of it.
    fn opening(at: Vec3) -> Warp {
        Warp {
            stand: at,
            mouth: at + Vec3::X * 0.5,
        }
    }

    /// Ground actually covered on foot: the sum of the legs, less the crossing.
    ///
    /// **The crossing has to come out or the measurement is meaningless.** A
    /// route through a portal has one leg joining two points a hundred and
    /// fifty metres apart, and that leg is not walked at all -- it is the
    /// transit. Counting it makes a route that crosses the map for nothing look
    /// like the longest route there is.
    fn on_foot(from: Vec3, legs: &[Vec3], jump: f32) -> f32 {
        legs.iter()
            .fold((from, 0.0), |(last, total), leg| {
                let step = last.distance(*leg);
                (*leg, total + if step > jump { 0.0 } else { step })
            })
            .1
    }

    /// A pair of portals is a way across the map, and the search takes it.
    #[test]
    fn a_route_takes_a_portal_rather_than_walking_round() {
        let (level, _) = crate::level::load();
        let mut field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let (from, to) = across_the_map(&field);
        let jump = field.cell_size() * 8.0;

        // Without a portal, whatever the walk costs.
        let plain = field
            .route(&mut search, from, to, 20000, Tolls::default())
            .expect("no route across the castle at all");
        let walked = on_foot(from, &plain.legs, jump);
        assert!(
            walked > 40.0,
            "the two ends are not far enough apart to test"
        );

        // And now with one at each end.
        field.set_warp(Some((opening(from), opening(to))));
        assert!(field.warped());
        let through = field
            .route(&mut search, from, to, 20000, Tolls::default())
            .expect("no route with a portal open");
        assert!(!through.partial, "{through:?}");
        assert!(
            on_foot(from, &through.legs, jump) < walked * 0.25,
            "the route still walked {} of the {walked} the ground costs: {through:?}",
            on_foot(from, &through.legs, jump)
        );

        // And taking it away puts the field back where it was, rather than
        // leaving a shortcut nobody can see.
        field.set_warp(None);
        assert!(!field.warped());
        let again = field
            .route(&mut search, from, to, 20000, Tolls::default())
            .expect("no route after the portal closed");
        assert!(
            (on_foot(from, &again.legs, jump) - walked).abs() < 1.0,
            "{again:?}"
        );
    }

    /// The taut route is broken at the crossing rather than drawn through it.
    ///
    /// Two things, and each of them was a way of getting this wrong.
    /// [`FlowField::pull`] straightens a run of cells by asking whether the
    /// body could walk from one to a later one, and across a warp that question
    /// has the wrong answer twice: usually no, so the route is left as the raw
    /// staircase, and where the two mouths happen to lie on one open line of
    /// sight, *yes* -- which quietly replaces the shortcut with the walk it was
    /// supposed to save. So: the crossing is there, it is the mouth rather than
    /// the spot in front of it, and every stretch that is not the crossing is a
    /// walk the field itself would allow.
    #[test]
    fn a_route_through_a_portal_is_cut_at_the_crossing() {
        let (level, _) = crate::level::load();
        let mut field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let (from, to) = across_the_map(&field);
        let (here, there) = (opening(from), opening(to));
        field.set_warp(Some((here, there)));
        let routed = field
            .route(&mut search, from, to, 20000, Tolls::default())
            .expect("no route with a portal open");

        // The leg that carries the body through is the opening itself and not
        // the spot in front of it -- a route that stops at the spot is a body
        // that walks up to a portal and stands there admiring it.
        let crossing = routed
            .legs
            .iter()
            .position(|leg| leg.distance(here.mouth) < 1e-3)
            .unwrap_or_else(|| panic!("the route never goes through the opening: {routed:?}"));
        assert!(crossing + 1 < routed.legs.len(), "{routed:?}");
        assert!(
            routed.legs[crossing + 1].distance(to) < field.cell_size() * 2.0,
            "the route comes out somewhere that is not the far mouth: {routed:?}"
        );

        // And every stretch either side of it is a walk, rather than a line
        // drawn through whatever was in the way.
        let mut anchor = from;
        for (index, leg) in routed.legs.iter().enumerate() {
            if index != crossing && index != crossing + 1 {
                assert!(
                    field.clear(anchor, *leg),
                    "leg {index} runs through something: {anchor:?} -> {leg:?}"
                );
            }
            anchor = *leg;
        }
    }

    /// The crowd's own field points at the opening rather than past it.
    ///
    /// The sweep and the flow are two passes over the same graph and they have
    /// to agree: a cell whose shortest way to the player is through a portal is
    /// counted that way by the first, and if the second then points at a
    /// neighbouring cell instead, what is drawn is a horde standing in front of
    /// an open portal shuffling sideways.
    #[test]
    fn the_crowd_is_pointed_into_the_opening() {
        let (level, _) = crate::level::load();
        let base = FlowField::new(&level);
        let (player, far) = across_the_map(&base);

        let sweep = |warp: Option<(Warp, Warp)>| {
            let mut world = World::new();
            let mut field = FlowField::new(&level);
            field.set_warp(warp);
            world.insert_resource(field);
            world.insert_resource(Time::<()>::default());
            world.spawn((crate::player::Player, Transform::from_translation(player)));
            world.run_system_once(rebuild).expect("the sweep ran");
            world.remove_resource::<FlowField>().expect("the field")
        };

        // How far the crowd at the far end thinks it is on foot -- which on
        // this castle may be *no distance at all*, because the two ends of the
        // map are on opposite sides of the moat and the sweep never gets
        // there. Both answers are the same fact for this test, and the second
        // is the stronger one: a portal is the only way across.
        let walked = sweep(None).at(far).steps;
        // A portal from there to the player's own feet, and the same question.
        let opened = sweep(Some((
            opening(far),
            Warp {
                stand: player,
                mouth: player + Vec3::X * 0.5,
            },
        )));
        let hops = opened
            .at(far)
            .steps
            .expect("the far end has no route to the player even with a portal open");
        match walked {
            None => {} // cut off entirely, and now it is not
            Some(walked) => assert!(
                hops < walked / 4,
                "the portal was worth {hops} steps against {walked} on foot"
            ),
        }
        assert!(hops <= 4, "a portal to his feet is {hops} steps of walking");

        // And the direction handed to a body standing there points at the
        // opening rather than at whichever neighbour is next-nearest.
        let centre = opened.centre_of(opened.cell_at(far));
        let wanted = Vec2::new(far.x + 0.5 - centre.x, far.z - centre.z);
        let towards = opened.at(far).towards;
        assert!(
            wanted.length() > 1e-3 && towards.dot(wanted.normalize()) > 0.7,
            "the flow points {towards:?} rather than at the opening {wanted:?}"
        );
    }

    /// Water is a price on the route, exactly as it is a price on the step.
    #[test]
    fn a_priced_route_keeps_out_of_water_it_could_have_crossed() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        // A pair of points chosen by the survey itself rather than by hand: the
        // castle's moat moves whenever the level does, and a test that names
        // coordinates in it is a test that quietly stops testing anything.
        let wet_cells: Vec<usize> = (0..WIDTH * WIDTH)
            .filter(|&cell| field.walkable[cell] && field.wet[cell])
            .collect();
        assert!(!wet_cells.is_empty(), "the castle has no water in it");
        // Somewhere with water in the middle: step across a wet cell to the
        // dry ground on the far side of it.
        let Some((from, to)) = wet_cells.iter().find_map(|&wet| {
            let (x, z) = (wet % WIDTH, wet / WIDTH);
            (4..WIDTH - 4).contains(&x).then_some(())?;
            let near = z * WIDTH + x - 3;
            let far = z * WIDTH + x + 3;
            (field.walkable[near] && !field.wet[near] && field.walkable[far] && !field.wet[far])
                .then(|| (field.centre_of(near), field.centre_of(far)))
        }) else {
            return; // no stretch of this castle's water is shaped like that
        };
        let free = field
            .route(&mut search, from, to, 8000, Tolls::default())
            .unwrap();
        let priced = field
            .route(
                &mut search,
                from,
                to,
                8000,
                Tolls {
                    wet: 40.0,
                    ..Tolls::default()
                },
            )
            .unwrap();
        every_leg_is_walkable(&field, from, &priced);
        let wetness = |routed: &Routed| {
            routed
                .legs
                .iter()
                .filter(|leg| field.wet[field.index(**leg)])
                .count()
        };
        assert!(
            wetness(&priced) <= wetness(&free),
            "pricing the water did not steer it out: {priced:?} against {free:?}"
        );
    }

    /// **A route that runs alongside a wall does not run along it.**
    ///
    /// This is the one in the screenshot: a squad routed past the moat railings
    /// walks *touching* the railings, because the straight line is the cheapest
    /// line and a wall beside it costs nothing at all. Priced by [`Tolls::hug`],
    /// the same route bows a couple of cells out into the open -- which over a
    /// long run costs almost nothing in distance, and is the difference between
    /// a file of Marios scraping a fence and a group walking beside one.
    #[test]
    fn a_priced_route_walks_beside_a_wall_rather_than_along_it() {
        // A lawn with a wall down the middle of it, and a walk from one end of
        // that wall to the other, starting and finishing right against it.
        let mut vertices = vec![
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        let mut indices = vec![[0u32, 1, 2], [0, 2, 3]];
        {
            let base = vertices.len() as u32;
            vertices.extend([
                Vec3::new(-40., 0., 0.),
                Vec3::new(40., 0., 0.),
                Vec3::new(40., 6., 0.),
                Vec3::new(-40., 6., 0.),
            ]);
            indices.push([base, base + 1, base + 2]);
            indices.push([base, base + 2, base + 3]);
        }
        let level = LevelData::new(vertices, indices, Vec::new());
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let from = Vec3::new(-35.0, 0.0, -0.7);
        let to = Vec3::new(35.0, 0.0, -0.7);
        // How near the wall the route ever gets, in metres. Sampled along the
        // legs rather than at them, because what is being asked about is the
        // walk and not its corners.
        let standoff = |routed: &Routed| {
            let mut anchor = from;
            let mut nearest = f32::INFINITY;
            for leg in &routed.legs {
                for step in 0..=20 {
                    let at = anchor.lerp(*leg, step as f32 / 20.0);
                    // The middle of the run only. Both ends are *at* the wall
                    // because that is where the walk was asked to start and
                    // finish, and a route is not hugging anything by arriving
                    // where it was sent.
                    if at.x.abs() < 25.0 {
                        nearest = nearest.min(at.z.abs());
                    }
                }
                anchor = *leg;
            }
            nearest
        };
        let tight = field
            .route(&mut search, from, to, 8000, Tolls::default())
            .expect("no way along the wall");
        let wide = field
            .route(
                &mut search,
                from,
                to,
                8000,
                Tolls {
                    hug: 1.0,
                    ..Tolls::default()
                },
            )
            .expect("no way along the wall when it costs something");
        every_leg_is_walkable(&field, from, &wide);
        assert!(
            standoff(&tight) < field.cell_size(),
            "the unpriced route already kept its distance, so this staging \
             proves nothing: {} m off, {tight:?}",
            standoff(&tight)
        );
        assert!(
            standoff(&wide) > standoff(&tight) + field.cell_size(),
            "pricing the wall did not push the route off it: {} m against {} m, \
             {wide:?}",
            standoff(&wide),
            standoff(&tight)
        );
    }

    /// The budget is a promise about the frame, so it has to actually bind.
    #[test]
    fn a_route_out_of_budget_comes_back_partial_rather_than_late() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let from = Vec3::new(-13.28, 2.6, 46.64);
        let to = Vec3::new(-13.28, 2.6, -30.0);
        let routed = field
            .route(&mut search, from, to, 20, Tolls::default())
            .unwrap();
        assert!(
            routed.partial,
            "twenty cells crossed the castle? {routed:?}"
        );
        assert!(routed.settled <= 21, "the budget did not bind: {routed:?}");
        assert!(
            !routed.legs.is_empty(),
            "a partial route with nowhere to go"
        );
        every_leg_is_walkable(&field, from, &routed);
    }

    /// A body that is not standing on the grid's idea of ground can still be
    /// routed -- which is every Mario in the moat and every ball on a ledge.
    #[test]
    fn a_route_can_start_and_end_off_the_walkable_grid() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let mut search = crate::route::Search::default();
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        // High over the lawn, which no cell is walkable at.
        let sky = lawn + Vec3::Y * 40.0;
        assert!(!field.walkable[field.index(sky)] || field.index(sky) == field.index(lawn));
        let routed = field
            .route(
                &mut search,
                sky,
                lawn + Vec3::new(12.0, 0.0, 0.0),
                4000,
                Tolls::default(),
            )
            .expect("a body above the lawn could not be routed off it");
        assert!(!routed.legs.is_empty());
    }

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

    /// A body is held out of walls its centre is welcome to stand beside.
    ///
    /// The two questions the tier asks are asked at two distances, and this is
    /// the gap between them: every cell the castle stands on is unwalkable, so
    /// [`FlowField::clear`] happily lets a body put its centre on the last cell
    /// of lawn -- which for an ant is two and a half metres of body inside the
    /// stonework, and reads on screen as the crowd standing in the wall.
    /// [`FlowField::walled`] is what the step asks about the body, and it has to
    /// see what `clear` does not.
    #[test]
    fn a_body_is_stopped_by_a_wall_its_centre_is_allowed_to_stand_beside() {
        let (level, _) = crate::level::load();
        let field = FlowField::new(&level);
        let ant = crate::enemy::Kind::Ant.body().0;
        // Every walkable cell with an unwalkable neighbour: the lawn's edge
        // against the castle, the cliffs, the moat.
        let mut edges = 0;
        let mut caught = 0;
        for here in 0..WIDTH * WIDTH {
            if !field.walkable[here] {
                continue;
            }
            let (hx, hz) = ((here % WIDTH) as isize, (here / WIDTH) as isize);
            for (dx, dz) in STEPS {
                let (tx, tz) = (hx + dx, hz + dz);
                if !(0..WIDTH as isize).contains(&tx) || !(0..WIDTH as isize).contains(&tz) {
                    continue;
                }
                let there = tz as usize * WIDTH + tx as usize;
                if field.walkable[there] {
                    continue;
                }
                // A step that stops well short of the boundary -- an eighth of a
                // cell, which is a couple of ticks of walking -- so the centre
                // stays in the cell it started in and `clear` has nothing to
                // say. The body reaches on past it.
                let at = field.origin + Vec2::new(hx as f32 + 0.5, hz as f32 + 0.5) * field.cell;
                let at = Vec3::new(at.x, field.ground[here], at.y);
                let towards = Vec2::new(dx as f32, dz as f32).normalize();
                let step = Vec3::new(towards.x, 0.0, towards.y) * field.cell * 0.125;
                edges += 1;
                assert!(
                    field.clear(at, at + step),
                    "the centre was refused a step inside its own cell"
                );
                if field.walled(at, at + step + step.normalize() * ant) {
                    caught += 1;
                }
            }
        }
        assert!(edges > 100, "the castle offered only {edges} edges to test");
        assert_eq!(
            caught,
            edges,
            "{} of {edges} bodies were walked into a wall their centre was \
             merely standing next to",
            edges - caught
        );
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
    /// stream of slimes walking through a fence somebody is standing next to.
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
            // Asked from both sides. A cast that grazes the very end of a wall
            // can find it from one direction and miss it from the other, and a
            // single graze is not the thing this test is about: what it is
            // about is the flow pointing across something the survey plainly
            // calls a wall.
            if field.wall_between(&level, index, next) && field.wall_between(&level, next, index) {
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
        let barred: usize = field
            .blocked
            .iter()
            .map(|bits| bits.count_ones() as usize)
            .sum();
        println!(
            "survey took {took:?}; {barred} blocked edge-ends, {} cells touch one",
            field.blocked.iter().filter(|bits| **bits != 0).count()
        );
    }
}
