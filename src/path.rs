//! Routes, cached: how a body that wants to be somewhere finds out which way
//! that actually is.
//!
//! **The problem this exists for is a body walking confidently in the wrong
//! direction.** [`crate::squad::steer`] looks a stride and a half ahead, which
//! is enough to swing round a fence and nowhere near enough to know that the
//! fence runs for forty metres and the way past it is back the way it came. A
//! Mario sent to the far side of the castle sets off through the wall of it,
//! gets held out by collision, slides along the stone until the wall turns a
//! corner it did not want, and stands there looking broken -- and every part of
//! that is the correct behaviour of a rule that cannot see further than its own
//! feet. No amount of better steering fixes it, because the missing information
//! is not local.
//!
//! So the route is worked out once, over the whole navigation grid, and then
//! *followed*. [`crate::route::astar`] is the search;
//! [`crate::flow::FlowField::route`] is the grid it runs on and the taut list
//! of corners it hands back; this module is the part that decides who gets one
//! and how often.
//!
//! # Nothing here runs per body per frame
//!
//! That rule is the reason this is a module rather than a function call inside
//! the walk step, and it is enforced in four places rather than trusted:
//!
//!   * **A search is only asked for when the straight line is blocked.**
//!     [`FlowField::clear`] is three array reads a sample and answers the only
//!     question that matters most of the time -- can this body simply walk at
//!     the thing? On open lawn it can, and no search happens at all. Routing is
//!     what obstruction costs, not what walking costs.
//!   * **A route is trusted for a while.** `path_refresh` seconds, and until
//!     the destination has moved further than [`DRIFT`]. A Mario chasing a
//!     slime does not re-plan because the slime took a step.
//!   * **A tick serves at most `path_budget` searches**, over the whole field,
//!     handed out from a rotating cursor so a body at the back of the queue is
//!     served next tick rather than never. Twenty Marios all whistled at once
//!     are routed over five ticks -- a sixth of a second -- and the frame does
//!     not notice.
//!   * **A search that runs long is stopped**, and hands back the best start it
//!     found. See [`crate::route::astar`], which is where the budget per search
//!     lives.
//!
//! Between them those mean the cost of pathing is bounded by a console row and
//! not by how many bodies are on the field, which is the property the crowd
//! tier is built on everywhere else in this game.
//!
//! # What a follower does with one
//!
//! It walks at [`Route::leg`] instead of at its goal, and everything else stays
//! exactly as it was: the same steering, the same hazard costs, the same wall
//! collision. A leg is by construction a point the body can see from where it
//! is standing, so the local rule is only ever asked the easy version of its
//! question -- get to that corner -- and the global rule has already answered
//! the hard one.
//!
//! A body with no route walks at its goal, which is what it did before this
//! module existed. That is the fallback everywhere: an unreachable goal, a
//! search that ran out, a grid with no opinion. Pathing here makes walking
//! better and is never what makes walking possible.

use bevy::prelude::*;

use crate::{
    console::GameTuning,
    flow::{FlowField, Tolls},
};

/// Where a body is going, and the way there.
///
/// Carried by anything that wants routing. The decider writes [`Self::want`]
/// every tick -- it is two floats and a compare, so there is no need to be
/// clever about when -- and [`plan`] fills in the rest, for as many bodies a
/// tick as the budget allows.
#[derive(Component, Default, Debug)]
pub struct Route {
    /// Where the body wants to end up, and what it will pay to avoid water and
    /// walls on the way.
    want: Option<(Vec2, Tolls)>,
    /// This body's place in the line across the route, counted out from the
    /// middle: -3.5 to 3.5 for a group of eight.
    ///
    /// **Without it a squad routed anywhere walks in single file, and it is
    /// worth being precise about why, because it is not a bug in anything.**
    /// A route is a handful of points, and every body sent the same way is
    /// handed points within a metre of each other -- the corners come off cell
    /// centres, and the cheapest chain past an obstacle is one chain. So twelve
    /// Marios all steer at the same spot, [`crate::enemy::spread`] shoves the
    /// ones that arrive first out of the way of the ones behind, and what comes
    /// out is a queue. Every part of that is each system doing its job.
    ///
    /// A lane is one number that breaks the tie. Each body is given its own
    /// standing offset across the route, so the same corner is a slightly
    /// different point for each of them and the group travels as a band rather
    /// than a thread. Assigned in [`plan`] when the group this body belongs to
    /// is served, spread evenly from one side to the other -- so a group of
    /// seven walks seven abreast and the outside two are the outside two, and
    /// none of them swaps places mid-march.
    ///
    /// A *place* rather than a distance, multiplied by `path_spread` where it is
    /// used -- so that row is metres between neighbours, the band comes out as
    /// wide as the group needs, and dragging it widens a march already under
    /// way.
    lane: f32,
    /// The corners left to walk, nearest first. Empty when the straight line
    /// will do, which is most of the time.
    legs: Vec<Vec3>,
    /// The destination [`Self::legs`] was worked out for. A route is only as
    /// good as the goal it was made for, and this is what notices when that has
    /// moved out from under it.
    planned_for: Option<Vec2>,
    /// Seconds before it is worth asking again.
    fresh_for: f32,
    /// Whether the last search stopped short of the goal. Such a route is still
    /// walked -- it goes the right way -- but it is re-asked as soon as it runs
    /// out rather than trusted to the end.
    partial: bool,
    /// Where the leg at the head of the route runs *from*: the corner before it,
    /// or where the body was standing when the route was handed out.
    ///
    /// Only [`Self::laned`] wants it, and it wants it because a lane has to be
    /// an offset across the *leg* rather than across the body's own bearing.
    /// Taken from the bearing, the offset swings round as the corner is
    /// approached and the laned point orbits it, so it has to be faded out over
    /// the last stretch -- which collapses the whole group onto the corner
    /// exactly where it most wants to still be a group, and every corner
    /// squeezes the band back into a file. Taken across the leg it is a fixed
    /// point on a line parallel to the one the route drew, which is what a lane
    /// is, and it needs no fading at all.
    anchor: Vec3,
    /// Whether the last search found nothing at all: no ground at either end,
    /// or no way between them. The body falls back to walking at its goal,
    /// which is the honest thing to do about a place that cannot be walked to.
    lost: bool,
    /// Whether the last search settled every cell it could reach and never
    /// found the goal.
    ///
    /// **Not a shade of [`Self::partial`], and telling them apart is the whole
    /// of "they try to reach it forever".** Both stop short. A partial route is
    /// a search that ran out of budget, and the right answer to one is to walk
    /// it and ask again from further on -- next time it gets there. An
    /// exhausted one has already looked at every cell on this side of whatever
    /// is in the way; there is no "further on" that helps, and asking again
    /// every second for the rest of the session settles the same thousand cells
    /// to reach the same answer.
    ///
    /// It is still *walked* -- it goes as near as anything can get, which is
    /// where a body sent at an island wants to stand anyway. What it is for is
    /// callers who have to decide whether an errand is worth continuing to
    /// hold: see [`crate::squad::update_goals`], which retires an order over it
    /// rather than making a Mario lean on a wall for the full stall clock.
    unreachable: bool,
}

impl Route {
    /// Says where this body is trying to get to, and what it will pay to keep
    /// out of water and off walls on the way there.
    ///
    /// Called every tick by whatever decides. Cheap on purpose: a destination
    /// that has not moved does nothing at all, and one that has only marks the
    /// route for reconsideration rather than replanning on the spot.
    pub fn want(&mut self, at: Vec2, tolls: Tolls) {
        self.want = Some((at, tolls));
    }

    /// Says this body has nowhere to be, and drops whatever it was walking.
    pub fn clear(&mut self) {
        self.want = None;
        self.forget();
    }

    fn forget(&mut self) {
        // The lane is not forgotten. It is which of the group this body is,
        // rather than anything about the walk, and a body that drew a new one
        // every time it was replanned would swap places with its neighbours in
        // the middle of the march.
        self.legs.clear();
        self.planned_for = None;
        self.fresh_for = 0.0;
        self.partial = false;
        self.lost = false;
        self.unreachable = false;
    }

    /// The point to actually walk at from `here`, or `None` to walk at the goal.
    ///
    /// Legs already reached are dropped as they are passed, so this is the next
    /// corner and nothing has to keep an index. Reaching one is measured in the
    /// horizontal plane -- a corner on the far side of a step is still that
    /// corner -- **and against the grid, which is the part that is not
    /// decoration.**
    ///
    /// Near is not the same as arrived when there is a wall in between, and the
    /// difference is a whole class of body that walks its route backwards. A
    /// corner sits at a cell's middle, which can be half a cell from a wall; a
    /// body is held a radius clear of that wall from the other side. Put those
    /// together and a body pressed against the *inside* of a wall is about a
    /// metre from a corner on the outside of it -- inside [`REACHED`], counted
    /// as turned, and the route moves on to the leg after it, which is back the
    /// way it came. Watched, that is a Mario walking to the mouth of a
    /// courtyard, turning round, walking back in, and doing it again for ever.
    ///
    /// So a corner counts as turned when the body is near it *and* the field
    /// agrees the two are on the same side of everything. The test only runs
    /// for a body that is already within [`REACHED`], which is a stride and a
    /// half of samples and only on the ticks that matter.
    pub fn leg(&mut self, field: &FlowField, here: Vec3, spread: f32) -> Option<Vec3> {
        while self.turned(field, here, spread) {
            // The corner just turned is where the next leg runs from.
            self.anchor = self.legs.remove(0);
        }
        Some(self.laned(*self.legs.first()?, spread))
    }

    /// Whether the corner at the head of the route counts as turned.
    ///
    /// Three things, and the third is the one that stops a body cutting the
    /// corner it was routed round. **Near** it, so that a body still walking at
    /// it has not turned it. **In sight** of it, because a corner a metre away
    /// through a wall is not a corner this body has reached -- see the note on
    /// [`Self::leg`]. And **in sight of the one after it**, which is what
    /// turning a corner means: a route bends round an obstacle by putting a
    /// point at the place from which the next point can be seen, and a body
    /// that lets go of it any earlier than that walks at the next one from
    /// somewhere the obstacle is still in the way. That is a Mario pinned
    /// against the inside of a fence, sliding along it, with a perfectly good
    /// route in hand -- and [`REACHED`] is a metre and a half, so "any earlier"
    /// is most of the way.
    fn turned(&self, field: &FlowField, here: Vec3, spread: f32) -> bool {
        let Some(leg) = self.legs.first() else {
            return false;
        };
        // Measured against the point this body was actually walking at, lane
        // and all, rather than against the middle of the route: a body out on
        // the edge of a wide band has arrived when it reaches its own place in
        // the line, not when it reaches somebody else's.
        let aim = self.laned(*leg, spread);
        let flat = Vec2::new(here.x, here.z);
        if Vec2::new(aim.x, aim.z).distance(flat) > REACHED || !field.clear(here, *leg) {
            return false;
        }
        match self.legs.get(1) {
            None => true,
            Some(next) => field.clear(here, *next),
        }
    }

    /// Where this body walks for a given corner, with its lane in it.
    ///
    /// Offset across the *leg* -- the line from [`Self::anchor`] to the corner
    /// -- so it is a fixed point on a line parallel to the route, and the group
    /// walks a corridor rather than converging on a thread.
    ///
    /// **The last corner is never laned.** Everything routed to the same place
    /// already has its own spot when it gets there -- a slot in the cluster, a
    /// ball of its own -- and nudging a body off the spot it was sent to is a
    /// different and worse bug than the one the lane is fixing.
    fn laned(&self, corner: Vec3, spread: f32) -> Vec3 {
        let lane = self.lane * spread;
        if self.legs.len() < 2 || lane == 0.0 {
            return corner;
        }
        let along =
            Vec2::new(corner.x - self.anchor.x, corner.z - self.anchor.z).normalize_or_zero();
        if along == Vec2::ZERO {
            return corner;
        }
        let across = Vec2::new(-along.y, along.x) * lane;
        corner + Vec3::new(across.x, 0.0, across.y)
    }

    /// The corner currently being walked at, without taking any off and without
    /// this body's lane in it.
    ///
    /// For anything that wants to know whether a body is getting anywhere --
    /// [`crate::squad::update_goals`] measures its stall clock against this,
    /// because progress along a route is progress towards the next corner and
    /// has nothing to do with the distance left to the goal.
    pub fn aim(&self) -> Option<Vec3> {
        self.legs.first().copied()
    }

    /// The corners still to be walked, for anything drawing them. See [`draw`].
    pub fn legs(&self) -> &[Vec3] {
        &self.legs
    }

    /// Whether the last search gave up short of the goal.
    pub fn partial(&self) -> bool {
        self.partial
    }

    /// Whether the last search could find no way there at all.
    pub fn lost(&self) -> bool {
        self.lost
    }

    /// Whether where this body is going cannot be walked to from where it is.
    ///
    /// The two ways that can be true, together, because to a caller deciding
    /// whether to keep an errand alive they are the same fact: the search found
    /// nowhere to start or finish ([`Self::lost`]), or it searched out
    /// everything it could reach and the goal was not in it. See the note on
    /// [`Self::unreachable`].
    pub fn stranded(&self) -> bool {
        self.lost || self.unreachable
    }

    /// Whether this body is walking a worked-out route rather than a straight
    /// line.
    pub fn routed(&self) -> bool {
        !self.legs.is_empty()
    }

    /// Whether it is time to think about this one again.
    fn due(&self) -> bool {
        let Some((want, _)) = self.want else {
            return false;
        };
        match self.planned_for {
            // Never planned for anything, or planned for somewhere else.
            None => true,
            Some(planned) if planned.distance(want) > DRIFT => true,
            // A route that has run out of corners is a route that has been
            // walked, and the body is now steering at its goal on its own.
            // Worth re-asking only if it was never a whole route.
            Some(_) => self.fresh_for <= 0.0 && (self.partial || !self.legs.is_empty()),
        }
    }
}

/// How near a corner counts as having reached it, in metres.
///
/// Wider than an arrival at a real goal, and deliberately: a corner is a place
/// to turn rather than a place to be, and a body that insists on standing
/// exactly on each one walks a route as a series of stops. Wide enough to
/// swallow the shove [`crate::enemy::spread`] gives a body in a crowd, so a
/// Mario pushed past a corner counts it as turned rather than walking back to
/// it.
const REACHED: f32 = 1.6;

/// How far a destination may move before the route to it is worth redoing, in
/// metres.
///
/// A body chasing something that walks is the case this is set against. Re-plan
/// on every step of the quarry and a fight is a search a tick; never re-plan and
/// the route is to where the slime used to be. Three metres is about two cells
/// of the navigation grid -- the distance at which the *route* would actually
/// come out different, rather than the distance at which the goal has moved.
const DRIFT: f32 = 3.0;

/// How many nodes one search may settle before it hands back what it has.
///
/// **Measured rather than guessed, and the measurement is the argument.** The
/// worst honest route on the castle -- one corner of the grounds to the far
/// side of the building, a hundred and eighty metres of walking against eighty
/// as the crow flies -- settles 1,545 cells and takes 0.11 ms in a release
/// build. A short walk across the lawn settles a few dozen and takes 0.011 ms.
/// Four thousand is therefore comfortably above anything this map can ask for
/// while still being less than half the grid, so the pathological case --
/// somebody pointing at an island, where the only way to answer is to settle
/// every reachable cell proving there is no way -- costs about a third of a
/// millisecond rather than being unbounded.
///
/// It was 1,200 first, which is *below* that worst route, and the failure is
/// worth writing down because it does not look like a budget problem from
/// outside. A search that stops short comes back [`Route::partial`], and a
/// partial route ends at whichever cell the heuristic liked best -- which, for
/// a search stopped by a wall, is the cell pressed against the wall. So the
/// Mario walked confidently up to the near side of the castle, re-planned,
/// walked to the same spot again, and stood there: the exact behaviour routing
/// was added to remove, produced by the routing. A budget has to be large
/// enough to answer the questions the map actually contains.
const SETTLE_BUDGET: usize = 4000;

/// What routing did this tick, for the debug overlay.
///
/// A resource rather than a log line because the useful form of all of this is
/// a number that moves: a field where every route comes back partial, or where
/// the queue never empties, looks from outside like bodies changing their minds,
/// and the way to tell those apart is to watch the counters while it happens.
#[derive(Resource, Default, Debug)]
pub struct Pathing {
    /// Bodies that wanted thinking about on the last tick.
    pub queued: usize,
    /// Of those, how many were served.
    pub searched: usize,
    /// How many were answered without a search, because the straight line was
    /// clear. On open ground this is nearly all of them, and that is the whole
    /// budget argument in one number.
    pub direct: usize,
    /// How many came back short of the goal, and how many found nothing.
    pub partial: usize,
    pub lost: usize,
    /// How many distinct journeys those bodies turned out to be making.
    ///
    /// The number the sharing is for: eight Marios sent to one place is eight
    /// queued and one group, and therefore one search rather than eight. A
    /// field where this tracks `queued` one for one is a field where nothing is
    /// going the same way as anything else -- which happens, and is worth being
    /// able to see. Counted when the groups are formed rather than when they are
    /// served, so a tick that runs out of budget still says how much there was
    /// to do.
    pub groups: usize,
    /// How many bodies are walking a worked-out route right now, as against
    /// walking straight at what they want. On open ground this is zero and
    /// everything is working; a field where it never falls is a field where
    /// something is in everybody's way.
    pub routed: usize,
    /// Cells settled by the searches on the last tick.
    pub settled: usize,
    /// Where the round robin has got to, so a body passed over this tick is at
    /// the front of the queue on the next one.
    cursor: usize,
}

/// Hands out routes, to as many bodies a tick as the budget allows.
///
/// Runs between the systems that decide where a body is going and the ones that
/// walk it there, which is the only ordering this needs: a `want` written this
/// tick is routed this tick, and a body whose turn it was not walks last tick's
/// route, or its goal, and comes back round.
pub fn plan(
    field: Res<FlowField>,
    tuning: Res<GameTuning>,
    mut stats: ResMut<Pathing>,
    mut search: Local<crate::route::Search>,
    mut followers: Query<(&Transform, &mut Route)>,
) {
    let dt = crate::player::FIXED_DT;
    let budget = tuning.path_budget.max(0.0) as usize;
    let most = (tuning.path_group.max(1.0) as usize).max(1);
    // Everyone who wants thinking about, in query order -- which is stable
    // between ticks, so the cursor below means the same thing from one tick to
    // the next.
    let mut queue: Vec<Waiting> = Vec::new();
    let mut routed = 0;
    for (at, mut route) in &mut followers {
        route.fresh_for -= dt;
        routed += usize::from(route.routed());
        let Some((want, tolls)) = route.want else {
            // Told to stop wanting anything. Whatever it was walking is not
            // where it is going any more.
            if route.planned_for.is_some() {
                route.forget();
            }
            continue;
        };
        if route.due() {
            queue.push(Waiting {
                at: at.translation,
                want,
                tolls,
                route,
            });
        }
    }
    *stats = Pathing {
        queued: queue.len(),
        routed,
        cursor: stats.cursor,
        ..Pathing::default()
    };

    // The cheap question first, and on open ground it is the only one asked. A
    // body that can see where it is going does not need to be told the way, and
    // saying so here is what keeps the searches for the bodies that are
    // actually stuck -- and keeps them out of the grouping below, which is then
    // only ever grouping journeys that are worth working out.
    let mut stuck: Vec<usize> = Vec::new();
    for (index, waiting) in queue.iter_mut().enumerate() {
        let there = Vec3::new(waiting.want.x, waiting.at.y, waiting.want.y);
        if field.clear(waiting.at, there) {
            waiting.route.legs.clear();
            waiting.route.planned_for = Some(waiting.want);
            waiting.route.fresh_for = tuning.path_refresh;
            waiting.route.partial = false;
            waiting.route.lost = false;
            waiting.route.unreachable = false;
            stats.direct += 1;
        } else {
            stuck.push(index);
        }
    }
    if stuck.is_empty() {
        stats.cursor = 0;
        return;
    }

    // **Bodies going the same way from the same place are one journey, and one
    // journey is one search.** Bucketed on a block of cells at each end rather
    // than on the exact points, because a squad's members are metres apart and
    // sent to spots metres apart, and that is the same walk by any reasonable
    // reading. What they get back is one route between them; where they part
    // company is the last stretch, which they steer themselves.
    let block = |at: Vec3| {
        let cell = field.cell_at(at);
        (
            (cell % crate::flow::WIDTH / GROUP_BLOCK) as i32,
            (cell / crate::flow::WIDTH / GROUP_BLOCK) as i32,
        )
    };
    let key = |waiting: &Waiting| {
        let there = Vec3::new(waiting.want.x, waiting.at.y, waiting.want.y);
        (block(waiting.at), block(there))
    };
    stuck.sort_by_key(|index| key(&queue[*index]));

    // Each bucket, split into groups of at most `path_group`, as evenly as it
    // divides. See [`GROUP_BLOCK`] for why the cap is there at all.
    let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
    let mut run = 0;
    while run < stuck.len() {
        let mut end = run + 1;
        while end < stuck.len() && key(&queue[stuck[end]]) == key(&queue[stuck[run]]) {
            end += 1;
        }
        let count = end - run;
        let parts = count.div_ceil(most);
        let mut taken = run;
        for part in 0..parts {
            // The remainder spread over the first few rather than piled on the
            // last, so fourteen is seven and seven and not ten and four.
            let size = count / parts + usize::from(part < count % parts);
            groups.push(taken..taken + size);
            taken += size;
        }
        run = end;
    }

    stats.groups = groups.len();

    // Round robin over the groups. Starting where the last tick stopped is what
    // makes the budget a *rate* rather than a filter: without it the same few
    // at the front of the query are served every tick and everybody behind them
    // walks at their goals for ever.
    let start = stats.cursor % groups.len();
    for offset in 0..groups.len() {
        let turn = (start + offset) % groups.len();
        if stats.searched >= budget {
            // Out of turns. Left exactly as it was, so it is due again next
            // tick and the cursor hands it the first turn.
            stats.cursor = turn;
            return;
        }
        let group = groups[turn].clone();
        let members = &stuck[group];
        // Where the group is and where it is going, as one body: the middle of
        // it at each end. A route worked out for the middle is one every member
        // can pick up from where it stands, because they are all inside a block
        // of it by construction.
        let count = members.len() as f32;
        let mut here = Vec3::ZERO;
        let mut want = Vec2::ZERO;
        // **Planned to the standard of whoever in it minds most.** A shared
        // route is walked by everybody in the group, so a body that would have
        // gone round the water does not get dragged through it by one that
        // would not.
        let mut tolls = Tolls::default();
        for index in members {
            let waiting = &queue[*index];
            here += waiting.at;
            want += waiting.want;
            tolls.wet = tolls.wet.max(waiting.tolls.wet);
            tolls.hug = tolls.hug.max(waiting.tolls.hug);
        }
        here /= count;
        want /= count;
        let there = Vec3::new(want.x, here.y, want.y);
        stats.searched += 1;
        let routed = field.route(&mut search, here, there, SETTLE_BUDGET, tolls);
        match &routed {
            Some(found) => {
                stats.settled += found.settled;
                stats.partial += usize::from(found.partial);
            }
            None => stats.lost += 1,
        }
        let width = members.len();
        for (rank, index) in members.iter().enumerate() {
            let waiting = &mut queue[*index];
            let want = waiting.want;
            let at = waiting.at;
            let route = &mut waiting.route;
            // Which of the group this body is, across the width of the route.
            // Spread evenly from one side to the other rather than scattered,
            // so a group of seven walks seven abreast and the outside two are
            // the outside two -- and kept as a *fraction*, so dragging
            // `path_spread` on the console widens a march already under way.
            // Its place in the line, counted out from the middle: -3.5 to 3.5
            // for a group of eight. Multiplied by `path_spread` where it is
            // used, so that is *metres between neighbours* and the band comes
            // out as wide as the group needs rather than as wide as one number
            // says. A fixed width is the thing that does not work: three metres
            // is a comfortable front for two and a queue for eight.
            route.lane = rank as f32 - (width - 1) as f32 / 2.0;
            route.planned_for = Some(want);
            match &routed {
                Some(found) => {
                    // The first leg runs from where this body is standing.
                    route.anchor = at;
                    route.legs.clone_from(&found.legs);
                    route.partial = found.partial;
                    route.lost = false;
                    // Stopped short *and* searched out, which is the grid
                    // saying there is no way rather than saying it needs
                    // longer. See [`Route::unreachable`].
                    route.unreachable = found.partial && found.exhausted;
                    // A partial route is re-asked as soon as it is walked rather
                    // than trusted for the full refresh: the group is heading
                    // for the edge of what the last search could see, and the
                    // search from *there* is the one that finishes the job.
                    route.fresh_for = match found.partial {
                        true => tuning.path_refresh * 0.25,
                        false => tuning.path_refresh,
                    };
                }
                None => {
                    // No ground at one end, or no way between them. Walking at
                    // the goal is what a body did before there was any of this,
                    // and it is still the best available answer -- the
                    // destination may yet become reachable, and something
                    // standing still because a search failed is worse than
                    // something walking into a wall.
                    route.legs.clear();
                    route.partial = false;
                    route.lost = true;
                    route.unreachable = false;
                    route.fresh_for = tuning.path_refresh;
                }
            }
        }
    }
    // Every group that wanted a turn got one.
    stats.cursor = 0;
}

/// One body waiting to be told the way.
struct Waiting<'a> {
    at: Vec3,
    want: Vec2,
    tolls: Tolls,
    route: Mut<'a, Route>,
}

/// How many cells wide the block is that decides whether two bodies are making
/// the same journey.
///
/// Four, which on the castle's grid is about seven metres: a squad's members
/// are metres apart and are sent to spots metres apart, and calling that one
/// walk is the whole of the sharing. Wider and two bodies with genuinely
/// different problems get handed one answer; narrower and a cluster of eight
/// splits into six journeys and the sharing buys nothing.
const GROUP_BLOCK: usize = 4;

/// Draws what the pathing is doing, when `path_debug` says to.
///
/// Three levels, each adding to the one before, because they answer different
/// questions and the noisiest one is rarely the one being asked:
///
///   1. **The routes.** One line per body from where it is standing through
///      every corner it still has to turn, and a cross on the ground where it
///      has been told to be. Green for a whole route, amber for one that
///      stopped short, red for a body that could not be routed at all and is
///      walking at its goal on faith. This is the level that answers "why is
///      that Mario going that way".
///   2. **The grid**, near the camera: a mark on every cell the survey found
///      ground on -- blue where it is out of its depth, orange where it touches
///      something and amber one cell clear of it, green out in the open, grey
///      where the sweep never reached -- and a bar across every edge something
///      solid stands in. This is the level that answers both "why does it think
///      it cannot get there", which is nearly always a fence the survey found
///      and the eye did not, and "why is it going the long way", which is the
///      orange.
///   3. **The flow field**, as an arrow per cell pointing the way the crowd
///      reads. Nothing to do with routing -- it is the other navigation system
///      in this game, and having both drawn the same way is how you tell which
///      of them a given piece of behaviour came from.
///
/// Drawn per frame rather than per tick, and immediate-mode, so it costs
/// nothing at all while it is off and leaves nothing behind when it is turned
/// off mid-session.
pub fn draw(
    tuning: Res<GameTuning>,
    field: Res<FlowField>,
    mut gizmos: Gizmos,
    followers: Query<(&Transform, &Route)>,
    camera: Query<&Transform, With<Camera3d>>,
) {
    let level = tuning.path_debug.round() as u32;
    if level == 0 {
        return;
    }
    // Off the ground, or half of every line is inside the lawn it runs over.
    let lift = Vec3::Y * 0.3;
    for (at, route) in &followers {
        let colour = match (route.lost(), route.partial()) {
            (true, _) => Color::srgb(1.0, 0.25, 0.2),
            (_, true) => Color::srgb(1.0, 0.75, 0.1),
            _ => Color::srgb(0.25, 1.0, 0.35),
        };
        if let Some((want, _)) = route.want {
            // The goal itself, whether or not there is a route to it: a cross
            // on the ground where this body has been told to be.
            let goal = Vec3::new(want.x, at.translation.y, want.y) + lift;
            gizmos.cross(goal, 0.7, colour);
        }
        let mut anchor = at.translation + lift;
        for leg in route.legs() {
            let leg = *leg + lift;
            gizmos.line(anchor, leg, colour);
            // A corner is a decision and worth seeing as one.
            gizmos.circle(
                Isometry3d::new(leg, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                0.35,
                colour,
            );
            anchor = leg;
        }
    }
    if level < 2 {
        return;
    }
    // Only what is near the camera. The grid is nine thousand cells and drawing
    // all of them is tens of thousands of lines a frame, which does not so much
    // show you the navigation as replace the picture with it.
    let Ok(eye) = camera.single() else {
        return;
    };
    let eye = eye.translation;
    let cell = field.cell_size();
    let reach = (GRID_DRAW_RANGE / cell).ceil() as isize;
    let here = field.cell_at(eye);
    let (cx, cz) = (
        (here % crate::flow::WIDTH) as isize,
        (here / crate::flow::WIDTH) as isize,
    );
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let (x, z) = (cx + dx, cz + dz);
            if x < 0
                || z < 0
                || x >= crate::flow::WIDTH as isize
                || z >= crate::flow::WIDTH as isize
            {
                continue;
            }
            let index = z as usize * crate::flow::WIDTH + x as usize;
            let survey = field.survey_of(index);
            if !survey.walkable {
                continue;
            }
            let centre = field.centre_of(index) + lift * 0.5;
            let colour = match (survey.wet, survey.steps.is_some(), survey.room) {
                (true, ..) => Color::srgba(0.2, 0.5, 1.0, 0.9),
                // Walkable, and the sweep never reached it: an island as far as
                // the crowd is concerned.
                (false, false, _) => Color::srgba(0.6, 0.6, 0.6, 0.5),
                // How much room there is, which is the other half of why a
                // route goes where it goes. Orange is a cell touching
                // something and amber one clear of it: a route that runs
                // through the orange had no roomier way to go, and one that
                // runs through it *anyway* on open ground is `path_clearance`
                // set too low.
                (false, true, 0) => Color::srgba(1.0, 0.55, 0.2, 0.7),
                (false, true, 1) => Color::srgba(0.9, 0.85, 0.3, 0.6),
                (false, true, _) => Color::srgba(0.35, 0.85, 0.4, 0.5),
            };
            // A flat plus rather than [`Gizmos::cross`], which has a vertical
            // arm as well and turns a lawn full of cells into a lawn full of
            // asterisks. What is being read here is a map, and a map is drawn
            // in the plane it describes.
            let arm = cell * 0.25;
            gizmos.line(centre - Vec3::X * arm, centre + Vec3::X * arm, colour);
            gizmos.line(centre - Vec3::Z * arm, centre + Vec3::Z * arm, colour);
            // The edges something stands across. Drawn as a bar *between* the
            // two cells rather than as a line to the neighbour, because that is
            // where the fence actually is.
            for step in 0..FlowField::step_count() {
                if survey.blocked & (1 << step) == 0 {
                    continue;
                }
                let offset = FlowField::step_offset(step);
                let towards = Vec3::new(offset.x as f32, 0.0, offset.y as f32) * cell * 0.5;
                let across = Vec3::new(-towards.z, 0.0, towards.x);
                let middle = centre + towards;
                gizmos.line(
                    middle - across * 0.8,
                    middle + across * 0.8,
                    Color::srgb(1.0, 0.3, 0.3),
                );
            }
            if level < 3 {
                continue;
            }
            // The crowd's own field: which way a slime standing here would walk.
            let guidance = field.at(centre);
            if guidance.towards != Vec2::ZERO {
                let towards = Vec3::new(guidance.towards.x, 0.0, guidance.towards.y);
                gizmos.arrow(
                    centre,
                    centre + towards * cell * 0.4,
                    Color::srgba(1.0, 0.9, 0.3, 0.8),
                );
            }
        }
    }
}

/// Sets the debug lines up so they can be read.
///
/// Two settings, and both are about an overlay rather than a picture. Thicker
/// than the default, because the world is drawn into a low-resolution target
/// and scaled up, so a two-pixel line arrives as a smear. And drawn *in front
/// of* the world rather than depth-tested against it, because the whole
/// question this overlay answers -- "why is that Mario walking away from where
/// I sent it" -- is asked about a route that goes round the back of something.
/// A route you can only see the near half of is the half you already knew.
pub fn configure(mut store: ResMut<bevy::gizmos::config::GizmoConfigStore>) {
    let (config, _) = store.config_mut::<bevy::gizmos::config::DefaultGizmoConfigGroup>();
    config.line.width = 3.0;
    config.depth_bias = -1.0;
}

/// How far from the camera the navigation grid is drawn, in metres.
///
/// Far enough to cover what is in front of you on the lawn, near enough that
/// the overlay is a few thousand lines rather than a hundred thousand.
const GRID_DRAW_RANGE: f32 = 45.0;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A world with the castle in it, one body, and nothing else.
    fn field(at: Vec3) -> (World, Entity) {
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(GameTuning::default());
        world.init_resource::<Pathing>();
        let body = world
            .spawn((Route::default(), Transform::from_translation(at)))
            .id();
        (world, body)
    }

    fn tick(world: &mut World) {
        world.run_system_once(plan).expect("plan could not run");
    }

    /// **The budget argument, as a number.** A body that can see where it is
    /// going is answered without a search, so a field walking about on open
    /// ground costs nothing at all.
    #[test]
    fn a_body_that_can_see_its_goal_is_not_routed() {
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let (mut world, body) = field(lawn);
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(Vec2::new(lawn.x + 8.0, lawn.z), Tolls::default());
        tick(&mut world);
        let stats = world.resource::<Pathing>();
        assert_eq!(stats.searched, 0, "it searched for a walk it could see");
        assert_eq!(stats.direct, 1);
        assert!(!world.get::<Route>(body).unwrap().routed());
    }

    /// And a body that cannot is, and what it gets is corners to walk.
    #[test]
    fn a_body_with_something_in_the_way_is_given_corners() {
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let (mut world, body) = field(lawn);
        // Through the castle and out the back, which is the walk no straight
        // line answers.
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(Vec2::new(lawn.x, lawn.z - 77.0), Tolls::default());
        tick(&mut world);
        let stats = world.resource::<Pathing>();
        assert_eq!(stats.searched, 1, "{stats:?}");
        let route = world.get::<Route>(body).unwrap();
        assert!(
            route.routed() || route.lost(),
            "neither routed nor lost: {route:?}"
        );
        if route.routed() {
            let first = route.legs()[0];
            assert!(
                first.distance(lawn) > 1.0,
                "the first corner is where it already is"
            );
        }
    }

    /// The route is a cache, and a cache that is rebuilt every tick is not one.
    #[test]
    fn a_route_is_kept_rather_than_worked_out_again_every_tick() {
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let (mut world, body) = field(lawn);
        let goal = Vec2::new(lawn.x, lawn.z - 77.0);
        // Long enough that neither a whole route's refresh nor the quarter of
        // one a partial route gets can expire inside the loop below. The
        // subject here is the cache, not the clock -- whether the far side of
        // this castle happens to be reachable is `flow`'s test, not this one.
        world.resource_mut::<GameTuning>().path_refresh = 30.0;
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(goal, Tolls::default());
        tick(&mut world);
        assert_eq!(world.resource::<Pathing>().searched, 1);
        for _ in 0..20 {
            // The same want, restated every tick exactly as a decider would.
            world
                .get_mut::<Route>(body)
                .unwrap()
                .want(goal, Tolls::default());
            tick(&mut world);
            let stats = world.resource::<Pathing>();
            assert_eq!(stats.searched, 0, "it re-planned an unchanged goal");
            assert_eq!(stats.queued, 0, "it queued an unchanged goal");
        }
    }

    /// A destination that has genuinely moved is a different question.
    #[test]
    fn a_goal_that_walks_away_is_replanned_once_it_has_gone_far_enough() {
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let (mut world, body) = field(lawn);
        let goal = Vec2::new(lawn.x, lawn.z - 77.0);
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(goal, Tolls::default());
        tick(&mut world);
        // A step of the quarry, well inside the drift: nothing happens.
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(goal + Vec2::new(DRIFT * 0.5, 0.0), Tolls::default());
        tick(&mut world);
        assert_eq!(world.resource::<Pathing>().searched, 0);
        // And a real move: asked again.
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(goal + Vec2::new(DRIFT * 3.0, 0.0), Tolls::default());
        tick(&mut world);
        assert_eq!(world.resource::<Pathing>().searched, 1);
    }

    /// **Bodies going the same way are one journey, and one journey is one
    /// search.** That is most of what caching a route is for: a squad of eight
    /// whistled at one spot is eight bodies, one group and one search, not
    /// eight searches for eight versions of the same answer.
    #[test]
    fn a_group_going_one_way_is_worked_out_once_between_them() {
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(GameTuning::default());
        world.init_resource::<Pathing>();
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let goal = Vec2::new(lawn.x, lawn.z - 77.0);
        let squad: Vec<Entity> = (0..8)
            .map(|i| {
                let at = lawn + Vec3::new(i as f32 * 0.5, 0.0, (i % 3) as f32 * 0.5);
                world
                    .spawn((Route::default(), Transform::from_translation(at)))
                    .id()
            })
            .collect();
        for (i, body) in squad.iter().enumerate() {
            // Sent to spots of their own around one place, exactly as
            // `Squad::send` spreads a cluster.
            let spot = goal + Vec2::new(i as f32 * 0.6, 0.0);
            world
                .get_mut::<Route>(*body)
                .unwrap()
                .want(spot, Tolls::default());
        }
        tick(&mut world);
        let stats = world.resource::<Pathing>();
        assert_eq!(stats.queued, 8);
        assert_eq!(stats.groups, 1, "{stats:?}");
        assert_eq!(stats.searched, 1, "eight searches for one walk: {stats:?}");
        // And all eight came away with the same route.
        let legs = world.get::<Route>(squad[0]).unwrap().legs().to_vec();
        for body in &squad {
            assert_eq!(world.get::<Route>(*body).unwrap().legs(), legs.as_slice());
        }
    }

    /// A group is a thing that moves together, and past ten that stops being a
    /// useful description of what is on screen. Fourteen is seven and seven --
    /// evenly, rather than ten and four.
    #[test]
    fn a_group_too_big_to_be_one_divides_itself_evenly() {
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(GameTuning::default());
        world.init_resource::<Pathing>();
        world.resource_mut::<GameTuning>().path_budget = 8.0;
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let goal = Vec2::new(lawn.x, lawn.z - 77.0);
        let squad: Vec<Entity> = (0..14)
            .map(|i| {
                let at = lawn + Vec3::new(i as f32 * 0.3, 0.0, 0.0);
                world
                    .spawn((Route::default(), Transform::from_translation(at)))
                    .id()
            })
            .collect();
        for body in &squad {
            world
                .get_mut::<Route>(*body)
                .unwrap()
                .want(goal, Tolls::default());
        }
        tick(&mut world);
        let stats = world.resource::<Pathing>();
        assert_eq!(stats.groups, 2, "{stats:?}");
        assert_eq!(stats.searched, 2, "{stats:?}");
        // Seven and seven, read off the lanes: each group spreads its members
        // evenly from one side of the route to the other, so there is one body
        // at each edge of each group.
        let edges = squad
            .iter()
            .filter(|body| {
                let lane = world.get::<Route>(**body).unwrap().lane;
                lane == -1.0 || lane == 1.0
            })
            .count();
        assert_eq!(edges, 4, "the two groups are not the same size");
    }

    /// Two bodies with genuinely different problems are two problems.
    #[test]
    fn bodies_going_different_ways_are_not_lumped_together() {
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(GameTuning::default());
        world.init_resource::<Pathing>();
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        for goal in [
            Vec2::new(lawn.x, lawn.z - 77.0),
            Vec2::new(lawn.x + 60.0, lawn.z - 60.0),
        ] {
            let body = world
                .spawn((Route::default(), Transform::from_translation(lawn)))
                .id();
            world
                .get_mut::<Route>(body)
                .unwrap()
                .want(goal, Tolls::default());
        }
        tick(&mut world);
        let stats = world.resource::<Pathing>();
        assert_eq!(stats.groups, 2, "{stats:?}");
    }

    /// **The promise the frame rate rests on.** However many bodies want
    /// routing, a tick serves the number the console says and no more -- and
    /// the ones it passed over are the ones it serves next.
    #[test]
    fn a_tick_serves_the_budget_and_the_rest_wait_their_turn() {
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(GameTuning::default());
        world.init_resource::<Pathing>();
        world.resource_mut::<GameTuning>().path_budget = 2.0;
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let goal = Vec2::new(lawn.x, lawn.z - 77.0);
        let bodies: Vec<Entity> = (0..9)
            .map(|i| {
                let at = lawn + Vec3::new(i as f32 * 0.4, 0.0, 0.0);
                world
                    .spawn((Route::default(), Transform::from_translation(at)))
                    .id()
            })
            .collect();
        let mut served = std::collections::HashSet::new();
        for _ in 0..6 {
            for body in &bodies {
                world
                    .get_mut::<Route>(*body)
                    .unwrap()
                    .want(goal, Tolls::default());
            }
            tick(&mut world);
            assert!(
                world.resource::<Pathing>().searched <= 2,
                "the budget did not bind: {:?}",
                world.resource::<Pathing>()
            );
            for body in &bodies {
                if world.get::<Route>(*body).unwrap().planned_for.is_some() {
                    served.insert(*body);
                }
            }
        }
        // Six ticks at two a tick is more than nine bodies' worth, and the
        // round robin is what makes that reach all of them rather than the same
        // two over and over.
        assert_eq!(
            served.len(),
            bodies.len(),
            "the queue starved somebody: {} of {}",
            served.len(),
            bodies.len()
        );
    }

    /// Corners are dropped as they are walked past, so the follower never has
    /// to keep an index into somebody else's list.
    #[test]
    fn walking_past_a_corner_takes_it_off_the_route() {
        // A flat lawn with nothing on it, so the visibility half of "reached"
        // is always satisfied and what is left under test is the order.
        let corners = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        let level =
            crate::level::LevelData::new(corners.to_vec(), vec![[0, 1, 2], [0, 2, 3]], Vec::new());
        let field = FlowField::new(&level);
        let mut route = Route {
            legs: vec![
                Vec3::new(0.0, 0.0, 10.0),
                Vec3::new(0.0, 0.0, 20.0),
                Vec3::new(0.0, 0.0, 30.0),
            ],
            ..Route::default()
        };
        assert_eq!(
            route.leg(&field, Vec3::ZERO, 0.0),
            Some(Vec3::new(0.0, 0.0, 10.0))
        );
        // Standing on the first corner, the second is what is left.
        assert_eq!(
            route.leg(&field, Vec3::new(0.0, 0.0, 10.2), 0.0),
            Some(Vec3::new(0.0, 0.0, 20.0))
        );
        // **Corners come off the front and only off the front.** Standing where
        // the last one is, having never reached the one before it, the route
        // still says to walk the one before it -- because a body that skips
        // ahead to whichever corner it happens to be nearest is a body cutting
        // the corner it was routed round, through whatever the route was
        // avoiding.
        assert_eq!(
            route.leg(&field, Vec3::new(0.0, 0.0, 30.0), 0.0),
            Some(Vec3::new(0.0, 0.0, 20.0))
        );
        // Walked in order, it runs out, and a body with no corners left is a
        // body steering at its goal again.
        route.leg(&field, Vec3::new(0.0, 0.0, 20.0), 0.0);
        assert_eq!(route.leg(&field, Vec3::new(0.0, 0.0, 30.0), 0.0), None);
        assert!(!route.routed());
    }

    /// **Near is not arrived when there is a wall in between.**
    ///
    /// The bug this pins is not obvious from the code and is unmistakable on
    /// screen: a Mario walks to the mouth of a courtyard, turns round, walks
    /// back in, and does it again for ever. A corner sits at a cell's middle,
    /// which can be most of a metre from a wall; a body is held a radius clear
    /// of the same wall from the other side; so a body pressed against the
    /// inside of a wall is inside [`REACHED`] of a corner on the outside of it,
    /// counts it as turned, and moves on to the leg after -- which is back the
    /// way it came.
    #[test]
    fn a_corner_on_the_far_side_of_a_wall_is_not_a_corner_that_has_been_turned() {
        // A lawn with one wall standing down the middle of it.
        let mut vertices = vec![
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
            Vec3::new(0., 0., -20.),
            Vec3::new(0., 0., 20.),
            Vec3::new(0., 6., 20.),
            Vec3::new(0., 6., -20.),
        ];
        vertices.truncate(8);
        let level = crate::level::LevelData::new(
            vertices,
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            Vec::new(),
        );
        let field = FlowField::new(&level);
        // Standing a body's width from the wall, with a corner the same again
        // on the other side of it -- well inside `REACHED` in a straight line,
        // and not somewhere this body has been.
        let here = Vec3::new(-0.6, 0.0, 0.0);
        let beyond = Vec3::new(0.6, 0.0, 0.0);
        assert!(
            Vec2::new(beyond.x, beyond.z).distance(Vec2::new(here.x, here.z)) < REACHED,
            "the staging does not put the corner within reach at all"
        );
        let mut route = Route {
            legs: vec![beyond, Vec3::new(0.6, 0.0, 30.0)],
            ..Route::default()
        };
        assert_eq!(
            route.leg(&field, here, 0.0),
            Some(beyond),
            "it turned a corner on the far side of a wall"
        );
        // And the same corner, from the same side of the wall, is turned.
        let mut route = Route {
            legs: vec![Vec3::new(-1.4, 0.0, 0.0), Vec3::new(-1.4, 0.0, 30.0)],
            ..Route::default()
        };
        assert_eq!(
            route.leg(&field, here, 0.0),
            Some(Vec3::new(-1.4, 0.0, 30.0)),
            "a corner it is standing on was not counted"
        );
    }

    /// **A squad sent the same way walks abreast rather than in single file.**
    ///
    /// Every body handed the same route is handed the same corners, so they all
    /// steer at one point and `enemy::spread` turns that into a queue. The lane
    /// is the one number that breaks the tie, and this is the shape of what it
    /// does: the same corner is a different point for each of them, on a line
    /// across the route, `path_spread` apart.
    #[test]
    fn a_lane_is_a_place_in_a_line_across_the_route() {
        let corners = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        let level =
            crate::level::LevelData::new(corners.to_vec(), vec![[0, 1, 2], [0, 2, 3]], Vec::new());
        let field = FlowField::new(&level);
        // A leg running due north, with another beyond it so this one is not
        // the last -- the last corner is each body's own spot and is not laned.
        let legs = vec![Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, 30.0)];
        let spread = 1.2;
        let aims: Vec<Vec3> = [-1.0, 0.0, 1.0]
            .iter()
            .map(|lane| {
                let mut route = Route {
                    legs: legs.clone(),
                    lane: *lane,
                    ..Route::default()
                };
                route
                    .leg(&field, Vec3::new(0.0, 0.0, -20.0), spread)
                    .expect("a route with corners left gave nowhere to walk")
            })
            .collect();
        // Across the leg, which runs north: so they differ in x and not in z.
        for aim in &aims {
            assert!(
                (aim.z - 10.0).abs() < 1e-3,
                "the lane moved it along the leg: {aim:?}"
            );
        }
        assert!(
            (aims[1].x - 0.0).abs() < 1e-3,
            "the middle of the line is the route"
        );
        assert!(
            (aims[0].x + aims[2].x).abs() < 1e-3
                && (aims[0].x.abs() - spread).abs() < 1e-3
                && (aims[2].x.abs() - spread).abs() < 1e-3,
            "neighbours are not {spread} m apart on either side: {aims:?}"
        );
        // **Not faded, and not measured from the body.** Taken across the
        // body's own bearing instead of across the leg, the offset swings round
        // as the corner is approached and has to be faded out to stop the body
        // orbiting it -- which collapses the whole group onto the corner
        // exactly where it most wants to still be a group. Walked right up to
        // the corner, the outside body is still out on its own line.
        let mut route = Route {
            legs: legs.clone(),
            lane: 1.0,
            ..Route::default()
        };
        let close = route
            .leg(&field, Vec3::new(-spread, 0.0, 6.0), spread)
            .expect("nowhere to walk");
        assert!(
            (close.x.abs() - spread).abs() < 1e-3,
            "the lane faded out as it closed: {close:?}"
        );
        // And the last corner of all is where it was actually sent.
        let mut route = Route {
            legs: vec![Vec3::new(0.0, 0.0, 10.0)],
            lane: 1.0,
            ..Route::default()
        };
        assert_eq!(
            route.leg(&field, Vec3::new(0.0, 0.0, -20.0), spread),
            Some(Vec3::new(0.0, 0.0, 10.0))
        );
    }

    /// Told to stop, a body drops the route rather than walking the last one it
    /// was given.
    #[test]
    fn clearing_the_want_clears_the_walk() {
        let lawn = Vec3::new(-13.28, 2.6, 46.64);
        let (mut world, body) = field(lawn);
        world
            .get_mut::<Route>(body)
            .unwrap()
            .want(Vec2::new(lawn.x, lawn.z - 77.0), Tolls::default());
        tick(&mut world);
        world.get_mut::<Route>(body).unwrap().clear();
        tick(&mut world);
        let route = world.get::<Route>(body).unwrap();
        assert!(!route.routed() && route.planned_for.is_none(), "{route:?}");
    }
}
