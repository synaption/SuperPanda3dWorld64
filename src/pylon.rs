//! Pylons: a power network you plant across the level.
//!
//! A pylon is a mast you put down the way you put a stellarator down -- aim
//! with the crosshair, hold the key, let go -- and on its own it does nothing
//! at all. What makes it worth building is what it joins: every pylon strings a
//! beam to every other pylon within [`REACH`] that it can *see*, and power
//! floods out from the machines that make it, hop by hop, along those beams.
//!
//! Three rules, and they are the whole of it:
//!
//!   * **A beam needs line of sight.** Two masts within reach of each other are
//!     linked when nothing stands between their emitter heads --
//!     [`crate::level::LevelData::segment_hit`] against the level's own
//!     collision, taken head to head rather than foot to foot, because a beam
//!     eight metres up clears a hedge a walker would have to go round. See
//!     [`links`], which is plain arithmetic over a list of points and a
//!     visibility test, so the rule can be exercised without a window.
//!   * **Power comes from the machines.** A stellarator within reach of a mast
//!     energises it, and that mast energises whatever it can see, outward
//!     until the network runs out. That flood is
//!     [`crate::route::flood`] -- the same breadth-first walk
//!     [`crate::flow`] sweeps the castle with to tell a crowd of thousands
//!     which way the player is. A network is a scattering of masts and a flow
//!     field is a lattice of cells, and underneath they are one question:
//!     what does this touch, and what does that touch.
//!   * **The supply run visits every mast.** One packet of light leaves the
//!     source and calls at each live pylon in turn before starting over. What
//!     order to call in is the travelling salesman, answered by
//!     [`crate::route::tour`]; how to get from one call to the next is a
//!     shortest path over the links, answered by the same flood. The route is
//!     expanded once, when the network changes, into the list of masts the
//!     packet actually flies between -- see [`Network::rebuild`].
//!
//! What the player gets out of it is range, and a road. Standing near a live
//! pylon fills the jetpack bar several times faster than standing on open
//! ground, so a line of masts across a valley is a line of places you can fly
//! from; and a mast is where a Mario hands in what it picked off a dead enemy,
//! which then flies home down the beams to a machine -- see [`crate::nuclonium`],
//! which owns the balls and asks [`Network::supply_route`] for the way back.
//! Both are why the network is worth pushing outward rather than being
//! decoration around the machine that powers it.
//!
//! And a mast can be lost. Every pylon stands as a [`crate::structure::Structure`]
//! on the friendly side, which is the whole of what makes the crowd come for it:
//! [`crate::enemy::alert`] asks what side a thing is on and never asks whether it
//! can walk. A network is something to defend now rather than something you
//! finish.
//!
//! **The model is measured, never written down.** The asset pipeline exports
//! `assets/actors/pylon.blend` to `pylon.glb`, and [`measure`] reads its height
//! and footprint back out of the file's own accessor bounds. Re-exporting it
//! at another size moves the beams, the ring on the ground and the overlap test
//! with it, for the reason [`crate::stellarator::machine`] documents at
//! length: an asset somebody is free to re-export must not have a copy of its
//! own dimensions living in the game.

use crate::{
    input::InputState,
    level::LevelData,
    player::Player,
    route,
    squad::{self, GOLDEN_ANGLE},
    stellarator::{self, Stellarator},
};
use bevy::prelude::*;

// -- the mast ---------------------------------------------------------------

/// The model, named once: [`spawn`] loads it and [`measure`] reads it off disk.
const MODEL: &str = "actors/pylon.glb#Scene0";

/// The optional glTF node holding the emitter head. [`claim`] finds it to give
/// it its idle shimmer; models without a separate head still use their upper
/// edge as the beam height.
const EMITTER_NODE: &str = "Pylon Emitter";

/// How far a beam carries, in metres.
///
/// **Long enough that line of sight is the rule and range is the backstop.**
/// It was 42 m -- about a quarter of the castle grounds -- and at that length
/// the thing a player was actually planning around was the number, not the
/// terrain: masts went down in a chain at regular intervals and the hills the
/// beams were supposed to be threading between never came into it. A beam is
/// light. What ought to stop it is something standing in the way.
///
/// So it is now most of the map. The castle grounds are a bit over 160 m
/// across, and a mast on the high ground can reach very nearly any other mast
/// it can *see* -- which turns the question from "how far apart" into "what is
/// between them", and makes a ridge line a real decision rather than scenery.
///
/// Not infinite, and the reason is not tidiness. [`links`] is a pair-wise sweep
/// that runs the cheap range test first precisely so the expensive visibility
/// ray runs only for pairs that passed it; with no range at all every pair of
/// masts on the map is a ray cast into the level's collision on every rebuild.
/// A number a little larger than the world is what keeps that ordering
/// meaningful on the next, bigger level rather than only on this one.
pub const REACH: f32 = 150.0;

/// How near a live pylon the player has to be for the bar to fill faster, and
/// by how much.
///
/// A radius rather than a contact: the point is to make a stretch of ground
/// friendly, not to make the player stand on a spot.
pub const SUPPLY_RADIUS: f32 = 14.0;
pub const SUPPLY_BOOST: f32 = 4.0;

/// What the mast measures, as the file on disk describes it.
#[derive(Debug, Clone, Copy)]
pub struct Mast {
    /// Ground to the top of the emitter head.
    pub height: f32,
    /// How much ground the footing covers, which is what two pylons may not
    /// share.
    pub radius: f32,
    /// How high up the beams are strung, which is the emitter rather than the
    /// very top of the head.
    pub emitter: f32,
}

/// What the game falls back on with the model missing from the install.
///
/// Roughly the shipped file and deliberately not pinned to it, for
/// [`crate::stellarator`]'s reason: keeping a constant in step with an asset is
/// the job [`measure`] exists to take away. Only reachable in a build with no
/// pylon to draw at all, where what it buys is a preview ring of a sensible
/// size and a network whose arithmetic cannot divide by zero.
const UNMEASURED: Mast = Mast {
    height: 8.0,
    radius: 0.95,
    emitter: 7.2,
};

/// The measurement, taken once.
pub fn mast() -> &'static Mast {
    static MAST: std::sync::OnceLock<Mast> = std::sync::OnceLock::new();
    MAST.get_or_init(|| measure().unwrap_or(UNMEASURED))
}

/// Reads the mast's size out of `assets/actors/pylon.glb`.
///
/// Accessor bounds rather than a walk over vertices: every glTF accessor
/// carries `min` and `max`. Blender may leave its Y-up conversion on the node,
/// so those local bounds are transformed before they are measured. The same
/// job [`crate::stellarator::machine`] does, with one fewer subtlety -- there
/// is no authored scale to recover here, because the pylon is authored in
/// metres.
fn measure() -> Option<Mast> {
    let bytes = std::fs::read(crate::asset_path().join(MODEL.trim_end_matches("#Scene0"))).ok()?;
    let length = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    let json: serde_json::Value = serde_json::from_slice(bytes.get(20..20 + length)?).ok()?;
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut emitter = None;
    for node in json["nodes"].as_array()? {
        let Some(index) = node["mesh"].as_u64() else {
            continue;
        };
        let numbers = |key: &str, count: usize| -> Option<Vec<f32>> {
            let values = node.get(key)?.as_array()?;
            (values.len() == count).then(|| {
                values
                    .iter()
                    .map(|value| value.as_f64().unwrap_or(0.0) as f32)
                    .collect()
            })
        };
        let transform = if let Some(matrix) = numbers("matrix", 16) {
            Mat4::from_cols_array(matrix.as_slice().try_into().ok()?)
        } else {
            let translation = numbers("translation", 3).unwrap_or_else(|| vec![0.0; 3]);
            let rotation = numbers("rotation", 4).unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
            let scale = numbers("scale", 3).unwrap_or_else(|| vec![1.0; 3]);
            Mat4::from_scale_rotation_translation(
                Vec3::from_slice(&scale),
                Quat::from_array(rotation.as_slice().try_into().ok()?),
                Vec3::from_slice(&translation),
            )
        };
        let mesh = &json["meshes"][index as usize];
        for primitive in mesh["primitives"].as_array()? {
            let accessor =
                &json["accessors"][primitive["attributes"]["POSITION"].as_u64()? as usize];
            let corner = |which: &str| -> Option<Vec3> {
                let bounds = accessor[which].as_array()?;
                Some(Vec3::new(
                    bounds.first()?.as_f64()? as f32,
                    bounds.get(1)?.as_f64()? as f32,
                    bounds.get(2)?.as_f64()? as f32,
                ))
            };
            let (min, max) = (corner("min")?, corner("max")?);
            let mut part_low = Vec3::splat(f32::MAX);
            let mut part_high = Vec3::splat(f32::MIN);
            for x in [min.x, max.x] {
                for y in [min.y, max.y] {
                    for z in [min.z, max.z] {
                        let point = transform.transform_point3(Vec3::new(x, y, z));
                        part_low = part_low.min(point);
                        part_high = part_high.max(point);
                    }
                }
            }
            low = low.min(part_low);
            high = high.max(part_high);
            if mesh["name"].as_str() == Some(EMITTER_NODE) {
                // The middle of the head rather than the top of it: a beam
                // leaves the emitter, and hanging it off the point would leave
                // every link visibly missing the thing it comes out of.
                emitter = Some((part_low.y + part_high.y) * 0.5);
            }
        }
    }
    if low.x > high.x {
        return None;
    }
    Some(Mast {
        height: high.y,
        radius: low.x.abs().max(high.x).max(low.z.abs()).max(high.z),
        // A file with no emitter node in it still has a top, and a beam strung
        // from just under it is a great deal better than no pylon at all.
        emitter: emitter.unwrap_or(high.y * 0.9),
    })
}

// -- the pieces -------------------------------------------------------------

/// A planted mast, and how much ground it stands on.
///
/// The radius is carried rather than read back off the transform for
/// [`crate::stellarator::Stellarator`]'s reason: the overlap test has to agree
/// with the ring the player was looking at when they let go.
#[derive(Component)]
pub struct Pylon {
    pub radius: f32,
}

/// The one placement preview: a ring on the ground that follows the crosshair
/// and is hidden the rest of the time.
///
/// Spawned once at startup and never despawned, the way the whistle's circle
/// and the stellarator's site are, so changing level does not take the thing
/// you build with away.
#[derive(Component)]
pub struct BuildSite;

/// The ring itself. Its material is the answer to "will this go here, and will
/// it be joined to anything".
#[derive(Component)]
pub struct SiteRing;

/// One drawn link between two masts. Rebuilt wholesale whenever the network
/// changes, which is rare -- a pylon is planted by hand -- and never touched in
/// between.
#[derive(Component)]
pub struct Beam;

/// The supply packet: one mote of light making the rounds of the network.
#[derive(Component)]
pub struct Packet {
    /// How far along [`Network::run`] it is, in legs; the fraction is where it
    /// sits between one mast and the next.
    along: f32,
}

/// The emitter head of a planted mast, so it can shimmer while its mast has
/// power and stand still while it does not.
#[derive(Component)]
pub struct Emitter {
    /// Seeded off the golden angle so no two heads pulse together. The same
    /// trick the wisps and the drifting coils use, and for the same reason: a
    /// field that never repeats, out of a game with no random number generator
    /// in it, so a session stays reproducible in a test.
    phase: f32,
    /// The mast this head belongs to, found by walking up out of the scene it
    /// arrived in. Kept because the head is three entities below the thing the
    /// network has an opinion about, and asking that question every frame is a
    /// walk up a hierarchy per mast per frame.
    mast: Entity,
}

// -- the network ------------------------------------------------------------

/// One mast in the network, as the graph sees it.
pub struct Node {
    pub entity: Entity,
    /// Where it stands, on the ground.
    pub at: Vec3,
    /// Where its beams leave from.
    pub top: Vec3,
    /// Hops from the nearest machine, or `None` for a mast with no power.
    pub hops: Option<u32>,
    /// Which machine feeds this mast *directly*, as an index into
    /// [`Network::feeds`], for the masts one is close enough to reach.
    ///
    /// `None` for a mast fed through its neighbours and for a dark one, so this
    /// is exactly the set of places power enters the network. Two things want
    /// that, and both used to guess at it: [`draw`], which now strings a beam
    /// along it so a machine is visibly *part* of the network rather than a prop
    /// standing near one, and [`Network::supply_route`], which ends a shipment's
    /// flight at it.
    pub feed: Option<usize>,
}

/// The live network: who is linked to whom, who has power, and the round the
/// supply packet is flying.
///
/// A resource rather than components on the masts because every question asked
/// of it is a question about the *whole* graph -- is this one connected, what
/// order should the packet call in -- and answering those from a query would
/// mean rebuilding the graph at each asking.
#[derive(Resource, Default)]
pub struct Network {
    pub nodes: Vec<Node>,
    /// Every linked pair, each once, as indices into [`Self::nodes`].
    pub links: Vec<(usize, usize)>,
    /// The masts the packet flies between, in order, each linked to the next.
    /// Empty while there is nothing live to visit.
    pub run: Vec<usize>,
    /// Where the machines feeding the network were when it was last built.
    ///
    /// Positions rather than the count this used to be, because a shipment
    /// coming home down the beams has to be *delivered* somewhere -- see
    /// [`Self::supply_route`]. The count is still what [`relink`] watches for a
    /// change, and it is `feeds.len()`.
    pub feeds: Vec<Vec3>,
    /// Bumped on every rebuild. The beam-drawing system redraws when it
    /// changes and does nothing at all when it does not.
    pub revision: u64,
}

impl Network {
    /// Whether a mast has power.
    pub fn powered(&self, node: usize) -> bool {
        self.nodes.get(node).is_some_and(|node| node.hops.is_some())
    }

    /// Whether the mast standing as `entity` has power.
    ///
    /// The same question asked from the other end, for callers holding an
    /// entity rather than a node number -- an emitter head wanting to know
    /// whether to breathe, a console command reporting on one mast.
    pub fn powered_entity(&self, entity: Entity) -> bool {
        self.nodes
            .iter()
            .any(|node| node.entity == entity && node.hops.is_some())
    }

    /// How many masts have power.
    pub fn live(&self) -> usize {
        self.nodes.iter().filter(|node| node.hops.is_some()).count()
    }

    /// Rebuilds the whole graph from the masts and the machines that are
    /// standing.
    ///
    /// Everything downstream of the links is worked out here, once, rather than
    /// asked for as it is needed: which masts have power, and what round the
    /// packet is flying. Both cost a sweep over a graph of a few dozen nodes,
    /// and both would otherwise be recomputed every frame by whatever wanted
    /// them.
    ///
    /// `sees` is the visibility test, handed in rather than taken so that the
    /// rule can be tested without a level: [`links`] documents what it is
    /// asked.
    pub fn rebuild(
        &mut self,
        masts: Vec<(Entity, Vec3)>,
        machines: &[Vec3],
        sees: impl Fn(Vec3, Vec3) -> bool,
    ) {
        let lift = Vec3::Y * mast().emitter;
        self.nodes = masts
            .into_iter()
            .map(|(entity, at)| Node {
                entity,
                at,
                top: at + lift,
                hops: None,
                feed: None,
            })
            .collect();
        let tops: Vec<Vec3> = self.nodes.iter().map(|node| node.top).collect();
        self.links = links(&tops, &sees);
        self.feeds = machines.to_vec();

        // Power. The sources are the masts a machine can reach and see; the
        // flood carries it outward from there, and a mast the sweep never
        // reaches is a mast standing dark.
        let mut touching = Vec::new();
        for (index, top) in tops.iter().enumerate() {
            // Which machine, not merely whether one -- see [`Node::feed`]. The
            // nearest of those that can reach and see it, so a mast standing
            // between two machines draws its beam to the one it is beside.
            let feed = machines
                .iter()
                .enumerate()
                .filter(|(_, feed)| in_reach(**feed, *top) && sees(**feed, *top))
                .min_by(|(_, a), (_, b)| {
                    a.distance_squared(*top)
                        .total_cmp(&b.distance_squared(*top))
                })
                .map(|(which, _)| which);
            if feed.is_some() {
                self.nodes[index].feed = feed;
                touching.push(index);
            }
        }
        let mut wired = vec![Vec::new(); self.nodes.len()];
        for &(a, b) in &self.links {
            wired[a].push(b);
            wired[b].push(a);
        }
        let powered = route::flood(self.nodes.len(), touching, |here| wired[here].clone());
        for (index, node) in self.nodes.iter_mut().enumerate() {
            node.hops = powered.steps(index);
        }

        // The supply run: which live masts to call at, in what order, and the
        // legs between them. The tour is over the live masts only -- a dark one
        // has nothing to deliver to and no beam to fly along -- and each pair
        // of calls is joined by the shortest chain of links between them, so
        // every leg of the finished run is one beam long.
        self.run.clear();
        let live: Vec<usize> = (0..self.nodes.len()).filter(|&i| self.powered(i)).collect();
        if live.len() < 2 {
            self.revision = self.revision.wrapping_add(1);
            return;
        }
        let order = route::tour(live.len(), |a, b| {
            self.nodes[live[a]].top.distance(self.nodes[live[b]].top)
        });
        for step in 0..order.len() {
            let from = live[order[step]];
            let to = live[order[(step + 1) % order.len()]];
            let leg = route::flood(self.nodes.len(), [from], |here| wired[here].clone());
            let chain = leg.path(to);
            if chain.is_empty() {
                // Two live masts with no path between them: they are fed by
                // different machines and the network is in two pieces. The
                // packet flies the piece it started in rather than teleporting
                // across the gap.
                break;
            }
            if self.run.is_empty() {
                self.run.push(chain[0]);
            }
            self.run.extend_from_slice(&chain[1..]);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// The way home from `node`: the points a delivered ball flies through to
    /// reach a machine.
    ///
    /// **The flood is read backwards.** [`Self::rebuild`] already walked outward
    /// from the machines and wrote every mast's hop count, so the shortest way
    /// *to* a machine is simply: step to whichever neighbour has a smaller
    /// count, until the count is zero, and then leave the network for the feed
    /// point that mast is drawing from. No second search, no path stored per
    /// mast -- the number that decides which masts are lit is the same number
    /// that decides which way is downhill.
    ///
    /// Returns `None` for a mast with no power, which has no way home by
    /// definition. The route is beam heights rather than ground positions, so
    /// what the player watches is a ball travelling *along the beams* it can
    /// see, and the last leg drops to the machine's own coils.
    pub fn supply_route(&self, node: usize) -> Option<Vec<Vec3>> {
        let mut hops = self.nodes.get(node)?.hops?;
        let mut wired = vec![Vec::new(); self.nodes.len()];
        for &(a, b) in &self.links {
            wired[a].push(b);
            wired[b].push(a);
        }
        let mut legs = vec![self.nodes[node].top];
        let mut here = node;
        while hops > 0 {
            // The neighbour nearest the machine. Strictly nearer, so the walk
            // cannot stall on a tie or double back, and it terminates because
            // the count falls by at least one every step.
            let next = wired[here]
                .iter()
                .copied()
                .filter(|&other| self.nodes[other].hops.is_some_and(|theirs| theirs < hops))
                .min_by_key(|&other| self.nodes[other].hops.unwrap_or(u32::MAX))?;
            here = next;
            hops = self.nodes[here].hops?;
            legs.push(self.nodes[here].top);
        }
        // Off the end of the network and into the machine that lit it. The mast
        // the walk finished on is one with no hops, which is one `rebuild`
        // recorded a [`Node::feed`] for, so this is the actual source rather
        // than a guess at the nearest -- and it is the same beam [`draw`] has
        // already strung, so the last leg of the flight is one the player can
        // watch the ball travelling along.
        legs.push(*self.feeds.get(self.nodes[here].feed?)?);
        Some(legs)
    }

    /// Where the packet is after flying `along` legs of the run, and which way
    /// it is pointing.
    ///
    /// Returns `None` while there is no run to fly. The run is a closed loop --
    /// its last mast is linked back to its first -- so this wraps rather than
    /// stopping at the end.
    pub fn packet_at(&self, along: f32) -> Option<(Vec3, Vec3)> {
        if self.run.len() < 2 {
            return None;
        }
        let legs = self.run.len();
        let leg = (along.floor() as usize) % legs;
        let fraction = along - along.floor();
        let from = self.nodes[self.run[leg]].top;
        let to = self.nodes[self.run[(leg + 1) % legs]].top;
        Some((from.lerp(to, fraction), (to - from).normalize_or_zero()))
    }
}

/// Whether two points are near enough to string a beam between.
///
/// Straight-line distance, height and all, unlike the footprint test that
/// decides whether a mast may be planted: a beam is a beam in three dimensions,
/// and one strung to a mast on a clifftop is genuinely longer than the map
/// makes it look.
pub fn in_reach(from: Vec3, to: Vec3) -> bool {
    from.distance_squared(to) <= REACH * REACH
}

/// Every pair of masts that should have a beam between them.
///
/// `sees` is asked whether the two ends can see each other and is separate from
/// the range test on purpose: range is arithmetic and visibility is a ray into
/// the level, so the cheap half runs first and the expensive half runs only for
/// pairs that passed it. That ordering is what keeps a network of a few dozen
/// masts to a handful of casts rather than to *n²* of them.
///
/// Each pair once, in ascending order, so a caller drawing them draws one beam
/// per link rather than two through each other.
pub fn links(tops: &[Vec3], sees: impl Fn(Vec3, Vec3) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for a in 0..tops.len() {
        for b in a + 1..tops.len() {
            if in_reach(tops[a], tops[b]) && sees(tops[a], tops[b]) {
                out.push((a, b));
            }
        }
    }
    out
}

// -- placing ----------------------------------------------------------------

/// The live placement: how long the key has been down, where it resolves to,
/// and what the ring is saying about that spot.
///
/// [`crate::stellarator::Build`]'s shape, and for the same reason: the state
/// belongs to the button rather than to any one entity.
#[derive(Resource, Default)]
pub struct Plant {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    /// Nothing already standing is in the way.
    pub clear: bool,
    /// It would join the network -- a machine or a live mast can see it.
    pub joins: bool,
}

impl Plant {
    /// The ring appears once the press has outlasted a tap, so a tap does not
    /// flash one on its way to planting a mast.
    pub fn showing(&self) -> bool {
        self.held_for.is_some_and(|held| held >= squad::TAP_SECONDS)
    }
}

/// Whether a mast may be planted here: is there room for its footing.
///
/// Deliberately [`crate::stellarator::fits`] rather than a second rule that
/// looks like it. A pylon and a machine are both things standing on the lawn
/// with a footprint, and a player who has learnt what a red ring means under
/// one should not have to learn it again under the other.
pub fn fits(at: Vec3, placed: &[(Vec3, f32)]) -> bool {
    stellarator::fits(at, mast().radius, placed)
}

/// The meshes and materials every mast, beam and ring is drawn with.
///
/// Built once and shared, exactly like [`crate::stellarator::FieldArt`]:
/// planting a mast should allocate a handful of entities and nothing else.
#[derive(Resource, Clone)]
pub struct GridArt {
    beam: Handle<Mesh>,
    ring: Handle<Mesh>,
    /// A beam carrying power, and one strung between two dark masts.
    live: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
    packet: Handle<StandardMaterial>,
    /// The ring: joined, standing alone, and blocked.
    joined: Handle<StandardMaterial>,
    lonely: Handle<StandardMaterial>,
    blocked: Handle<StandardMaterial>,
}

/// How wide a ring the console's `pylon <n>` plants its masts on.
///
/// A distance of its own rather than a fraction of [`REACH`], which is what it
/// was. A beam now carries most of the way across the map, and a ring at a
/// fraction of that is a ring whose far side is over the horizon -- the command
/// exists to put a network in *one photograph*, and the thing being photographed
/// is a handful of masts, not the radius they happen to be legal at.
const COMMAND_RING: f32 = 18.0;

/// How thick a beam is drawn, in metres. Thin enough to be a beam rather than a
/// girder, thick enough to survive the internal render resolution the display
/// settings can drop the world to.
const BEAM_WIDTH: f32 = 0.16;

/// How big the supply packet is, and how fast it flies, in metres a second.
///
/// Faster than anything else in the game moves, because it is light rather than
/// a vehicle, and slow enough to follow with your eye across a valley.
const PACKET_SIZE: f32 = 0.55;
const PACKET_SPEED: f32 = 26.0;

/// Builds the shared art and puts the placement preview up.
pub fn prepare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> GridArt {
    let glow = |colour: Color, strength: f32| StandardMaterial {
        base_color: colour,
        emissive: colour.to_linear() * strength,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let flat = |colour: Color| StandardMaterial {
        base_color: colour,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let art = GridArt {
        // The tracer's mesh and the wisp's: a unit cuboid stretched along one
        // axis is a beam, and there is one of them rather than one per link.
        beam: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        // The whistle's annulus, shared with the squad and the stellarator so
        // that every ring this game asks a player to read is the same mark.
        ring: meshes.add(squad::ring_mesh()),
        live: materials.add(glow(Color::srgba(0.35, 0.90, 1.00, 0.85), 6.0)),
        dark: materials.add(glow(Color::srgba(0.30, 0.38, 0.46, 0.35), 0.4)),
        packet: materials.add(glow(Color::srgba(0.85, 0.98, 1.00, 1.0), 14.0)),
        joined: materials.add(flat(Color::srgba(0.40, 0.95, 1.00, 0.80))),
        lonely: materials.add(flat(Color::srgba(0.95, 0.80, 0.30, 0.80))),
        blocked: materials.add(flat(Color::srgba(1.00, 0.35, 0.30, 0.80))),
    };
    let site = commands
        .spawn((
            BuildSite,
            Transform::default(),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
        ))
        .id();
    commands.entity(site).with_child((
        SiteRing,
        bevy::light::NotShadowCaster,
        Mesh3d(art.ring.clone()),
        MeshMaterial3d(art.joined.clone()),
        // Just clear of the ground, the same 5 cm the whistle's ring and the
        // machine's footprint are lifted by, so it is not half-buried in the
        // slope it is drawn on.
        Transform::from_xyz(0.0, 0.05, 0.0).with_scale(Vec3::new(
            mast().radius,
            1.0,
            mast().radius,
        )),
    ));
    commands.insert_resource(art.clone());
    art
}

/// Puts one mast in the world.
///
/// Shared by the plant key, the console and anything else that ever wants one,
/// the way [`crate::stellarator::spawn`] is.
pub fn spawn(commands: &mut Commands, assets: &AssetServer, at: Vec3, yaw: f32) -> Entity {
    let planted = commands
        .spawn((
            Pylon {
                radius: mast().radius,
            },
            // What makes a mast a thing the crowd comes for. A `Side` is all
            // [`crate::enemy::alert`] asks of anything before it decides to
            // chase it, so these three components -- and no targeting code at
            // all -- are why an ant walking past a pylon stops and hits it.
            crate::enemy::Side::Friendly,
            crate::structure::Structure::new(mast().radius, mast().height),
            crate::health::Health::new(crate::health::PYLON_HEALTH),
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
        ))
        .id();
    commands.entity(planted).with_child((
        bevy::world_serialization::WorldAssetRoot(assets.load(MODEL)),
        // Nothing to correct: the generator writes the mast upright, in metres,
        // standing on its own origin. The child exists to give `n64::convert` a
        // scene root to walk down from -- see [`crate::stellarator::spawn`],
        // which is the same arrangement for the same reason.
        Transform::default(),
    ));
    planted
}

/// Everything the preview and the masts both need kept off each other's
/// `Transform`.
///
/// Every exclusion is load-bearing: Bevy proves two queries disjoint from their
/// filters alone, so a mutable `Transform` that does not name the other
/// `Transform` queries in its own system is a schedule that refuses to build --
/// which, in a windowed build, is a game that opens and shuts without a word.
type SiteQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<BuildSite>,
        Without<Player>,
        Without<Camera3d>,
        Without<Pylon>,
        Without<Stellarator>,
    ),
>;

/// The plant key: held opens a site, released puts a mast on it.
///
/// Runs at the render rate rather than on the fixed step, for
/// [`crate::stellarator::place`]'s reason -- the preview is drawn every frame
/// -- and it takes the released edge the same latched way, so a press is
/// neither lost nor counted twice across the fixed-step boundary.
#[allow(clippy::too_many_arguments)]
pub fn place(
    time: Res<Time>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut input: ResMut<InputState>,
    level: Res<LevelData>,
    art: Res<GridArt>,
    mut plant: ResMut<Plant>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
    player: Query<&Transform, With<Player>>,
    planted: Query<(&Transform, &Pylon)>,
    machines: Query<(&Transform, &Stellarator)>,
    mut site: SiteQuery,
    mut visibility: Query<&mut Visibility, With<BuildSite>>,
    mut ring: Query<&mut MeshMaterial3d<StandardMaterial>, With<SiteRing>>,
) {
    let (Ok(camera), Ok(leader)) = (camera.single(), player.single()) else {
        return;
    };
    let released = InputState::take(&mut input.pylon_released);
    if input.pylon || released {
        // Refreshed on the press as well as on the hold, so a tap too short to
        // have opened a site still plants somewhere.
        plant.aim = squad::aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
        );
        plant.held_for = Some(plant.held_for.unwrap_or(0.0) + time.delta_secs());
        let taken: Vec<_> = planted
            .iter()
            .map(|(transform, pylon)| (transform.translation, pylon.radius))
            .chain(
                machines
                    .iter()
                    .map(|(transform, machine)| (transform.translation, machine.radius)),
            )
            .collect();
        plant.clear = fits(plant.aim, &taken);
        // Would it join anything? The same two tests the network itself makes,
        // asked of a mast that is not there yet: a machine or another mast
        // within reach, with nothing in the way.
        let top = plant.aim + Vec3::Y * mast().emitter;
        let sees = |from: Vec3, to: Vec3| level.segment_hit(from, to).is_none();
        plant.joins = machines
            .iter()
            .map(|(transform, _)| feed_point(transform))
            .chain(
                planted
                    .iter()
                    .map(|(transform, _)| transform.translation + Vec3::Y * mast().emitter),
            )
            .any(|other| in_reach(other, top) && sees(other, top));
    }
    if released {
        plant.held_for = None;
        if plant.clear {
            // Turned to face whoever planted it, so a line of masts does not
            // read as a texture stamped at one compass bearing.
            let away = plant.aim - leader.translation;
            spawn(&mut commands, &assets, plant.aim, away.x.atan2(away.z));
        }
    }
    let showing = plant.showing();
    if let Ok(mut transform) = site.single_mut() {
        if showing {
            transform.translation = plant.aim;
        }
    }
    if let Ok(mut visible) = visibility.single_mut() {
        *visible = if showing {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let Ok(mut material) = ring.single_mut() {
        // Three answers rather than two: blocked, clear but standing on its
        // own, and clear and wired in. The middle one is a legal mast that does
        // nothing yet, and a player who could not tell it apart from the third
        // would find that out only by walking to it.
        let wanted = match (plant.clear, plant.joins) {
            (false, _) => &art.blocked,
            (true, false) => &art.lonely,
            (true, true) => &art.joined,
        };
        if material.0.id() != wanted.id() {
            material.0 = wanted.clone();
        }
    }
}

/// Where a machine feeds the network from: the middle of its coils.
///
/// Its own lift, scaled the way the machine is, so a half-size stellarator
/// feeds from half the height rather than from a number written down here.
fn feed_point(transform: &Transform) -> Vec3 {
    transform.translation + Vec3::Y * stellarator::machine().lift * transform.scale.y
}

/// Rebuilds the network when what is standing has changed.
///
/// Cheap to check and expensive to do, so the check is the whole system most
/// frames: a rebuild costs a ray per pair within reach, and the set of masts
/// only changes when somebody plants one, a level is swapped, or a machine goes
/// up. Counting both is enough to notice all three, because nothing in this
/// game moves a mast once it is planted.
pub fn relink(
    level: Res<LevelData>,
    mut network: ResMut<Network>,
    masts: Query<(Entity, &Transform), With<Pylon>>,
    machines: Query<&Transform, With<Stellarator>>,
) {
    let standing = masts.iter().count();
    let feeding = machines.iter().count();
    if standing == network.nodes.len() && feeding == network.feeds.len() {
        return;
    }
    let mut planted: Vec<(Entity, Vec3)> = masts
        .iter()
        .map(|(entity, transform)| (entity, transform.translation))
        .collect();
    // Sorted, because a Bevy query hands entities over in whatever order the
    // archetypes happen to be in, and a network whose node numbering moved
    // between two frames is a supply run that jumps. Sorted by position rather
    // than by entity id so that two runs of the same session agree.
    planted.sort_by(|a, b| {
        (a.1.x, a.1.z)
            .partial_cmp(&(b.1.x, b.1.z))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let feeds: Vec<Vec3> = machines.iter().map(feed_point).collect();
    network.rebuild(planted, &feeds, |from, to| {
        level.segment_hit(from, to).is_none()
    });
}

/// Draws the beams, and only when there is something new to draw.
///
/// The whole set is despawned and respawned on a rebuild rather than being
/// edited in place. A network is a few dozen links and it changes when a player
/// plants a mast; the alternative is keeping an entity per link in step with a
/// graph that is itself rebuilt wholesale, which is bookkeeping in exchange for
/// nothing.
pub fn draw(
    mut commands: Commands,
    network: Res<Network>,
    art: Res<GridArt>,
    mut drawn: Local<u64>,
    beams: Query<Entity, With<Beam>>,
) {
    if *drawn == network.revision && !network.is_changed() {
        return;
    }
    *drawn = network.revision;
    for beam in &beams {
        commands.entity(beam).despawn();
    }
    // Every beam there is: mast to mast, and machine to mast for the masts a
    // machine feeds directly.
    //
    // **The second kind was missing entirely**, and the network read as a set of
    // masts standing near a stellarator rather than as anything joined to it.
    // Power flooded out of the machine and the supply packet made its rounds, so
    // it *worked* -- there was simply nothing on the screen saying where any of
    // it came from, and the further a mast could be planted from its source the
    // more plainly that showed. Now the loose end of the beams is the machine.
    let feeds = network.nodes.iter().filter_map(|node| {
        node.feed
            .and_then(|which| network.feeds.get(which))
            .map(|feed| (*feed, node.top, true))
    });
    let links = network.links.iter().map(|&(a, b)| {
        (
            network.nodes[a].top,
            network.nodes[b].top,
            network.powered(a) && network.powered(b),
        )
    });
    for (from, to, live) in links.chain(feeds) {
        let span = to - from;
        let length = span.length();
        if length <= 1e-3 {
            continue;
        }
        commands.spawn((
            Beam,
            bevy::light::NotShadowCaster,
            Mesh3d(art.beam.clone()),
            MeshMaterial3d(if live {
                art.live.clone()
            } else {
                art.dark.clone()
            }),
            // `looking_to` points local -Z along what it is given, so the
            // negation here puts local +Z -- the axis the cuboid is stretched
            // on -- along the beam. The same turn the tracer and the wisp make,
            // for the same mesh.
            Transform::from_translation((from + to) * 0.5)
                .looking_to(-span / length, Vec3::Y)
                .with_scale(Vec3::new(BEAM_WIDTH, BEAM_WIDTH, length)),
        ));
    }
}

/// Flies the supply packet round the run, spawning it the first time there is
/// one and hiding it whenever there is not.
pub fn carry(
    time: Res<Time>,
    mut commands: Commands,
    network: Res<Network>,
    art: Res<GridArt>,
    mut packet: Query<(Entity, &mut Packet, &mut Transform, &mut Visibility)>,
) {
    let Ok((entity, mut packet, mut transform, mut visible)) = packet.single_mut() else {
        // None yet. One is spawned whatever the network looks like, so the
        // system that flies it has something to fly and nothing has to be
        // spawned mid-flight.
        commands.spawn((
            Packet { along: 0.0 },
            bevy::light::NotShadowCaster,
            Mesh3d(art.beam.clone()),
            MeshMaterial3d(art.packet.clone()),
            Transform::from_scale(Vec3::splat(PACKET_SIZE)),
            Visibility::Hidden,
        ));
        return;
    };
    let Some(legs) = (network.run.len() > 1).then(|| network.run.len()) else {
        *visible = Visibility::Hidden;
        packet.along = 0.0;
        return;
    };
    // Advanced in legs rather than in metres, so a long hop across a valley
    // takes longer than a short one between two masts on a lawn: the speed is
    // in metres and the leg it is on says how many of them this leg is.
    let leg = (packet.along.floor() as usize) % legs;
    let from = network.nodes[network.run[leg]].top;
    let to = network.nodes[network.run[(leg + 1) % legs]].top;
    let length = from.distance(to).max(0.01);
    packet.along = (packet.along + PACKET_SPEED * time.delta_secs() / length) % legs as f32;
    if let Some((at, heading)) = network.packet_at(packet.along) {
        *transform = Transform::from_translation(at)
            .looking_to(
                if heading == Vec3::ZERO {
                    -Vec3::Z
                } else {
                    -heading
                },
                Vec3::Y,
            )
            .with_scale(Vec3::new(PACKET_SIZE, PACKET_SIZE, PACKET_SIZE * 2.4));
        *visible = Visibility::Visible;
    } else {
        *visible = Visibility::Hidden;
    }
    // Nothing else touches the entity; the `commands` borrow above is only for
    // the frame there is no packet at all.
    let _ = entity;
}

/// Fills the player's bar faster while he is standing by a live mast.
///
/// The reason to build a network. Run on the fixed step beside the rest of the
/// simulation, and it spends nothing and refuses nothing -- a drained bar comes
/// back faster here, which is exactly the point of standing next to a pylon.
pub fn supply(
    network: Res<Network>,
    mut player: Query<(&Transform, &mut crate::energy::Energy), With<Player>>,
) {
    let Ok((transform, mut energy)) = player.single_mut() else {
        return;
    };
    let near = network.nodes.iter().any(|node| {
        node.hops.is_some()
            && node.at.distance_squared(transform.translation) <= SUPPLY_RADIUS.powi(2)
    });
    if near {
        // The extra fill only. `player::drive` has already advanced the bar
        // once this step, so what is added here is the difference between
        // standing on open ground and standing under a pylon.
        energy.advance(crate::player::FIXED_DT * (SUPPLY_BOOST - 1.0));
    }
}

/// Gives every emitter head that arrives its own clock.
///
/// By name, the way [`crate::stellarator::claim`] finds the coils, because a
/// glTF node is the only thing this port and the generator have agreed on.
/// `try_insert` rather than `insert`: a scene can be despawned by a level
/// change between the query and the flush, and a scene that has gone has
/// nothing to claim.
pub fn claim(
    mut commands: Commands,
    arrivals: Query<(Entity, &Name), Added<Name>>,
    hierarchy: Query<&ChildOf>,
    masts: Query<&Pylon>,
) {
    for (entity, name) in &arrivals {
        if name.as_str() != EMITTER_NODE {
            continue;
        }
        let Some(mast) = owner(entity, &hierarchy, &masts) else {
            // A head that arrived under nothing this module planted. A level
            // is free to put a pylon model down as scenery, and scenery does
            // not belong to the network.
            continue;
        };
        commands.entity(entity).try_insert(Emitter {
            // Off the entity's own index, so two heads that arrived together
            // are still a golden angle apart and the same mast breathes the
            // same way every run.
            phase: entity.index().index() as f32 * GOLDEN_ANGLE,
            mast,
        });
    }
}

/// Walks up out of a loaded scene to the mast it was spawned under.
///
/// The same walk [`crate::n64`] makes to find what an arriving mesh belongs to,
/// asking a different question of it: a glTF node is several levels below the
/// entity that carries the game's own components, and the loader is what put
/// those levels there.
fn owner(entity: Entity, hierarchy: &Query<&ChildOf>, masts: &Query<&Pylon>) -> Option<Entity> {
    let mut ancestor = entity;
    loop {
        if masts.get(ancestor).is_ok() {
            return Some(ancestor);
        }
        ancestor = hierarchy.get(ancestor).ok()?.parent();
    }
}

/// Breathes the emitter heads of masts that have power.
///
/// A scale rather than a material: the head is a lit object in a scene the
/// renderer has already restyled, and pulsing its emissive would mean a
/// material per mast. Small -- a tenth either way -- because this is the
/// network idling, not an alarm.
pub fn shimmer(
    time: Res<Time>,
    network: Res<Network>,
    mut heads: Query<(&Emitter, &mut Transform)>,
) {
    let elapsed = time.elapsed_secs();
    for (emitter, mut transform) in &mut heads {
        // A dark mast is a mast doing nothing, and it looks like one: the head
        // sits at its authored size until the network reaches it.
        let pulse = match network.powered_entity(emitter.mast) {
            true => (elapsed * 1.6 + emitter.phase).sin(),
            false => 0.0,
        };
        transform.scale = Vec3::splat(1.0 + 0.10 * pulse);
    }
}

/// Carries out `pylon <n>` and `pylon clear` from the console.
///
/// A line of masts running away from the player, each inside the next one's
/// reach, with a machine at the near end feeding them -- so what the command
/// leaves standing is a *live* network rather than a row of dark poles. Each
/// mast is dropped onto the ground under it, because the line is drawn on a
/// map and the map has hills.
///
/// Runs in the overlay rather than in the simulation, for `enemy::crowd`'s
/// reason: the console is open at the moment the command is typed, and a
/// network that only appeared once you shut it is a network you never saw
/// arrive.
pub fn command(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut console: ResMut<crate::console::ConsoleState>,
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    standing: Query<Entity, With<Pylon>>,
) {
    // Taken before the loop and unconditionally: a request left in the queue
    // because this frame had nothing to do with it is a network planted twice.
    for request in console.take_requests() {
        let count = match request {
            crate::console::Request::ClearPylons => 0,
            crate::console::Request::Pylons(count) => count,
            // Not this system's request. Put back, so whoever it does belong to
            // still sees it -- `take_requests` drains the queue whole.
            other => {
                console.defer(other);
                continue;
            }
        };
        // Both cases clear first, so `pylon 5` twice is five masts and not ten.
        for mast in &standing {
            commands.entity(mast).despawn();
        }
        let Ok(player) = player.single() else {
            continue;
        };
        if count == 0 {
            continue;
        }
        // A ring around him rather than a line away from him, for two
        // reasons: every mast on it is inside the next one's reach whatever
        // the count, so the network is connected by construction, and a ring
        // fits in one view -- a line marches off past the far side of the map
        // and is photographed as two specks.
        let centre = player.translation;
        // Dropped onto whatever is under it, and `None` where there is
        // nothing: part of the ring can hang over the moat or off the edge of
        // the map, and a mast planted on open air is a mast standing in the
        // sky. Those stops are skipped rather than fudged onto the player's own
        // height.
        let ground = |at: Vec3| {
            level
                .floor_height(at + Vec3::Y * 40.0)
                .map(|height| Vec3::new(at.x, height, at.z))
        };
        // The machine that feeds them, in the middle. At 0.6 of its authored
        // size, which leaves the ring clear of its footprint: a stellarator is
        // sixteen metres across, and one standing through a mast is the very
        // thing the placement rule refuses by hand.
        stellarator::spawn(
            &mut commands,
            &assets,
            ground(centre).unwrap_or(centre),
            0.0,
            0.6,
        );
        let radius = COMMAND_RING;
        for step in 0..count {
            let angle = step as f32 / count as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            let Some(at) = ground(centre + Vec3::new(sin, 0.0, cos) * radius) else {
                continue;
            };
            // Facing inward, at the machine they are drawing from.
            spawn(&mut commands, &assets, at, (-sin).atan2(-cos));
        }
    }
}

/// Placing and drawing, in the order one frame does them.
pub fn systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (place, relink, draw, carry, shimmer).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything in sight of everything else.
    fn open(_: Vec3, _: Vec3) -> bool {
        true
    }

    fn at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    #[test]
    fn a_beam_needs_both_range_and_sight() {
        let tops = [at(0.0, 0.0), at(REACH * 0.5, 0.0), at(REACH * 2.0, 0.0)];
        let near = links(&tops, open);
        assert_eq!(near, vec![(0, 1)], "only the pair within reach links");
        // A wall between the two that were in reach leaves nothing at all.
        let blind = links(&tops, |_, _| false);
        assert!(blind.is_empty(), "{blind:?}");
        // Each pair once and in order, never both ways round.
        let ring = [at(0.0, 0.0), at(5.0, 0.0), at(0.0, 5.0)];
        assert_eq!(links(&ring, open), vec![(0, 1), (0, 2), (1, 2)]);
    }

    #[test]
    fn power_floods_out_from_the_machines_and_stops_where_the_chain_breaks() {
        let mut network = Network::default();
        // Four masts in a line, each within reach of the next and of nothing
        // further, and a machine sitting behind the first one -- behind it far
        // enough that the machine feeds that mast and no other, so what the
        // rest have is what came down the chain. The last mast is out past the
        // end of it entirely.
        let masts = vec![
            (Entity::from_raw_u32(1).unwrap(), at(0.0, 0.0)),
            (Entity::from_raw_u32(2).unwrap(), at(REACH * 0.9, 0.0)),
            (Entity::from_raw_u32(3).unwrap(), at(REACH * 1.8, 0.0)),
            (Entity::from_raw_u32(4).unwrap(), at(REACH * 9.0, 0.0)),
        ];
        network.rebuild(masts, &[at(-REACH * 0.5, 0.0)], open);
        assert!(network.powered(0) && network.powered(1) && network.powered(2));
        assert!(!network.powered(3), "power jumped a gap it cannot cross");
        assert_eq!(network.live(), 3);
        // And the hops count outward from the machine rather than from nowhere.
        assert_eq!(network.nodes[0].hops, Some(0));
        assert_eq!(network.nodes[2].hops, Some(2));
        assert_eq!(network.nodes[3].hops, None);
    }

    #[test]
    fn a_network_with_no_machine_is_a_network_standing_dark() {
        let mut network = Network::default();
        let masts = vec![
            (Entity::from_raw_u32(1).unwrap(), at(0.0, 0.0)),
            (Entity::from_raw_u32(2).unwrap(), at(REACH * 0.5, 0.0)),
        ];
        network.rebuild(masts, &[], open);
        // Linked -- the beams are there and drawn dark -- but not live.
        assert_eq!(network.links.len(), 1);
        assert_eq!(network.live(), 0);
        assert!(network.run.is_empty(), "a dark network has nothing to fly");
        assert!(network.packet_at(0.0).is_none());
    }

    #[test]
    fn the_supply_run_calls_at_every_live_mast_along_real_beams() {
        let mut network = Network::default();
        // A square of masts, all within reach of each other, fed at one corner.
        let corners = [
            at(0.0, 0.0),
            at(REACH * 0.6, 0.0),
            at(REACH * 0.6, REACH * 0.6),
            at(0.0, REACH * 0.6),
        ];
        let masts: Vec<_> = corners
            .iter()
            .enumerate()
            .map(|(index, &at)| (Entity::from_raw_u32(index as u32 + 1).unwrap(), at))
            .collect();
        network.rebuild(masts, &[at(1.0, 1.0)], open);
        assert_eq!(network.live(), 4);
        // Every live mast is called at.
        for node in 0..4 {
            assert!(
                network.run.contains(&node),
                "mast {node} is never visited: {:?}",
                network.run
            );
        }
        // And every leg of the run is a beam that exists. This is what the
        // shortest-path expansion buys: without it a tour that hops between two
        // masts with no link between them would fly through a hillside.
        for pair in network.run.windows(2) {
            let (a, b) = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            assert!(
                network.links.contains(&(a, b)),
                "the run flies {a}->{b}, which is not a beam"
            );
        }
        // The packet is somewhere on the run and moves along it.
        let (start, _) = network.packet_at(0.0).expect("a live run has a packet");
        let (later, _) = network.packet_at(0.5).expect("still flying");
        assert!(start.distance(later) > 1.0, "the packet did not move");
    }

    #[test]
    fn a_delivered_ball_walks_downhill_to_the_machine_that_lit_the_mast() {
        let mut network = Network::default();
        // Four masts in a chain, fed at the near end only, so the far one is
        // three hops out and there is exactly one way home.
        let masts: Vec<_> = (0..4)
            .map(|index| {
                (
                    Entity::from_raw_u32(index + 1).unwrap(),
                    at(REACH * 0.6 * index as f32, 0.0),
                )
            })
            .collect();
        let feed = at(-REACH * 0.5, 0.0);
        network.rebuild(masts, &[feed], open);
        assert_eq!(network.live(), 4);
        assert_eq!(network.nodes[3].hops, Some(3));

        // Every lit mast records which machine lit it, and only the ones a
        // machine reaches directly: that is the set `draw` strings the feed
        // beams along and the set a shipment's last leg ends at.
        assert_eq!(
            network.nodes[0].feed,
            Some(0),
            "the near mast has no source"
        );
        assert_eq!(
            network.nodes[3].feed, None,
            "a mast three hops out is not fed by the machine directly"
        );

        let route = network
            .supply_route(3)
            .expect("a lit mast has a way back to what lit it");
        // Four masts and then the machine, at beam height until the last leg.
        assert_eq!(route.len(), 5, "{route:?}");
        assert_eq!(route[0], network.nodes[3].top);
        assert_eq!(route[4], feed);
        // Strictly downhill: every step is nearer the machine than the last.
        for (step, pair) in route[..4].windows(2).enumerate() {
            assert!(
                pair[1].distance(feed) < pair[0].distance(feed),
                "leg {step} of {route:?} does not head home"
            );
        }
        // The mast the machine feeds directly is one leg and the machine.
        assert_eq!(network.supply_route(0).map(|route| route.len()), Some(2));
        // And a dark mast has no way home at all.
        let mut dark = Network::default();
        dark.rebuild(
            vec![(Entity::from_raw_u32(1).unwrap(), at(0.0, 0.0))],
            &[],
            open,
        );
        assert!(dark.supply_route(0).is_none());
        assert!(
            dark.supply_route(7).is_none(),
            "and neither has a mast that is not there"
        );
    }

    #[test]
    fn a_mast_may_not_be_planted_through_something_already_standing() {
        let radius = mast().radius;
        let taken = [(at(0.0, 0.0), radius)];
        assert!(fits(at(radius * 2.0, 0.0), &taken));
        assert!(!fits(at(radius * 1.5, 0.0), &taken));
        assert!(fits(at(0.0, 0.0), &[]));
    }

    #[test]
    fn the_mast_is_measured_off_the_file_it_is_drawn_from() {
        let measured = mast();
        // A pylon is a tall thin thing that stands on the ground and carries
        // its beams near the top. All three are properties of the shape rather
        // than of its size, which is the point: how big a pylon is is the
        // author's to change in `assets/actors/pylon.blend`.
        assert!(measured.height > 2.0, "{measured:?}");
        assert!(measured.radius * 4.0 < measured.height, "{measured:?}");
        assert!(
            measured.emitter > measured.height * 0.6 && measured.emitter <= measured.height,
            "{measured:?}"
        );
    }
}
