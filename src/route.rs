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
//! [`tour`] is the other half: not "how do I get there" but "what order should
//! I visit these in", which is the travelling salesman, and which the pylon
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
