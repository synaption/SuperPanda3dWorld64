//! Graph search, shared by everything in this game that has to get somewhere.
//!
//! Two algorithms, and they are here rather than in their callers because the
//! callers are otherwise nothing alike. [`flow`](crate::flow) sweeps a grid of
//! ninety-six squared cells so a crowd of thousands can read its way to the
//! player out of an array. [`pylon`](crate::pylon) floods a handful of masts
//! wired to each other by line of sight, so that power spreads out from the
//! machines that make it. One is a lattice and the other is a scattering of
//! points on a lawn, and underneath they are the same breadth-first walk over
//! "what touches what".
//!
//! The graph is never built. Both callers already know their own edges --
//! [`crate::flow::FlowField::passable`] is a bitmask test and three array
//! reads, and a pylon's neighbours are a range check and a ray -- so [`flood`]
//! asks for them through a closure rather than being handed a structure that
//! would have to be assembled, kept in step, and paid for on every rebuild.
//!
//! [`astar`] is the third, and it exists because the other two answer the wrong
//! question for one body going one place. [`flood`] is *every* node's distance
//! from a source: unbeatable when a thousand enemies all want the same
//! destination, and absurd when one Mario wants a route to one ball -- it sweeps
//! nine thousand cells to use forty of them. A* sweeps outward from the start
//! but always pops whichever open node looks most promising, so on the castle
//! it settles a few hundred cells instead of all of them, and it is asked for
//! *one* body at a time.
//!
//! **Nothing in this game may run a search per unit per frame, and [`astar`]
//! is built so that it cannot.** Every call takes a `budget` of nodes it may
//! settle, and a call that runs out returns the best partial route it found
//! rather than nothing -- so the caller gets a body walking usefully in the
//! right direction instead of a body standing still or a frame spent proving
//! there is no way through. [`crate::path`] is the other half of the rule: it
//! meters how many of these run in a tick at all.
//!
//! [`tour`] is the last: not "how do I get there" but "what order should I
//! visit these in", which is the travelling salesman, and which the pylon
//! network needs to send one supply packet round every mast it has. It is
//! nearest-neighbour improved by 2-opt, because a network is a dozen or two
//! stops and an exact answer to a problem this shape is not worth a frame.

/// A node the sweep never reached, in the units [`Flood::steps`] counts in.
pub const UNREACHED: u32 = u32::MAX;

/// What one breadth-first sweep found: how far every node is from the nearest
/// source, and which neighbour it was reached from.
///
/// The parents are what makes this more than a distance field. A flow field
/// only ever wants "which way is downhill", which the distances alone answer,
/// but anything routing a *thing* along the graph -- a packet crossing a pylon
/// network -- wants the actual chain of nodes, and reconstructing it from
/// distances means guessing at ties. Kept rather than recomputed, because the
/// sweep already knew.
pub struct Flood {
    /// Steps from the nearest source, or [`UNREACHED`].
    pub steps: Vec<u32>,
    /// The node each one was first reached from, or [`UNREACHED`] for a source
    /// and for anything the sweep never got to.
    from: Vec<u32>,
}

impl Flood {
    /// How many steps `node` is from the nearest source, or `None` if the
    /// sweep never reached it.
    pub fn steps(&self, node: usize) -> Option<u32> {
        match self.steps.get(node) {
            Some(&UNREACHED) | None => None,
            Some(&steps) => Some(steps),
        }
    }

    /// Whether the sweep reached `node` at all -- which for a pylon network is
    /// the whole question of whether it has power.
    pub fn reached(&self, node: usize) -> bool {
        self.steps(node).is_some()
    }

    /// The chain from the source that reached `node` down to `node` itself,
    /// source first. Empty when it was never reached.
    ///
    /// Walked backwards along the parents and then turned round, which is the
    /// only direction the sweep recorded: a node knows what reached it, and a
    /// source has no idea which of its neighbours will end up hanging off it.
    pub fn path(&self, node: usize) -> Vec<usize> {
        if !self.reached(node) {
            return Vec::new();
        }
        let mut chain = vec![node];
        let mut here = node;
        while self.from[here] != UNREACHED {
            here = self.from[here] as usize;
            chain.push(here);
            // A parent chain cannot be longer than the graph, and one that is
            // says the caller handed out edges that changed under the sweep.
            // Bailing beats looping forever inside a frame.
            if chain.len() > self.steps.len() {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// The next node to move to on the way *out* from the sources, following
    /// the chain that reached `node`.
    ///
    /// The first hop of [`Self::path`] without building the vector, for callers
    /// stepping one edge at a time.
    pub fn first_hop(&self, node: usize) -> Option<usize> {
        let chain = self.path(node);
        chain.get(1).copied()
    }
}

/// What one A* search found: the chain of nodes from the start to the end it
/// settled on, and what it cost to find out.
///
/// The counters are not diagnostics for their own sake. A search that ran out
/// of budget is the one case a caller has to *do* something about -- walk the
/// partial route and ask again shortly, rather than treat it as the answer --
/// and [`crate::path`] puts both numbers on the debug overlay, because a route
/// that is quietly being truncated every time looks from outside exactly like a
/// unit that keeps changing its mind.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// Start first, end last. Never empty for a search that returned at all.
    pub nodes: Vec<usize>,
    /// What walking it costs, in whatever units `neighbours` handed out.
    pub cost: f32,
    /// How many nodes the search settled before it stopped.
    pub settled: usize,
    /// Whether the search ran out of *ground* rather than out of budget.
    ///
    /// **The difference between "I could not find it in time" and "it is not
    /// there", and only the caller can act on it.** A [`Self::partial`] route
    /// means the chain stops short; on its own that says nothing about why, and
    /// the honest thing to do about a budget that ran out is to walk as far as
    /// it got and ask again from there. But a search that emptied its open set
    /// has settled every cell it could ever reach, and the goal was not among
    /// them -- asking again from anywhere on this side of the gap will settle
    /// the same cells and fail the same way, for ever. See
    /// [`crate::path::Route::unreachable`].
    pub exhausted: bool,
    /// Whether [`Self::nodes`] stops short of the goal.
    ///
    /// True when the budget ran out, in which case the chain runs to whichever
    /// node came nearest the goal by the heuristic. A partial route is worth
    /// having: it is by construction a walk that gets closer, and the caller
    /// asks again from wherever it ends up.
    pub partial: bool,
}

/// The working memory of an A* search, kept between calls.
///
/// **Three arrays the size of the graph, and a search that reused nothing would
/// allocate and zero all three every time it ran.** On the navigation grid that
/// is nine thousand cells, three times over, to settle a few hundred of them --
/// more work spent preparing than searching. So the caller keeps one of these
/// (a `Local` on the system that plans routes) and hands it back.
///
/// Clearing it is a counter rather than a fill. Every entry carries the number
/// of the search that wrote it, and a stale stamp reads as "never visited",
/// so starting a search costs one increment instead of nine thousand writes.
#[derive(Default)]
pub struct Search {
    cost: Vec<f32>,
    from: Vec<u32>,
    stamp: Vec<u32>,
    epoch: u32,
    open: std::collections::BinaryHeap<Ranked>,
}

impl Search {
    /// Readies the scratch for a graph of `count` nodes and forgets the last
    /// search.
    fn begin(&mut self, count: usize) {
        if self.cost.len() != count {
            self.cost = vec![0.0; count];
            self.from = vec![UNREACHED; count];
            self.stamp = vec![0; count];
            self.epoch = 0;
        }
        self.open.clear();
        // Wrapping would make every stale entry read as current, which is a
        // search that silently refuses to visit anything. Once every four
        // billion searches, pay for the fill.
        match self.epoch.checked_add(1) {
            Some(next) => self.epoch = next,
            None => {
                self.stamp.fill(0);
                self.epoch = 1;
            }
        }
    }

    /// Whether this search has seen `node` yet.
    fn seen(&self, node: usize) -> bool {
        self.stamp[node] == self.epoch
    }
}

/// One node waiting to be looked at, ordered by what it is estimated to cost.
///
/// `Ord` reversed and by `total_cmp`, so the standard max-heap pops the
/// cheapest and a NaN estimate cannot panic the comparison inside the heap --
/// a heuristic that returned one would simply sort last.
#[derive(Clone, Copy, PartialEq)]
struct Ranked {
    estimate: f32,
    node: u32,
}

impl Eq for Ranked {}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The cheapest way from `start` to `goal`, or the best start of one.
///
/// `neighbours` is asked, for each node the search settles, which nodes may be
/// stepped to from it and what that step costs -- the same shape [`flood`] asks
/// its edges in, with a price attached. `heuristic` estimates what is left to
/// walk from a node to the goal.
///
/// **The heuristic must never overstate what is left**, or the first route
/// found is not the cheapest one. On a grid that means straight-line or octile
/// distance and nothing cleverer: any penalty the edges add -- water, a climb --
/// only ever makes the real cost larger, so an estimate that ignores them stays
/// a lower bound by construction. That is worth writing down because it is the
/// one property that makes the answer an answer rather than a plausible walk.
///
/// `budget` is how many nodes it may settle before it gives up. It is a
/// guarantee about the frame rather than a tuning knob: an unreachable goal on
/// a nine-thousand-cell grid is a search that settles *every walkable cell*
/// proving it, and a game cannot pay that on the tick somebody points at the
/// far side of the moat. Out of budget, what comes back is the chain to
/// whichever node the heuristic liked best -- a real walk in the right
/// direction, marked [`Found::partial`] so the caller knows to ask again.
///
/// `None` only when `start` or `goal` is out of range, or when the reachable
/// part of the graph was searched out and the goal was not in it.
pub fn astar<I>(
    search: &mut Search,
    count: usize,
    start: usize,
    goal: usize,
    budget: usize,
    mut neighbours: impl FnMut(usize) -> I,
    heuristic: impl Fn(usize) -> f32,
) -> Option<Found>
where
    I: IntoIterator<Item = (usize, f32)>,
{
    if start >= count || goal >= count {
        return None;
    }
    search.begin(count);
    search.stamp[start] = search.epoch;
    search.cost[start] = 0.0;
    search.from[start] = UNREACHED;
    search.open.push(Ranked {
        estimate: heuristic(start),
        node: start as u32,
    });
    // The nearest miss, in case the budget runs out. Seeded with the start, so
    // there is always something to hand back -- a chain of one, which is a body
    // that has been told to stay where it is rather than a body with no answer.
    let mut nearest = (heuristic(start), start);
    let mut settled = 0;
    while let Some(Ranked { estimate, node }) = search.open.pop() {
        let here = node as usize;
        if here == goal {
            return Some(Found {
                nodes: chain(search, start, goal),
                cost: search.cost[goal],
                settled,
                partial: false,
                exhausted: false,
            });
        }
        // A node can be pushed more than once, when a cheaper way to it turns
        // up after it was first queued. The stale copies are still in the heap
        // -- taking them out would cost a search of it -- so they are thrown
        // away here instead, by noticing that the estimate they were filed
        // under is no longer the one the node holds.
        if estimate > search.cost[here] + heuristic(here) + 1e-4 {
            continue;
        }
        settled += 1;
        if settled > budget {
            break;
        }
        for (there, step) in neighbours(here) {
            if there >= count || !step.is_finite() || step < 0.0 {
                continue;
            }
            let cost = search.cost[here] + step;
            if search.seen(there) && cost >= search.cost[there] {
                continue;
            }
            search.stamp[there] = search.epoch;
            search.cost[there] = cost;
            search.from[there] = here as u32;
            let left = heuristic(there);
            if left < nearest.0 {
                nearest = (left, there);
            }
            search.open.push(Ranked {
                estimate: cost + left,
                node: there as u32,
            });
        }
    }
    // Searched out, or out of budget. Either way the useful answer is the same
    // one: walk to whatever came nearest.
    if nearest.1 == start && start != goal {
        return None;
    }
    Some(Found {
        nodes: chain(search, start, nearest.1),
        cost: search.cost[nearest.1],
        settled,
        partial: true,
        // The loop breaks on `settled > budget` and falls out of the bottom
        // when the open set empties, so this is exactly "the heap ran dry".
        exhausted: settled <= budget,
    })
}

/// Walks the parents back from `node` to `start` and turns the chain round.
fn chain(search: &Search, start: usize, node: usize) -> Vec<usize> {
    let mut nodes = vec![node];
    let mut here = node;
    while here != start {
        let parent = search.from[here];
        if parent == UNREACHED || nodes.len() > search.from.len() {
            break;
        }
        here = parent as usize;
        nodes.push(here);
    }
    nodes.reverse();
    nodes
}

/// Breadth-first from every source at once.
///
/// `count` is how many nodes the graph has, `sources` the nodes that start at
/// zero, and `neighbours` is asked, for each node the sweep pops, which nodes
/// may be stepped to from it. Refusing an edge there is the whole of pathing:
/// a sweep that never crosses a wall cannot route anybody through one.
///
/// Multiple sources cost nothing extra and are what a pylon network is: several
/// machines making power, and every mast wanting its distance to the nearest
/// one rather than to a chosen one.
///
/// A plain queue rather than a priority queue, so every edge costs one step.
/// Both callers want hops -- cells for the crowd, masts for the network -- and
/// a real metric would buy neither of them anything and cost a heap.
pub fn flood<I>(
    count: usize,
    sources: impl IntoIterator<Item = usize>,
    mut neighbours: impl FnMut(usize) -> I,
) -> Flood
where
    I: IntoIterator<Item = usize>,
{
    let mut steps = vec![UNREACHED; count];
    let mut from = vec![UNREACHED; count];
    let mut queue = std::collections::VecDeque::new();
    for source in sources {
        if source < count && steps[source] == UNREACHED {
            steps[source] = 0;
            queue.push_back(source);
        }
    }
    while let Some(here) = queue.pop_front() {
        let next = steps[here] + 1;
        for there in neighbours(here) {
            if there >= count || steps[there] != UNREACHED {
                continue;
            }
            steps[there] = next;
            from[there] = here as u32;
            queue.push_back(there);
        }
    }
    Flood { steps, from }
}

/// An order to visit every one of `count` nodes in, starting and ending at
/// node zero.
///
/// The travelling salesman, answered the way a game should answer it: a
/// nearest-neighbour walk for a first guess, then 2-opt -- repeatedly reversing
/// a stretch of the tour wherever that shortens it -- until nothing improves.
/// Nearest-neighbour alone is famously bad at the end, where it has to run back
/// across everything it skipped; 2-opt takes most of that out and is a few
/// dozen lines.
///
/// `cost` is asked for the distance between two nodes and may be anything
/// consistent: a straight line for masts on a lawn, a hop count for something
/// routed over a graph. It is called often, so it should be cheap -- a lookup
/// or a subtraction, not a ray cast.
///
/// Bounded rather than run to convergence: the loop stops improving long before
/// [`TOUR_PASSES`] on the network sizes this is used at, and a cap means a
/// pathological cost function costs a bounded amount of one frame instead of
/// the frame.
pub fn tour(count: usize, cost: impl Fn(usize, usize) -> f32) -> Vec<usize> {
    if count <= 2 {
        return (0..count).collect();
    }
    let mut visited = vec![false; count];
    let mut order = Vec::with_capacity(count);
    let mut here = 0;
    visited[0] = true;
    order.push(0);
    for _ in 1..count {
        let mut best = None;
        for (candidate, seen) in visited.iter().enumerate() {
            if *seen {
                continue;
            }
            let distance = cost(here, candidate);
            if best.is_none_or(|(_, shortest)| distance < shortest) {
                best = Some((candidate, distance));
            }
        }
        let Some((next, _)) = best else { break };
        visited[next] = true;
        order.push(next);
        here = next;
    }
    // 2-opt over the closed tour. Reversing `order[i..=j]` replaces the two
    // edges entering and leaving that stretch with the two that cross the other
    // way; if that is shorter, the crossing is gone. A tour with no crossings
    // left is what this converges to.
    let closed = |order: &[usize], index: usize| order[(index + 1) % order.len()];
    for _ in 0..TOUR_PASSES {
        let mut improved = false;
        for i in 0..order.len() - 1 {
            for j in i + 1..order.len() {
                let (a, b) = (order[i], closed(&order, i));
                let (c, d) = (order[j], closed(&order, j));
                if b == c || a == d {
                    continue;
                }
                let before = cost(a, b) + cost(c, d);
                let after = cost(a, c) + cost(b, d);
                if after + 1e-4 < before {
                    order[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    order
}

/// How many times [`tour`] sweeps the whole tour looking for a crossing to
/// undo. Each pass is `count²` cost lookups and the improvement is all in the
/// first two or three; this is a ceiling, not a target.
const TOUR_PASSES: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    /// A four-by-four lattice, with an optional wall down the middle.
    fn lattice(walled: bool) -> impl Fn(usize) -> Vec<usize> {
        move |here: usize| {
            let (x, y) = (here % 4, here / 4);
            let mut out = Vec::new();
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if !(0..4).contains(&nx) || !(0..4).contains(&ny) {
                    continue;
                }
                // The wall stands between columns one and two, all the way
                // down, so the right half is only reachable round nothing at
                // all -- it is not reachable.
                if walled && (x.min(nx as usize) == 1 && x.max(nx as usize) == 2) {
                    continue;
                }
                out.push(ny as usize * 4 + nx as usize);
            }
            out
        }
    }

    #[test]
    fn a_sweep_counts_hops_from_its_source() {
        let found = flood(16, [0], lattice(false));
        assert_eq!(found.steps(0), Some(0));
        // Manhattan distance on a four-neighbour lattice.
        assert_eq!(found.steps(3), Some(3));
        assert_eq!(found.steps(15), Some(6));
        assert!(found.reached(15));
    }

    #[test]
    fn a_wall_the_neighbours_refuse_is_a_wall_the_sweep_cannot_cross() {
        let found = flood(16, [0], lattice(true));
        assert!(found.reached(1), "the near half should still be swept");
        assert!(!found.reached(2), "the sweep walked through the wall");
        assert_eq!(found.steps(2), None);
        assert!(found.path(2).is_empty());
    }

    #[test]
    fn a_path_runs_from_the_source_to_the_node_one_step_at_a_time() {
        let found = flood(16, [0], lattice(false));
        let chain = found.path(15);
        assert_eq!(chain.first(), Some(&0));
        assert_eq!(chain.last(), Some(&15));
        assert_eq!(chain.len(), 7, "{chain:?}");
        // Every hop is an edge of the lattice.
        for pair in chain.windows(2) {
            assert!(
                lattice(false)(pair[0]).contains(&pair[1]),
                "{pair:?} is not an edge"
            );
        }
        assert_eq!(found.first_hop(15), Some(chain[1]));
    }

    #[test]
    fn several_sources_are_swept_at_once_and_the_nearest_one_wins() {
        let found = flood(16, [0, 15], lattice(false));
        // The far corner is a source itself now rather than six steps away.
        assert_eq!(found.steps(15), Some(0));
        // And the middle is reached from whichever corner is nearer.
        assert_eq!(found.steps(5), Some(2));
        assert_eq!(found.steps(10), Some(2));
    }

    /// The four-neighbour lattice again, priced at one per step.
    fn priced(walled: bool) -> impl Fn(usize) -> Vec<(usize, f32)> {
        let edges = lattice(walled);
        move |here| edges(here).into_iter().map(|there| (there, 1.0)).collect()
    }

    /// Manhattan distance to the far corner, which on a four-neighbour lattice
    /// with unit steps is exact and therefore admissible.
    fn towards(goal: usize) -> impl Fn(usize) -> f32 {
        move |node| {
            let (x, y) = ((node % 4) as f32, (node / 4) as f32);
            let (gx, gy) = ((goal % 4) as f32, (goal / 4) as f32);
            (x - gx).abs() + (y - gy).abs()
        }
    }

    #[test]
    fn a_search_finds_the_shortest_way_across_the_lattice() {
        let mut search = Search::default();
        let found = astar(&mut search, 16, 0, 15, 1000, priced(false), towards(15)).unwrap();
        assert!(!found.partial);
        assert_eq!(found.nodes.first(), Some(&0));
        assert_eq!(found.nodes.last(), Some(&15));
        assert_eq!(found.cost, 6.0);
        assert_eq!(found.nodes.len(), 7, "{found:?}");
        for pair in found.nodes.windows(2) {
            assert!(
                lattice(false)(pair[0]).contains(&pair[1]),
                "{pair:?} is not an edge"
            );
        }
        // And it looked at less of the graph than a sweep would have. This is
        // the whole reason it exists beside `flood`, so it is worth an
        // assertion rather than a comment.
        assert!(
            found.settled < 16,
            "it settled the whole lattice: {found:?}"
        );
    }

    /// Cost is not distance, which is the whole reason the crowd's own sweep
    /// could not be reused for this: a flood counts hops, and a route through
    /// the moat is not the same length as a route round it.
    #[test]
    fn the_cheapest_way_is_taken_rather_than_the_shortest() {
        // One cell in the middle of the lattice made ten times as expensive to
        // step into. Every route from corner to corner is six steps, so the
        // only thing that can steer the answer away from it is the price.
        let toll = |here: usize| -> Vec<(usize, f32)> {
            lattice(false)(here)
                .into_iter()
                .map(|there| (there, if there == 5 { 10.0 } else { 1.0 }))
                .collect()
        };
        let mut search = Search::default();
        let found = astar(&mut search, 16, 0, 15, 1000, toll, towards(15)).unwrap();
        assert!(!found.partial);
        assert_eq!(found.cost, 6.0, "it paid the toll: {found:?}");
        assert!(
            !found.nodes.contains(&5),
            "it walked through the expensive cell: {found:?}"
        );
    }

    #[test]
    fn a_search_that_cannot_get_there_says_so_rather_than_guessing() {
        let mut search = Search::default();
        // The right half is walled off entirely. What comes back is the best
        // approach to the wall -- a real walk, marked as not the whole answer.
        let found = astar(&mut search, 16, 0, 3, 1000, priced(true), towards(3)).unwrap();
        assert!(found.partial, "it claimed to have crossed the wall");
        assert_eq!(found.nodes.first(), Some(&0));
        assert!(
            found.nodes.last().is_some_and(|node| node % 4 <= 1),
            "it ended up past the wall: {found:?}"
        );
    }

    /// **The guarantee the frame rate rests on**: a search may be stopped, and
    /// what it gives back when it is stopped is still a walk in the right
    /// direction rather than nothing.
    #[test]
    fn a_search_out_of_budget_hands_back_the_best_start_it_found() {
        let mut search = Search::default();
        let found = astar(&mut search, 16, 0, 15, 2, priced(false), towards(15)).unwrap();
        assert!(found.partial, "two nodes was enough for the whole lattice?");
        assert!(found.settled <= 3, "the budget was not a budget: {found:?}");
        assert_eq!(found.nodes.first(), Some(&0));
        let end = *found.nodes.last().unwrap();
        assert!(
            towards(15)(end) < towards(15)(0),
            "the partial route goes the wrong way: {found:?}"
        );
    }

    #[test]
    fn the_scratch_is_reusable_and_a_stale_search_does_not_leak_into_the_next() {
        let mut search = Search::default();
        let first = astar(&mut search, 16, 0, 15, 1000, priced(false), towards(15)).unwrap();
        // The same scratch, a different question, twice over -- the second of
        // which is the one that would read the first's parents if the epoch
        // stamp were not doing its job.
        let blocked = astar(&mut search, 16, 0, 3, 1000, priced(true), towards(3)).unwrap();
        assert!(blocked.partial);
        let again = astar(&mut search, 16, 0, 15, 1000, priced(false), towards(15)).unwrap();
        assert_eq!(first, again);
    }

    #[test]
    fn a_search_to_where_it_already_is_is_a_chain_of_one() {
        let mut search = Search::default();
        let found = astar(&mut search, 16, 5, 5, 1000, priced(false), towards(5)).unwrap();
        assert_eq!(found.nodes, vec![5]);
        assert_eq!(found.cost, 0.0);
        assert!(!found.partial);
        // And a node that is not in the graph at all is not an answer.
        assert!(astar(&mut search, 16, 0, 99, 1000, priced(false), towards(0)).is_none());
    }

    #[test]
    fn a_tour_of_points_on_a_circle_comes_out_as_the_circle() {
        // Eight points evenly round a circle. The shortest tour is the one that
        // walks them in order; anything that crosses the middle is longer.
        let points: Vec<_> = (0..8)
            .map(|i| {
                let angle = i as f32 / 8.0 * std::f32::consts::TAU;
                (angle.cos(), angle.sin())
            })
            .collect();
        // Shuffled into an order no walk would pick, so a tour that came out
        // right cannot have come out right by being handed the answer.
        let scrambled: Vec<_> = [0usize, 3, 6, 1, 4, 7, 2, 5]
            .iter()
            .map(|&i| points[i])
            .collect();
        let cost = |a: usize, b: usize| {
            let (ax, ay) = scrambled[a];
            let (bx, by) = scrambled[b];
            ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
        };
        let order = tour(scrambled.len(), cost);
        assert_eq!(order.len(), scrambled.len(), "a stop was dropped");
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..scrambled.len()).collect::<Vec<_>>());
        let length: f32 = (0..order.len())
            .map(|i| cost(order[i], order[(i + 1) % order.len()]))
            .sum();
        // The perimeter of a regular octagon of unit radius, and nothing
        // shorter exists.
        let perimeter = 8.0 * (std::f32::consts::PI / 8.0).sin() * 2.0;
        assert!(
            length < perimeter * 1.001,
            "the tour is {length}, the circle is {perimeter}"
        );
    }

    #[test]
    fn a_tour_of_nothing_much_is_still_a_tour() {
        assert!(tour(0, |_, _| 0.0).is_empty());
        assert_eq!(tour(1, |_, _| 0.0), vec![0]);
        assert_eq!(tour(2, |_, _| 1.0), vec![0, 1]);
    }
}
