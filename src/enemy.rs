//! The things that can be fought, and how they resolve against each other.
//!
//! Two sides: the enemies, and the player with his Marios. Both sides notice
//! each other the same way, pass the alarm along the same way, and walk to
//! whatever they have noticed the same way -- see [`Side`] and [`alert`] -- and
//! then each resolves what it does on arrival in its own terms, because a
//! sword, a punch and walking into a slime are three different things.
//!
//! The player's half of the combat rules is ported from `Interactions.resolve`
//! in `sm64py/objects.py`, and every distance is that build's, converted from
//! SM64 units to the port's world scale of 1/100.

use crate::{
    audio::{Sfx, SoundQueue},
    console::{ConsoleState, CrowdKind, GameTuning, Request},
    health::{self, Health},
    level::LevelData,
    player::{Controller, Motion, Player, FIXED_DT, PLAYER_HEIGHT},
    squad::Ally,
};
use bevy::{prelude::*, world_serialization::WorldAssetRoot};

/// How far above an enemy's own feet counts as landing on top of it, as a
/// fraction of its height.
const STOMP_MARGIN: f32 = 0.6;

/// How far the player's body reaches in the horizontal plane for the purpose
/// of touching an enemy, which is not the same as the radius he is pushed out
/// of walls with.
const PLAYER_REACH: f32 = 0.37;

/// How much past its own body an enemy counts as touching a Mario from.
///
/// [`spread`] holds two bodies apart at the sum of their radii, so an enemy and
/// the Mario it is fighting are *exactly* at the distance a bare radius test
/// calls a miss, and whether any given tick lands is down to the last bit of a
/// float. This is the slack that makes standing against something mean standing
/// against it.
const TOUCH_MARGIN: f32 = 0.25;

/// How far a swing reaches past the body it is aimed at. Wider than a touch on
/// purpose: Luna swings a sword, and a weapon that only hits what is already
/// standing on him is not a weapon.
///
/// Past the *body*, like [`PLAYER_REACH`] and unlike its own first version. As a
/// distance to the target's centre it was unreachable against a large actor:
/// [`spread`] holds an ant of radius 2.5 at 3.27 m from the player, and a swing
/// of 2.2 never landed. The enemies got bigger and the sword did not.
const ATTACK_REACH: f32 = 1.9;

/// How near an enemy has to be before [`combat`] will look at it at all.
///
/// The furthest anything in that function can reach: the widest body on the
/// field plus the longest of the sword's swing, the player's own touch and his
/// height, with room over the top so that the bound is obviously a bound rather
/// than a number that has to be re-derived every time one of those changes.
const COMBAT_REACH: f32 = 12.0;

/// Up off a stomped enemy, and back off one that got a hit in.
const BOUNCE_VELOCITY: f32 = 12.6;
const KNOCKBACK_SPEED: f32 = 7.2;
const KNOCKBACK_RISE: f32 = 6.0;

/// How long the player is immune after a hit -- 30 frames at 30 Hz in the
/// original. Long enough for the knockback to carry him clear of whatever hit
/// him, which is the entire point of it.
const INVULNERABLE_SECONDS: f32 = 1.0;

/// How fast an enemy ambles about with nobody to chase, which is not how fast
/// it comes for you once it has noticed you -- that is `enemy_speed`.
const WANDER_SPEED: f32 = 1.2;

/// The amble an enemy falls back on while nothing has its attention: how far
/// from where it was placed it will wander, how near it has to get before that
/// counts as arriving, and how long it stands about afterwards before picking
/// somewhere else.
///
/// A walk to a fixed spot followed by a rest, rather than a point it chases
/// continuously -- the same shape [`crate::squad::Ally`] ambles in, and for the
/// same reason: a target that moves every tick is one the walker never arrives
/// at, so it never stands still and its walk cycle restarts forever.
const WANDER_RADIUS: f32 = 7.0;
const WANDER_ARRIVE: f32 = 0.6;
const WANDER_REST: f32 = 1.5;
const WANDER_REST_SPREAD: f32 = 3.0;

/// The tallest thing a walking enemy can get up.
///
/// Not a tuning knob so much as a statement of what these are: short things
/// close to the ground. Anything taller than this is a wall to it however walkable the
/// top happens to be, which is the difference between climbing a slope and
/// appearing on top of a cliff.
///
/// It is half a unit because that is the tolerance [`LevelData::ground_at`]
/// already answers with, and one step limit is better than two that disagree.
pub(crate) const STEP_UP: f32 = 0.5;

/// How fast an enemy's feet follow the ground under them, climbing and
/// falling, in world units a second.
///
/// The floor query answers where its feet belong; putting them there in one
/// step is what makes a walker crossing a lip or a step look like it changed
/// elevation rather than walked up it. Climbing is the slower of the two on
/// purpose: things fall faster than they clamber.
const CLIMB_SPEED: f32 = 4.0;
const FALL_SPEED: f32 = 14.0;

/// How much room two enemies keep between their bodies, on top of the two
/// bodies themselves.
///
/// They are held apart as the cylinders they are already fought as, so this is
/// only the daylight between them -- but without some, a crowd all chasing the
/// same player converges on the same spot and stacks up into one enemy with
/// several models in it.
const PERSONAL_SPACE: f32 = 0.35;

/// How much of the overlap between two enemies is taken out per tick, how much
/// overlap is beneath noticing, and the furthest one may be shoved in a tick.
///
/// All three exist to keep a crowd still. Both members of a pair are pushed, so
/// even a quarter each closes the gap between them soon enough; the slack means
/// two that are merely touching are left alone rather than shoved apart and
/// pulled back every tick; and the cap stops one in the middle of a press, with
/// neighbours leaning on it from every side, from being fired out of the crowd
/// by the sum of them.
const SPREAD_RATE: f32 = 0.25;
const SPREAD_SLACK: f32 = 0.05;
const SPREAD_LIMIT: f32 = 1.5;

/// The most of an overlap one shove is allowed to take out, however many ticks
/// that shove is standing in for.
///
/// A body that is only shoved every fourth tick is given four ticks' worth of
/// rate so that a strided crowd settles as fast as an unstrided one -- but four
/// quarters is the whole overlap, and a pair that each take the whole of it are
/// a pair that swap places and come back. Half leaves the convergence
/// monotonic no matter how coarse the stride gets.
const SPREAD_RATE_CAP: f32 = 0.5;

/// How many fixed ticks pass between two shoves of a cheap-tier body.
///
/// Four, which is the stride [`update`] already walks a far enemy at: something
/// that only moves every fourth tick does not need holding out of its
/// neighbours more often than it moves. The whole point of the number is that
/// the cheap tier is *the whole rest of the field* -- eighteen hundred of the
/// two thousand -- so a quarter of it a tick is what makes spacing all of them
/// cost about what spacing the nearest two hundred used to.
const CROWD_SPREAD_STRIDE: u32 = 4;

/// Where a crawler's probes start and how far past its feet they reach.
///
/// `PROBE_EYE` is the height the forward probe is cast from -- low, because a
/// bug that meets a wall is put down where its probe struck it, and a probe
/// cast from its back would have it teleport half its own height up the wall.
/// `PROBE_RISE` and `PROBE_DROP` bound the down probe, and between them decide
/// the steepest step it can climb and the widest lip it can walk over before
/// the surface is considered to have run out.
const PROBE_EYE: f32 = 0.12;
const PROBE_RISE: f32 = 0.4;
const PROBE_DROP: f32 = 0.8;

/// How far past its next step a crawler looks for something in its way.
///
/// Short on purpose. A bug that meets a wall is stood where its probe found it,
/// so this is also the furthest it can be moved in a tick by finding one --
/// and a slope counts as something in the way once it rises more than
/// `PROBE_EYE` over this. Reaching a body's width ahead, as looks reasonable,
/// has bugs jumping the better part of a metre up every hill on the lawn.
const PROBE_REACH: f32 = 0.15;

/// How near a goal counts as being stood on it, in world units.
///
/// The deadband that stops a chase overshooting its own standoff spot. Only has
/// to be smaller than anything the eye could see and larger than the float
/// noise in a position that is rewritten thirty times a second, and a
/// millimetre is comfortably both.
const ARRIVED: f32 = 1e-3;

/// How fast a crawler can turn, in radians a second.
///
/// Not decoration. A bug that could turn instantly ping-pongs between the floor
/// and the wall in front of it every single tick -- the wall is the way towards
/// a player stood behind it, and the floor is the way towards him again the
/// moment the bug is on the wall -- and it spends the whole fight spinning on
/// the spot. Turning at a bug's pace, it commits to the climb.
const TURN_RATE: f32 = 3.0;

/// How fast a crawler may roll from one surface's angle onto another's, in
/// radians a second.
///
/// A degree a millisecond, which is 1,000 degrees a second: quick enough that
/// nothing looks hesitant, slow enough that a right-angle corner takes three
/// ticks instead of none. Before it the surface normal was taken whole the tick
/// it was found, so a bug meeting the foot of a wall was *instantly* lying at
/// ninety degrees to the floor it had been on -- a flip with no frames in it,
/// which reads as the model glitching rather than as an animal climbing.
///
/// This is separate from [`TURN_RATE`], which is how fast it may turn *within*
/// a surface. Both exist for the same reason and neither substitutes for the
/// other: one is which way it is walking, this is which way is up.
const ROLL_RATE: f32 = 1000.0 * std::f32::consts::PI / 180.0;

/// How far off its surface a crawler is held. Nothing to do with looks: the
/// next tick's probes start here, and a probe starting exactly on a triangle is
/// a probe that may or may not find it depending on the last bit of the float.
const CRAWL_SKIN: f32 = 0.02;

/// How far from level a surface has to be before a crawler treats what is
/// behind it as something to climb over rather than walk along.
///
/// Half, which is a surface tilted more than sixty degrees. The number is not
/// measured off anything; what it has to do is separate two cases that are
/// nowhere near each other. A floor, a ramp and a hillside are all things a bug
/// walks *along*, and the steepest of those the level will call ground at all
/// is [`crate::level::GROUND_NORMAL_Y`] -- 0.7, or about forty-five degrees --
/// because anything sheerer is a wall that [`walk`] pushes bodies out of. A
/// wall and a ceiling are things a bug goes *over*, and their normals lie at
/// nought and minus one. So anywhere in that gap does, and the middle of it is
/// the least interesting place to put it. See [`crawl_towards`].
const CLIMB_STEEP: f32 = 0.5;

/// The enemies the port places. Each is resolved against the player as an
/// upright cylinder, the way the original does: a radius in the horizontal
/// plane and a height above its feet.
///
/// The discriminants index [`sizes`], so a kind added here is a kind measured
/// there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Slime,
    Ant,
}

/// Every kind, in discriminant order.
pub const KINDS: [Kind; 2] = [Kind::Slime, Kind::Ant];

/// How big an actor is, in world units, taken from the model itself.
#[derive(Clone, Copy, Debug)]
pub struct Size {
    /// The furthest its mesh reaches from its origin in the horizontal plane.
    pub radius: f32,
    /// Base to top.
    pub height: f32,
    /// How far its mesh reaches *below* its origin, which is how far the
    /// placement code has to hold it up to stand it on the floor.
    pub lift: f32,
}

/// What the game falls back on when an actor's model cannot be measured.
///
/// Only reachable with the model missing from the install, which is a game with
/// invisible enemies whatever this says -- but an enemy of size zero is one
/// nothing can walk past and nothing can stomp, so it is worth being a
/// creature-shaped number rather than a default.
const UNMEASURED: Size = Size {
    radius: 0.6,
    height: 0.65,
    lift: 0.0,
};

/// Every actor's size, measured from its glTF the first time one is asked for.
///
/// **This is why there is no scale factor in this file.** An actor is spawned at
/// the size it was authored at and measured at the size it is drawn, so the two
/// cannot disagree; changing how big a slime is means changing the slime, and
/// `tools/resize_actor.py` is how. The alternative -- constants next to a
/// `draw_scale` that multiplies them -- lasted exactly as long as it took to
/// re-export an actor, and left ants drawn four metres long while being spaced,
/// shadowed and punched as something 0.6 across.
///
/// Read from disk rather than from Bevy's loaded meshes because it is wanted
/// before anything has loaded and by tests that never build a renderer. It is
/// two files, once, at whatever the install's asset path is: about a megabyte
/// read and a JSON chunk parsed, on the first enemy of a session.
fn sizes() -> &'static [Size; KINDS.len()] {
    static SIZES: std::sync::OnceLock<[Size; KINDS.len()]> = std::sync::OnceLock::new();
    SIZES.get_or_init(|| KINDS.map(|kind| measure(kind.model()).unwrap_or(UNMEASURED)))
}

/// The size of the actor in a `.glb`, from its meshes' own position bounds.
///
/// Every glTF accessor carries `min` and `max`, so this is a header read rather
/// than a walk over vertices. Those bounds are the **bind pose**, which is what
/// the renderer skins from and what the exporter guarantees is in the file.
///
/// **Whether a node's transform counts depends on whether its mesh is skinned,
/// and the two answers are opposite.** A skinned mesh's vertices are in the
/// skin's own space: glTF says the node it hangs under is ignored, and the
/// inverse bind matrices cancel whatever the skeleton root carries -- which is
/// how `assets/actors/ant.blend` gets away with an armature scaled by 4.0,
/// yawed 180 degrees and lifted 0.79 m without any of it showing, in Blender's
/// viewport or in the game. An *unskinned* mesh in the same file takes its node
/// chain in full, so its bounds have to be walked down from the scene roots.
///
/// Ignoring node transforms for everything was right while an actor was one
/// skinned mesh and nothing else. Four spheres added to the ant in Blender were
/// plain objects with no skin and a node scale of about 0.3, and their raw mesh
/// data is a sphere of radius 1.681: measured flat, the ant became a creature
/// 5.0 m across that hung 1.68 m below its own origin, and the size tests said
/// so in four different ways.
///
/// Anything that did survive to change an actor's drawn size would show up in
/// `the_sheets_agree_with_the_models_they_were_baked_from`, which checks this
/// measurement against a sheet the renderer actually drew.
pub fn measure(model: &str) -> Option<Size> {
    let bytes = std::fs::read(crate::asset_path().join(model)).ok()?;
    // 12 bytes of header, then the JSON chunk's length and type.
    let length = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    let json: serde_json::Value = serde_json::from_slice(bytes.get(20..20 + length)?).ok()?;
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let corner = |accessor: &serde_json::Value, which: &str| {
        let bounds = accessor[which].as_array()?;
        Some(Vec3::new(
            bounds.first()?.as_f64()? as f32,
            bounds.get(1)?.as_f64()? as f32,
            bounds.get(2)?.as_f64()? as f32,
        ))
    };
    // A mesh's own bounds, unioned into the box under whatever transform the
    // caller says applies to it. Both corners of the box are carried through,
    // because a rotation or a negative scale swaps which one is which.
    let mut swallow = |mesh: &serde_json::Value, into: Mat4| -> Option<()> {
        for primitive in mesh["primitives"].as_array()? {
            let at = primitive["attributes"]["POSITION"].as_u64()? as usize;
            let accessor = &json["accessors"][at];
            let (near, far) = (corner(accessor, "min")?, corner(accessor, "max")?);
            for x in [near.x, far.x] {
                for y in [near.y, far.y] {
                    for z in [near.z, far.z] {
                        let at = into.transform_point3(Vec3::new(x, y, z));
                        low = low.min(at);
                        high = high.max(at);
                    }
                }
            }
        }
        Some(())
    };
    let nodes = json["nodes"].as_array()?;
    // Down from the scene's roots, so an unskinned mesh is measured where it is
    // actually drawn. Skinned meshes are taken flat wherever they are found --
    // including ones hung under a node this walk never reaches, since being
    // reachable is not what decides whether their vertices count.
    let mut stack: Vec<(usize, Mat4)> = json["scenes"]
        [json["scene"].as_u64().unwrap_or(0) as usize]["nodes"]
        .as_array()?
        .iter()
        .filter_map(|node| Some((node.as_u64()? as usize, Mat4::IDENTITY)))
        .collect();
    // Cheap insurance against a file whose graph has a cycle in it: a glTF with
    // one is invalid, and a loop here would hang the game before its first
    // frame with nothing on screen to say why.
    let mut left = nodes.len() * 4;
    while let Some((index, above)) = stack.pop() {
        left = left.checked_sub(1)?;
        let node = nodes.get(index)?;
        let here = above * node_matrix(node);
        if let Some(mesh) = node["mesh"].as_u64() {
            if node.get("skin").is_none() {
                swallow(&json["meshes"][mesh as usize], here)?;
            }
        }
        for child in node["children"].as_array().into_iter().flatten() {
            stack.push((child.as_u64()? as usize, here));
        }
    }
    for node in nodes {
        if let Some(mesh) = node["mesh"].as_u64() {
            if node.get("skin").is_some() {
                swallow(&json["meshes"][mesh as usize], Mat4::IDENTITY)?;
            }
        }
    }
    if low.x > high.x {
        return None;
    }
    Some(Size {
        radius: low.x.abs().max(high.x).max(low.z.abs()).max(high.z),
        height: high.y - low.y,
        lift: (-low.y).max(0.0),
    })
}

/// One glTF node's local transform, from whichever of the two forms it is
/// written in: a 4x4 `matrix`, or `translation`/`rotation`/`scale` with each
/// part optional.
fn node_matrix(node: &serde_json::Value) -> Mat4 {
    let numbers = |key: &str, count: usize| -> Option<Vec<f32>> {
        let list = node[key].as_array()?;
        (list.len() == count).then(|| {
            list.iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect()
        })
    };
    if let Some(cells) = numbers("matrix", 16) {
        return Mat4::from_cols_slice(&cells);
    }
    let translation = numbers("translation", 3).map_or(Vec3::ZERO, |v| Vec3::new(v[0], v[1], v[2]));
    // glTF writes a quaternion x, y, z, w -- the same order `Quat::from_xyzw`
    // takes, and not the order `Quat::from_array` documents itself with.
    let rotation =
        numbers("rotation", 4).map_or(Quat::IDENTITY, |v| Quat::from_xyzw(v[0], v[1], v[2], v[3]));
    let scale = numbers("scale", 3).map_or(Vec3::ONE, |v| Vec3::new(v[0], v[1], v[2]));
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

impl Kind {
    pub fn model(self) -> &'static str {
        match self {
            Self::Slime => "actors/slime.glb",
            Self::Ant => "actors/ant.glb",
        }
    }

    /// How solid this actor is drawn, as a share of its own opacity.
    ///
    /// The slime is jelly, and jelly you cannot see into is a painted ball. A
    /// fifth of the way to clear is enough to read as one -- it is the eyes
    /// showing faintly through the far side of the body that says the material
    /// has a depth, and past this the creature starts to disappear into
    /// whatever it is standing in front of.
    ///
    /// [`crate::n64::soften`] leaves the eyes themselves alone, so what fades
    /// is the body and only the body.
    pub fn opacity(self) -> f32 {
        match self {
            Self::Slime => 0.8,
            Self::Ant => 1.0,
        }
    }

    /// How much punishment one of these takes before it goes down.
    ///
    /// A slime dies to anything that reaches it -- a player's blow, a Mario's,
    /// a bullet -- and an ant survives a Mario's first punch and not his
    /// second. See [`crate::health`] for why those two facts are the whole of
    /// what these numbers are chosen for.
    pub fn health(self) -> i32 {
        match self {
            Self::Slime => crate::health::SLIME_HEALTH,
            Self::Ant => crate::health::ANT_HEALTH,
        }
    }

    /// What touching one of these costs whatever it touched.
    ///
    /// Contact damage rather than an attack, because that is how these fight:
    /// neither actor has a strike clip and neither one aims at anything. What
    /// stops it being a hundred points a second is the invulnerability window
    /// on the far side -- see [`INVULNERABLE_SECONDS`].
    pub fn damage(self) -> i32 {
        match self {
            Self::Slime => crate::health::SLIME_DAMAGE,
            Self::Ant => crate::health::ANT_DAMAGE,
        }
    }

    /// Which of the model's glTF animations is the one it walks on.
    ///
    /// The decomp actors these replaced had exactly one clip each, so an index
    /// was never a question: `#Animation0` was the only thing in the file.
    /// Authored art carries a whole set. The slime has ten -- three emotes, two
    /// idles, two ways of moving, a squish and a wiggle -- and `Scoot_Move` is
    /// the one it crosses ground on; `Jump_Move` hops, which reads as a
    /// different creature at crowd distance and does not match the flat speed
    /// [`update`] moves it at. The ant has two, `bite` and `walk`, and only one
    /// of those is a cycle.
    ///
    /// Kept as an index rather than a name because that is all a glTF asset
    /// label can carry. `the_clip_index_is_still_the_walk` pins each index to
    /// the name it is supposed to be, so a re-export that reorders the file is
    /// a failing test rather than a crowd standing still and pulling faces.
    pub fn clip(self) -> usize {
        match self {
            Self::Slime => 6,
            Self::Ant => 1,
        }
    }

    /// Radius and height of its collision cylinder, **measured from the model**.
    ///
    /// Not written down here, and there is no scale factor anywhere in this file
    /// for one to drift out of step with: an actor is drawn at the size it was
    /// authored at, and this is that size read back out of the same glTF the
    /// renderer loads. Set an actor's size in Blender -- see
    /// `tools/resize_actor.py` -- and the crowd's spacing, its shove distances,
    /// the height the player has to be above to stomp one and the width of its
    /// shadow all follow, because every one of those is expressed in these.
    ///
    /// The radius is the furthest the mesh reaches from its origin in the
    /// horizontal plane, which for the ant is nose to tail rather than side to
    /// side. Footprint matters more than height does: a model wider than the
    /// cylinder that spaces it is a crowd that visibly interpenetrates.
    pub fn body(self) -> (f32, f32) {
        let size = self.size();
        (size.radius, size.height)
    }

    /// How far the model's geometry hangs below its own transform origin.
    ///
    /// The placement code treats an enemy's translation as the point its feet
    /// rest on, and for a rig whose root sits up inside the body that is simply
    /// not where its origin is. Seat the origin on the floor -- which is exactly
    /// what [`crawl`] does, two centimetres of skin above the surface it found
    /// -- and the bottom of the model is underground. That is the "scuttlebugs
    /// clipping through the floor" report, and it happened on flat stone with
    /// nothing steep anywhere near, which is why nothing about walls or ceilings
    /// or stale surface normals ever fixed it.
    ///
    /// Both actors that ship now are authored with their feet on `y = 0`, so
    /// both measure zero and nothing is lifted. Measuring rather than writing it
    /// down is what stops the correction outliving the model it was measured
    /// off: a re-exported ant that had *stopped* hanging spent a session
    /// floating a third of its own height above the ground on a constant nobody
    /// had re-measured.
    ///
    /// This is the bind pose, which is what a glTF's POSITION accessors hold.
    /// `tools/measure_actor_hang.py` is the authority on the difference: it
    /// evaluates the *skinned* mesh on every frame of every clip, which is what
    /// tells a permanent rig offset from a squash that dips through the floor
    /// plane for a few frames of a walk cycle. For both shipped actors the two
    /// agree.
    pub fn lift(self) -> f32 {
        self.size().lift
    }

    /// How big this actor is, measured once on first use and kept.
    fn size(self) -> Size {
        sizes()[self as usize]
    }

    /// How wide a shadow it casts, or `None` for an actor that is drawn
    /// without one.
    ///
    /// The ant's is narrower than its collision cylinder, which is deliberately
    /// generous so that walking near one of these counts as touching it. A
    /// shadow drawn at that width would stick out well past the model and read
    /// as a puddle. What is left is about the spread of six feet under a body
    /// held clear of the ground, which is what a blob shadow is for.
    ///
    /// The slime is drawn with no disc at all. It has no feet and nothing held
    /// clear: it is a dome resting on the lawn, and everything a disc under one
    /// can do is wrong. Inside its own radius it is hidden by the body from
    /// every angle a camera can reach -- which is what "the blobs have no
    /// shadow" looked like when it did have one. Wide enough to be seen past
    /// the body, it is a dark ring round a creature that is already touching
    /// the floor, and reads as the slime hovering over its own stain. So it
    /// gets nothing, and what grounds it is the contact itself.
    ///
    /// `None` rather than a zero radius so that both places that draw these --
    /// the disc under the near model and the square baked into the far crowd's
    /// mesh by [`crate::impostor::build_shadows`] -- have to say what they do
    /// about it, and cannot disagree.
    pub fn shadow_radius(self) -> Option<f32> {
        match self {
            Self::Slime => None,
            Self::Ant => Some(self.body().0 * 0.7),
        }
    }
}

/// Required rather than added at the spawn site, so that no enemy can exist
/// without a tier -- including the ones tests build by hand. A missing
/// `Detail` would not be a compile error, it would be an enemy silently left
/// out of every query that decides how much simulation it gets.
///
/// [`crate::path::Route`] rides along for the same reason and on the same
/// terms [`crate::squad::Ally`] requires one: a body that can be told to chase
/// something across a castle is a body that needs telling the way. An empty
/// route costs a vector that never allocates and means "walk straight at it",
/// which is what every enemy did before there was any of this -- and is what a
/// [`Crawler`] does for ever, because a body that can walk up the wall in front
/// of it does not want telling the way round. See [`navigate`].
#[derive(Component)]
#[require(Detail, crate::path::Route)]
pub struct Enemy {
    /// What it is, which is also its collision cylinder and its model: kept as
    /// the one fact rather than as a copy of each thing derived from it.
    pub kind: Kind,
    pub animation: Handle<AnimationClip>,
}

/// Which side of the fight a creature is on.
///
/// On the enemies, on the Marios, and on the player -- the player carries one
/// without being able to notice anything himself, because a side is what makes
/// him worth noticing. Everything [`alert`] does is asked in terms of this
/// rather than of what a creature is: a Mario looks for the nearest thing not
/// on its side exactly as a slime does.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Hostile,
    Friendly,
}

/// What a creature has noticed, which is the whole of whether it is coming for
/// you or ambling about -- and, once it is coming for something, which of the
/// several things worth coming for it has settled on.
///
/// Aggro is never lost by walking away. Once an enemy has seen you it comes
/// until it dies or something else takes its attention, which is why the target
/// is kept as *who* rather than as a flag: a second thing worth chasing is a
/// change of target rather than a special case.
///
/// **Acquiring is instant and switching is not**, and that asymmetry is the
/// whole design. A creature with nothing to chase takes the first thing it
/// notices, and the alarm chain in [`alert`] hands the same answer to its whole
/// neighbourhood on the tick it happens -- walking into the middle of a crowd
/// has to be one mistake rather than a series of small ones. But a creature
/// already in a fight is not re-pointed by anything; a rival has to *out-earn*
/// what it is already fighting, over seconds, and beat it by a margin.
///
/// Without that second half the two are the same mechanism and the field is
/// unusable: the chain is a flood fill, so one Mario landing a punch re-seeds
/// every enemy within earshot of every enemy within earshot of it, and the
/// whole crowd swaps ends every time anybody does anything. Attention that can
/// be *divided* has to accumulate.
#[derive(Component)]
pub struct Aggro {
    /// Who it is after, or `None` while nothing has its attention.
    pub target: Option<Entity>,
    /// Where that target was when [`alert`] last looked.
    ///
    /// The enemy walks to this rather than to the target itself, so it heads
    /// for the last place it knew of when the target is gone -- and so that
    /// the movement step needs nothing but the enemy's own components.
    pub at: Vec3,
    /// How wide the thing it is chasing is.
    ///
    /// **Everything that walks towards something has to know this**, because
    /// "walk to it" means walking to its *edge*: a chaser aimed at a body's
    /// centre is aimed at a place [`spread`] will not let it stand, and the two
    /// then argue about its position for as long as the fight lasts. What that
    /// looks like is a Mario standing inside an ant, vibrating.
    ///
    /// Carried on the aggro rather than looked up where it is needed, because
    /// [`update`] walks the field in parallel and cannot be reading other
    /// entities while it does. [`alert`] has already found the target once and
    /// fills this in as it writes [`Self::at`].
    pub room: f32,
    /// How much attention [`Self::target`] has earned, and is holding the fight
    /// with. Meaningless in absolute terms -- see `GameTuning::threat_near`;
    /// what it is for is being compared against [`Self::rivalry`].
    pub commitment: f32,
    /// The one rival being tracked, and what it has earned so far.
    ///
    /// **One slot, not a table.** Two hundred creatures are appraised in full
    /// every tick and a per-target score for each of them is a per-tick
    /// allocation this module has spent real effort not having -- see
    /// [`Noticing`]. One slot is enough because the decay does the pruning: a
    /// rival that stops earning shrinks out of the slot on its own, and the
    /// next claimant takes it as soon as its own credit is the larger of the
    /// two. What it costs is that a creature cannot track a slow third
    /// contender behind a fast second one, which in a fight nobody can see.
    pub contender: Option<Entity>,
    pub rivalry: f32,
}

impl Default for Aggro {
    fn default() -> Self {
        Self {
            target: None,
            at: Vec3::ZERO,
            // The player, who is what most things are chasing most of the time,
            // and the safest thing to assume for anything that sets a target
            // without going through `alert`.
            room: crate::player::PLAYER_RADIUS,
            commitment: 0.0,
            contender: None,
            rivalry: 0.0,
        }
    }
}

/// Something one side just did to the other, waiting to be noticed.
///
/// Queued rather than resolved where it happens, because the systems that kill
/// things are not the system that decides who is angry about it: a punch lands
/// in [`ally_combat`], a stomp in [`combat`] and a bullet in
/// [`crate::weapon::fly`], and all three want the same sentence written into
/// [`alert`]'s pass over the field. Sending it as an act rather than as an aggro
/// write is also what keeps the tier split honest -- who *witnessed* it is a
/// question about the near tier, and the killer does not know the answer.
#[derive(Clone, Copy)]
pub struct Threat {
    /// Who did it, which is what the witnesses turn on.
    pub by: Entity,
    /// Where it happened. Deliberately the *victim's* position rather than the
    /// killer's: what draws a crowd is where the body fell, and a Mario firing
    /// from behind you is not standing where the noise came from.
    pub at: Vec3,
}

/// The acts waiting to be noticed this tick.
///
/// Drained by [`alert`], which runs after everything that fills it except
/// [`crate::weapon::fly`] -- a bullet's kill is a tick late, which is a
/// thirtieth of a second nobody can see.
#[derive(Resource, Default)]
pub struct Threats {
    acts: Vec<Threat>,
}

impl Threats {
    /// Records a kill for whoever is near enough to have seen it.
    pub fn kill(&mut self, by: Entity, at: Vec3) {
        self.acts.push(Threat { by, at });
    }
}

/// How wide a berth an enemy gives what it is chasing, on top of the two bodies
/// involved, and how far that berth varies from one to the next.
///
/// Without it every enemy in a brood walks to the same point -- the target's
/// feet -- and a crowd chasing one player is a crowd converging on one spot,
/// which is a scrum that [`spread`] then spends the fight pushing apart. With
/// it they arrive around him instead.
///
/// **On top of the two bodies**, which it did not used to be. As an absolute
/// distance it was a standing instruction to walk 1.0 m from the target's
/// middle, which is fine against a slime 0.6 m across and is a metre and a half
/// inside an ant 5 m across. Every number in this file that says how close two
/// things get is measured between their surfaces for that reason; the actors
/// are authored art now and their size is not a constant anybody here gets to
/// assume.
const STAND_OFF: f32 = 0.6;
const STAND_OFF_SPREAD: f32 = 1.6;

/// The furthest past two bodies that [`Quirk::stand_off`] will ever park one
/// creature from what it is chasing.
///
/// Published because **anything that lands a blow has to reach at least this
/// far**, or the chaser walks to where it is allowed to stand and then cannot
/// hit what it walked to. That is the trap [`MARIO_REACH`] documents, and
/// [`crate::structure::REACH`] was in it: a berth of up to 2.2 m against a reach
/// of 1.6 meant the share of a crowd that drew a wide berth stood round a pylon
/// swinging at air, and a siege stalled with the mast on its last few points.
pub const CLOSEST_STAND: f32 = STAND_OFF + STAND_OFF_SPREAD;

/// How far off the straight line to its goal an enemy wanders, and how quickly
/// that wander swings from one side to the other.
const WEAVE_WIDTH: f32 = 0.9;
const WEAVE_RATE: f32 = 1.3;

/// How much faster or slower than the rest one enemy walks, as a fraction.
const PACE_SPREAD: f32 = 0.25;

/// The small differences between one enemy and the next.
///
/// All of it comes off a single number fixed when the enemy is placed, so a
/// brood out of one pipe is a crowd of individuals rather than a marching band,
/// and none of it needs a random number generator -- the field stays
/// reproducible, which is what lets a test walk one of these across the castle
/// and get the same answer twice.
#[derive(Component)]
pub struct Quirk {
    seed: f32,
}

impl Quirk {
    fn new(phase: f32) -> Self {
        Self {
            seed: phase * crate::squad::GOLDEN_ANGLE,
        }
    }

    /// The single number everything else here is derived from.
    ///
    /// Read by [`crate::impostor`] to offset one enemy's walk cycle from the
    /// next's, which is the same job it does for the pace and the weave: a
    /// crowd of sprites all on the same frame is a marching band.
    pub fn seed(&self) -> f32 {
        self.seed
    }

    /// Its own walking pace, as a multiple of the speed for its kind.
    fn pace(&self) -> f32 {
        1.0 + PACE_SPREAD * (self.seed * 1.7).sin()
    }

    /// The spot around a target it makes for, rather than the target itself.
    ///
    /// `bodies` is the two radii involved -- the target's and its own -- so the
    /// spot it picks is outside both of them however big either one is. Aiming
    /// at somewhere it cannot stand is the one thing this must not do: [`spread`]
    /// would spend the fight pushing it back out again.
    fn stand_off(&self, bodies: f32) -> Vec3 {
        let angle = self.seed * 2.3;
        let reach = bodies + STAND_OFF + STAND_OFF_SPREAD * (self.seed * 0.9).sin().abs();
        Vec3::new(angle.sin(), 0.0, angle.cos()) * reach
    }

    /// How far to one side of the straight line to its goal it is at `elapsed`,
    /// which is what keeps a group from converging along one bearing.
    fn weave(&self, elapsed: f32) -> f32 {
        WEAVE_WIDTH * (elapsed * WEAVE_RATE + self.seed).sin()
    }

    /// Where in its walk cycle it starts, in seconds.
    ///
    /// Read when a culled enemy's animation is started again on coming back
    /// into view. Starting every one of them at zero would put a crowd that
    /// crossed the draw distance together into perfect step, which is the one
    /// thing this whole component exists to prevent. A second is longer than
    /// either actor's clip, so the whole cycle is covered whatever its length --
    /// a seek past the end of a looping clip wraps.
    fn animation_phase(&self) -> f32 {
        (self.seed * 0.7).sin().abs()
    }
}

/// Where an enemy mills about while [`Aggro`] is empty.
#[derive(Component)]
pub struct Wander {
    /// The spot it was placed, which is the middle of its patch.
    home: Vec3,
    /// The spot in that patch it is walking to at the moment.
    goal: Vec3,
    /// How long it still has to stand about before it sets off again.
    rest_left: f32,
    /// Its own place in the sequence of spots, which is what keeps a brood out
    /// of lockstep with each other.
    phase: f32,
}

impl Wander {
    fn new(home: Vec3, phase: f32) -> Self {
        let mut wander = Self {
            home,
            goal: home,
            rest_left: 0.0,
            phase,
        };
        wander.somewhere_else();
        wander
    }

    /// Picks the next spot to amble to, and how long to stand about first.
    ///
    /// The golden angle, advanced per enemy: successive spots do not line up
    /// into a path that retraces itself, and two enemies never pick the same
    /// one at the same moment -- with no random number generator anywhere, so a
    /// field of them stays reproducible in a test. The same trick, for the same
    /// reasons, as [`crate::squad::Ally::amble_somewhere_else`].
    fn somewhere_else(&mut self) {
        self.phase += crate::squad::GOLDEN_ANGLE;
        let spread = |scale: f32| (self.phase * scale).sin().abs();
        let reach = WANDER_RADIUS * (0.4 + 0.6 * spread(0.37));
        self.goal = self.home + Vec3::new(self.phase.sin(), 0.0, self.phase.cos()) * reach;
        self.rest_left = WANDER_REST + WANDER_REST_SPREAD * spread(0.21);
    }

    /// Where it is walking this tick, or `None` while it is standing about.
    ///
    /// Arrival is measured in the horizontal plane on purpose. The spot is a
    /// place on the ground, and the ground under it is rarely at the height the
    /// enemy was placed at -- on a hill, or up a wall, a spot judged in three
    /// dimensions is one it can never quite reach.
    fn goal(&mut self, position: Vec3, dt: f32) -> Option<Vec3> {
        if self.rest_left > 0.0 {
            self.rest_left = (self.rest_left - dt).max(0.0);
            return None;
        }
        let there = Vec2::new(self.goal.x - position.x, self.goal.z - position.z);
        if there.length() < WANDER_ARRIVE {
            self.somewhere_else();
            return None;
        }
        Some(self.goal)
    }
}

/// An enemy that walks the level's surfaces rather than its floors.
///
/// An ant has six legs and no opinion about which way is down, so it
/// treats a wall and a ceiling as more floor: it keeps its own up vector, which
/// is the normal of whatever it is stuck to at the time, and everything it does
/// -- which way it steps, which way it faces, which way it probes -- is asked
/// relative to that rather than to the world's Y.
#[derive(Component)]
pub struct Crawler {
    /// Up for this bug: the normal of the surface under its feet.
    pub up: Vec3,
    /// The way it is walking: a unit vector lying in that surface, which is
    /// also the way its model is turned.
    ///
    /// Kept rather than worked out afresh each tick from where it wants to be,
    /// because where it wants to be can be behind a wall -- and a bug that
    /// reconsiders that every tick never gets anywhere. See [`TURN_RATE`].
    pub heading: Vec3,
}

impl Default for Crawler {
    fn default() -> Self {
        // Whatever it is eventually stuck to, it starts the right way up and
        // finds out on its first step.
        Self {
            up: Vec3::Y,
            heading: Vec3::Z,
        }
    }
}

/// Puts one enemy in the world.
///
/// Shared by the level's own placements and by the warp pipes so the two
/// cannot drift apart -- a pipe spawning something subtly different from what
/// the level places is exactly the kind of difference that is invisible until
/// it is a bug report.
pub fn spawn(
    commands: &mut Commands,
    assets: &AssetServer,
    kind: Kind,
    position: Vec3,
    phase: f32,
) -> Entity {
    let enemy = commands
        .spawn((
            Enemy {
                kind,
                animation: assets.load(format!("{}#Animation{}", kind.model(), kind.clip())),
            },
            // Its pool, sized by what it is. Every place that lands a blow on
            // one of these goes through `health::strike`, which treats an
            // enemy built without this component as one that dies to the first
            // hit -- so this line is what makes a kind's toughness real rather
            // than what makes it killable.
            crate::health::Health::new(kind.health()),
            Side::Hostile,
            Aggro::default(),
            Quirk::new(phase),
            Wander::new(position, phase),
            // No `WorldAssetRoot` and hidden to begin with: [`shed_scenes`]
            // builds the model on the first frame the enemy is near enough to
            // need one. Spawning it here instead would have a field of two
            // thousand build two thousand actor scenes and destroy all but a
            // couple of hundred of them again on the next frame.
            Visibility::Hidden,
            // No scale. The model is drawn at the size it was authored at, and
            // [`Kind::body`] is that size measured back off it.
            Transform::from_translation(position),
            // Parts of both of these are flat quads the original turns to face
            // the camera every frame.
            crate::billboard::BillboardActor,
        ))
        .insert_if(Crawler::default(), || kind == Kind::Ant)
        .insert_if(crate::n64::Translucent(kind.opacity()), || {
            kind.opacity() < 1.0
        })
        .id();
    // Nothing at all for a kind that is drawn without one: no component, so no
    // disc is ever attached and nothing has to be hidden afterwards.
    if let Some(radius) = kind.shadow_radius() {
        commands
            .entity(enemy)
            .insert(crate::shadow::ShadowCaster::new(radius, kind.body().1));
    }
    enemy
}

/// Where a benchmark crowd is centred, how far out it spreads, and the height
/// its floor query is asked from.
///
/// The centre and reach cover the castle grounds -- the collision spans roughly
/// x -82..82 and z -81..68 -- and the sky is above everything in it, so the
/// query finds the highest surface under each spot rather than starting inside
/// a hill.
const CROWD_CENTRE: Vec2 = Vec2::new(0.0, -6.0);
const CROWD_REACH: f32 = 70.0;
const CROWD_SKY: f32 = 90.0;

/// How many spots a crowd will try before giving up on reaching its count.
///
/// A spiral spot can land off the edge of the collision, where there is no
/// floor to stand something on, so the walk takes more spots than it places.
/// Four to one is far more slack than the castle needs and still terminates.
const CROWD_TRIES: usize = 4;

/// The spots a crowd of `count` is placed on: a sunflower spiral over the
/// castle, each one dropped onto whatever is under it.
///
/// A spiral by the golden angle with the radius going as the square root, which
/// is what spreads the field evenly instead of leaving it in rings -- the same
/// reason [`crate::squad::GOLDEN_ANGLE`] is used everywhere else here. No random
/// number generator, so `crowd 2000` puts the same field down every time and
/// two runs of the benchmark differ by the build rather than by the layout.
fn crowd_spots(count: usize, level: &LevelData) -> Vec<Vec3> {
    let mut spots = Vec::with_capacity(count);
    for step in 0..count * CROWD_TRIES {
        if spots.len() == count {
            break;
        }
        let angle = step as f32 * crate::squad::GOLDEN_ANGLE;
        // Against the count rather than the step, so the field covers the same
        // ground whether it holds two hundred or two thousand.
        let reach = CROWD_REACH * (step as f32 / count as f32).sqrt();
        let at = CROWD_CENTRE + Vec2::new(angle.sin(), angle.cos()) * reach;
        let sky = Vec3::new(at.x, CROWD_SKY, at.y);
        if let Some(floor) = level.floor_height(sky) {
            spots.push(Vec3::new(at.x, floor, at.y));
        }
    }
    spots
}

/// Carries out the console's `crowd` command.
///
/// Runs whether or not the console is open, because the console is open at the
/// moment the command is typed and a crowd that only appeared once you shut it
/// would be a crowd you never saw arrive.
pub fn crowd(
    mut commands: Commands,
    assets: Res<AssetServer>,
    level: Res<LevelData>,
    mut console: ResMut<ConsoleState>,
    existing: Query<Entity, With<Enemy>>,
) {
    // Taken before the loop and unconditionally: leaving a request in the queue
    // because this frame had nothing to do with it is a crowd placed again on
    // the next one.
    for request in console.take_requests() {
        let (count, kind) = match request {
            Request::ClearCrowd => (0, CrowdKind::Mix),
            Request::Crowd(count, kind) => (count, kind),
            // Not this system's request. Put back, so whoever it does belong
            // to still sees it -- `take_requests` drains the queue whole.
            other => {
                console.defer(other);
                continue;
            }
        };
        // Both cases clear first. `crowd 2000` twice is a field of two
        // thousand, not of four -- which is the whole point of a command whose
        // job is to make one number reproducible.
        for enemy in &existing {
            commands.entity(enemy).despawn();
        }
        for (index, spot) in crowd_spots(count, &level).into_iter().enumerate() {
            let kind = match kind {
                CrowdKind::Slime => Kind::Slime,
                CrowdKind::Ant => Kind::Ant,
                CrowdKind::Mix if index % 2 == 0 => Kind::Slime,
                CrowdKind::Mix => Kind::Ant,
            };
            spawn(&mut commands, &assets, kind, spot, index as f32);
        }
    }
}

/// How much of the simulation an enemy is getting this tick.
///
/// The crowd budget is a *count*, not a distance: the nearest
/// `sim_budget` enemies are simulated in full and everything else is carried by
/// the flow field, however many of them there are. That is what makes a field of
/// five thousand cost the same as a field of five hundred -- the expensive tier
/// has a fixed size, and the cheap tier is O(1) an enemy.
///
/// What [`Full`](Detail::Full) buys, and what a [`Crowd`](Detail::Crowd) enemy
/// therefore does without:
///
/// * collision against the level -- walls, ledges, the step-up rule
/// * being held out of its neighbours by [`spread`]
/// * the aggro chain in [`alert`], and being a thing the player can hit
/// * a crawler's ability to walk up a wall
///
/// None of that is visible on something the size of a thumbnail, which is the
/// only kind of enemy that is ever in the cheap tier.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Detail {
    #[default]
    Full,
    Crowd,
}

/// How much further than `enemy_draw` an enemy must get before its skinned
/// model is thrown away, and how much nearer than it before one is built again.
///
/// A tenth. Small enough that the swap still happens where it was asked for,
/// wide enough that an enemy ambling along the boundary does not spawn and
/// despawn a whole actor scene on alternate frames.
const SWAP_HYSTERESIS: f32 = 1.1;

/// How far across its route a crowd enemy is allowed to wander, as a fraction
/// of the route itself.
///
/// A flow field hands every enemy in a cell the *same* direction, so a crowd
/// following it walks in single file down one line -- which is both unlike the
/// near tier, whose enemies each pick their own spot around the target and
/// weave across the line to it, and unlike anything alive. The conga lines are
/// obvious from a distance and read as a thinner crowd than is actually there.
///
/// The same [`Quirk::weave`] the near tier uses, applied across the flow
/// instead of across a straight line to a goal, costs one sine a tick and
/// spreads the stream back out into a crowd.
const CROWD_WEAVE: f32 = 0.8;

/// How far along the flow field's route the cheap tier notices the player, as a
/// multiple of `enemy_sight`.
///
/// **One, deliberately: the cheap tier notices at exactly the range the
/// expensive one does, and no further.**
///
/// The temptation is to be generous here, because the cheap tier has no
/// [`alert`] chain -- a near enemy also hears about the player from a neighbour
/// who saw him, and comes from further than it can see, so an approximation
/// with no chain looks under-alert on paper. Both attempts at compensating for
/// that were wrong, and instructively so:
///
/// * four times sight, or about fifty metres, had the whole map charging the
///   player the instant it spawned. The lawn emptied and the field appeared to
///   have lost most of its enemies. It had not; they were all behind the camera.
/// * sight plus one hop of earshot, about twenty-three metres, was subtler and
///   still wrong. Every crowd enemy tests the range *independently*, so the
///   whole band inside it acquires at once -- where the real chain only spreads
///   between enemies that are near each other and only from one that has just
///   taken the alarm. The result was a visible front, with emptied lawn behind
///   it, at whatever radius was chosen.
///
/// The lesson is that an approximation of a chain is not a bigger radius, and
/// that the failure looks like missing enemies rather than like over-eager
/// ones. **A cheaper tier may look worse; it must not behave differently.**
/// Matching the near tier's own acquisition range under-alerts a little, in a
/// way that corrects itself the moment an enemy is promoted -- which is the
/// direction an error here should point.
const CROWD_SIGHT: f32 = 1.0;

/// Builds and throws away the skinned model as an enemy crosses `enemy_draw`.
///
/// Hiding a distant enemy stops it being *drawn*, which was the whole of the
/// first version of this and bought the draw calls back. What it does not stop
/// is the enemy existing: a slime is not one entity but a scene of about
/// nine, an ant about thirty-three, and every one of them is a
/// transform to propagate, a visibility to compute and an archetype row to walk
/// past. A mixed field of two thousand is some forty thousand entities,
/// and eight thousand is close to a hundred and seventy thousand -- at which point the entity
/// count, rather than the draws or the AI, is what a frame is made of. It was
/// measured: at eight thousand the simulation budget saved nothing at all,
/// because the simulation was no longer the expensive part.
///
/// So an enemy past the swap distance keeps only itself. Its `WorldAssetRoot`
/// is taken away, its children are despawned with it, and both come back the
/// moment the enemy is near enough to be drawn as a model again. What stands in
/// for it meanwhile is its impostor, which needs nothing but the root's own
/// transform.
///
/// **The children have to be despawned here by hand.** Removing a
/// `WorldAssetRoot` does not take the scene it spawned with it: the component's
/// `on_remove` hook calls `WorldInstanceSpawner::unregister_instance`, which is
/// documented as removing the spawner's record of the instance *without
/// despawning any entities*. The stale `WorldInstance` left on the enemy then
/// makes the re-insert worse rather than better -- the spawner asks for the old
/// instance to be despawned, finds no record of it, and drops the request -- so
/// an enemy that had walked out to `enemy_draw` and back was wearing two
/// skeletons, and three after the next round trip. It read as every enemy on
/// the field being doubled up, and it also meant the entity count this whole
/// function exists to hold down never actually fell.
///
/// The handle is not stored anywhere: `AssetServer::load` hands back the same
/// handle for the same path, so rebuilding it costs a lookup rather than a load.
pub fn shed_scenes(
    mut commands: Commands,
    assets: Res<AssetServer>,
    enemies: Query<(Entity, &Enemy, &Visibility, Option<&WorldAssetRoot>), With<Enemy>>,
) {
    for (entity, enemy, visibility, root) in &enemies {
        match (*visibility == Visibility::Hidden, root.is_some()) {
            // Gone far enough away to stop being a model. The enemy's only
            // children are the scene the root spawned, so all of them go.
            (true, true) => {
                commands
                    .entity(entity)
                    .remove::<WorldAssetRoot>()
                    .despawn_related::<Children>();
            }
            // Come near enough to be one again.
            (false, false) => {
                commands.entity(entity).insert(WorldAssetRoot(
                    assets.load(format!("{}#Scene0", enemy.kind.model())),
                ));
            }
            _ => {}
        }
    }
}

/// Spreads the alarm outward through the flow field while anything is chasing
/// the player.
///
/// This is the cheap tier's whole substitute for [`alert`]'s shouting chain,
/// and it exists because leaving it out was visibly wrong: a fully simulated
/// field of two thousand ends up converging on the player almost entirely, as
/// the chain cascades through a dense crowd within a tick or two, while a
/// tiered field left the far crowd ambling about. The difference read as the
/// crowd having lost most of its enemies.
///
/// The query is the whole cost: a scan that stops at the first enemy with the
/// player's scent.
pub fn rouse_crowd(
    time: Res<Time>,
    mut field: ResMut<crate::flow::FlowField>,
    hunters: Query<&Aggro, With<Enemy>>,
) {
    let roused = hunters.iter().any(|aggro| aggro.target.is_some());
    field.rouse(roused, time.delta_secs());
}

/// Sorts the field by distance and hands the nearest `sim_budget` the full
/// simulation.
///
/// The cutoff is found with `select_nth_unstable`, which partitions in linear
/// time rather than sorting -- the exact order of two thousand enemies is not
/// wanted, only the boundary between the nearest two hundred and the rest.
pub fn assign_detail(
    tuning: Res<GameTuning>,
    player: Query<&Transform, With<Player>>,
    mut enemies: Query<(&Transform, &mut Detail), With<Enemy>>,
    mut ranked: Local<Vec<f32>>,
) {
    let Ok(player) = player.single() else {
        return;
    };
    let here = player.translation;
    let budget = tuning.sim_budget.max(0.0) as usize;
    ranked.clear();
    ranked.extend(
        enemies
            .iter()
            .map(|(transform, _)| here.distance_squared(transform.translation)),
    );
    // Everything fits in the budget, so nothing is demoted. The common case
    // while the field is small, and worth not partitioning for.
    let cutoff = if ranked.len() <= budget {
        f32::INFINITY
    } else {
        *ranked.select_nth_unstable_by(budget, f32::total_cmp).1
    };
    for (transform, mut detail) in &mut enemies {
        let wanted = if here.distance_squared(transform.translation) < cutoff {
            Detail::Full
        } else {
            Detail::Crowd
        };
        // Assigned only on a change. Writing the same value back would mark the
        // component changed for every enemy in the field every tick, which is
        // the sort of thing that costs more than the system it belongs to.
        if *detail != wanted {
            *detail = wanted;
        }
    }
}

/// Everything that can be noticed: where it is and whose side it is on.
///
/// `Detail` is optional because the player and the Marios are creatures too and
/// carry no tier of their own -- they are always simulated in full.
type Creatures<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Side,
        Option<&'static Detail>,
        // How big it is, for whatever ends up chasing it. The player and the
        // Marios are not `Enemy` and answer with a Mario's own radius.
        Option<&'static Enemy>,
        // And nor is a pylon or a warp pipe, which are metres across and would
        // otherwise be walked to as though they were a Mario -- a crowd that
        // spends the whole fight standing *inside* the mast it is knocking
        // down. See `Aggro::room`, which is the field this fills.
        Option<&'static crate::structure::Structure>,
    ),
>;

/// Everything that does the noticing, which is the same crowd less the player,
/// who is told what to chase by whoever is holding the controller.
type Hunters<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Side,
        &'static mut Aggro,
        // What a Mario has decided to do with itself, which is the difference
        // between one coming to fight and one walking past on an errand. Absent
        // on everything that is not a Mario. See [`GameTuning::threat_focus`].
        Option<&'static crate::squad::Ally>,
        Option<&'static Detail>,
    ),
>;

/// Whether a creature is in the tier that gets the expensive treatment.
///
/// Anything without a tier -- the player, the Marios -- always is.
fn detailed(detail: Option<&Detail>) -> bool {
    detail != Some(&Detail::Crowd)
}

/// How many ticks apart one creature's appraisals of the field are.
///
/// A creature already in a fight looks up every sixth tick -- a fifth of a
/// second -- rather than every one. Two reasons, and the cheap one is not the
/// important one.
///
/// The cheap one: appraising means a nearest-not-my-side search, which the old
/// code skipped outright for anything that already had a target -- so in a
/// settled fight, where nearly everything has one, it did almost none of them.
/// Re-appraising is new work in exactly that case, and the stagger is what
/// keeps it to a sixth of the near tier a tick. The *peak* is unchanged either
/// way: a field where nothing has noticed anything yet searched with every
/// creature then and searches with every creature now.
///
/// The important one: a crowd that asks the question on the same tick answers
/// it on the same tick. Even with the margin doing its job, a row of ants whose
/// scores all cross at once still turns as one body, which is the look this is
/// here to avoid. The offset comes off `Entity::index`, which is sequential
/// across a brood out of one pipe -- so the phases spread evenly, for free, and
/// without a component or a random number anywhere.
///
/// It gates the *decision*, not just the looking, and that distinction is the
/// whole value of it. Attention arrives synchronously -- a kill is one event
/// credited to every witness on the same tick -- so a line of ants at much the
/// same range from the same Mario cross the margin on the same kill however
/// carefully the scores were accumulated. Staggering only the appraisal leaves
/// them turning in lockstep anyway; staggering the switch is what actually
/// spreads it out. That was not a prediction, it was a test asserting a crowd
/// would split and reporting that all eight of them turned on tick 16.
///
/// Acquiring from nothing is deliberately *not* staggered: a creature with no
/// target looks every tick, exactly as it always did, so nothing about walking
/// into a crowd got slower.
const ATTENTION_SPAN: u32 = 6;

/// The least attention a live target may be holding a fight with.
///
/// Without it a target that has walked out of sight decays to nothing, and then
/// the next creature to wander past takes the fight on its first credit -- the
/// chaotic switch this is all built to prevent, arrived at from underneath.
/// The floor means stealing a fight always costs a rival some seconds of
/// out-earning, even when the incumbent is long gone.
const COMMITMENT_FLOOR: f32 = 1.0;

/// One creature's attention as [`alert`] works on it: read out of its [`Aggro`],
/// argued over by everything below, and written back at the end.
///
/// A flat array of these rather than the components themselves, because the
/// alarm reaches sideways -- what one creature notices is credited to its
/// neighbours -- and a query cannot be held open at two rows at once.
#[derive(Clone, Copy)]
struct Hunt {
    entity: Entity,
    at: Vec3,
    side: Side,
    target: Option<Entity>,
    commitment: f32,
    contender: Option<Entity>,
    rivalry: f32,
    /// Whether this one both looks at the field and acts on what it holds, this
    /// tick. See [`ATTENTION_SPAN`].
    appraising: bool,
}

impl Hunt {
    /// Takes `who` outright, for `amount`.
    ///
    /// What acquiring from nothing does, and the only path that moves a target
    /// without going through the margin. See [`Aggro`] for why those are two
    /// different things.
    fn seize(&mut self, who: Entity, amount: f32) {
        self.target = Some(who);
        self.commitment = amount.max(COMMITMENT_FLOOR);
        self.contender = None;
        self.rivalry = 0.0;
    }

    /// Puts `amount` of attention on `who`, wherever it belongs.
    ///
    /// A claimant that is neither the target nor the tracked rival takes the
    /// rival slot only once its own credit beats what is already in there, so
    /// the slot belongs to whoever has earned the most rather than to whoever
    /// acted last. That is the same rule the switch uses, one level down -- and
    /// with the decay above it, a rival that stops earning shrinks out of the
    /// slot on its own and lets the next one in.
    fn credit(&mut self, who: Entity, amount: f32) {
        if self.target == Some(who) {
            self.commitment += amount;
        } else if self.contender == Some(who) {
            self.rivalry += amount;
        } else if amount > self.rivalry {
            self.contender = Some(who);
            self.rivalry = amount;
        }
    }
}

/// The working set [`alert`] keeps between ticks: the two crowds it sorts, the
/// two grids it buckets them into, and the buffers the flood fill walks.
///
/// Fields rather than locals so that noticing allocates nothing. Every one of
/// these was a fresh `Vec` or a fresh `HashMap` a tick, which is a handful of
/// microseconds nobody would notice on its own and an allocator that never
/// settles across the whole schedule.
#[derive(Default)]
pub struct Noticing {
    crowd: Vec<(Entity, Vec3, Side)>,
    hunting: Vec<Hunt>,
    seen: Neighbourhood,
    earshot_grid: Neighbourhood,
    nearby: Vec<usize>,
    shouting: Vec<usize>,
    decided: Vec<Hunt>,
    /// Who is already coming for whom, sorted by the one doing the coming. See
    /// [`GameTuning::threat_focus`], which is what it is read for.
    engaged: Vec<(Entity, Entity)>,
    /// Which slice of the field appraises on this tick. See [`ATTENTION_SPAN`].
    tick: u32,
}

/// Who has noticed whom, who has been told about it, and who has earned enough
/// of somebody's attention to take it off whoever had it.
///
/// One rule for both sides. A creature with nothing to chase looks for the
/// nearest creature not on its side within `enemy_sight`; the moment it finds
/// one, everything on *its* side within `enemy_alert` hears about it, and
/// everything within `enemy_alert` of them hears it in turn, until the chain
/// runs out of neighbours. A crowd with one lookout in it all turns round at
/// once, which is what makes walking into the middle of one a mistake rather
/// than a series of separate small ones.
///
/// A creature that is *already* fighting something is not in that chain, and
/// that is the difference between this and what it replaced. It hears the same
/// shouts and sees the same neighbours, but they arrive as attention rather
/// than as orders: three sources feed it -- who is nearest, what the
/// neighbourhood is shouting about, and who has just killed something in front
/// of it -- and all three decay. Only when one rival is holding `threat_switch`
/// times what the current fight is holding does anything actually turn. So a
/// squad of Marios wading into the fight you are in pulls the enemies off you
/// over a couple of seconds, in ones and twos, rather than all at once on the
/// tick of the first punch.
///
/// Only creatures that took the alarm this tick pass it on, plus the ones that
/// just switched. One already in a fight is not shouting about it forever, or
/// the alarm would creep across the whole field through whatever incidental
/// pairs drift within earshot.
///
/// Nothing is ever given up on except by dying: aggro is not a leash, and an
/// enemy that has seen you comes until it or you are gone, or until something
/// else has earned it. What it does lose outright is a target that has been
/// despawned, which is not giving up but having won.
pub fn alert(
    tuning: Res<GameTuning>,
    mut threats: ResMut<Threats>,
    everyone: Creatures,
    mut hunters: Hunters,
    mut scratch: Local<Noticing>,
) {
    let Noticing {
        crowd,
        hunting,
        seen,
        earshot_grid,
        nearby,
        shouting,
        decided,
        engaged,
        tick,
    } = &mut *scratch;
    *tick = tick.wrapping_add(1);
    // Only the tier being simulated in full takes part. Everything this system
    // does is quadratic-ish in the crowd it is handed -- two spatial grids and a
    // flood fill -- so handing it two hundred rather than two thousand is most
    // of what the budget buys. The rest of the field notices the player through
    // the flow field instead, in [`update`], at no cost at all.
    crowd.clear();
    crowd.extend(
        everyone
            .iter()
            .filter(|(_, _, _, detail, _, _)| detailed(*detail))
            .map(|(entity, transform, side, _, _, _)| (entity, transform.translation, *side)),
    );
    // Everyone who does the noticing, with last tick's attention aged. The
    // decay is what keeps a fight current rather than settling it on whoever
    // got there first: a Mario that stops killing stops being interesting.
    let keep = (1.0 - tuning.threat_decay * FIXED_DT).clamp(0.0, 1.0);
    hunting.clear();
    hunting.extend(
        hunters
            .iter()
            .filter(|(_, _, _, _, _, detail)| detailed(*detail))
            .map(|(entity, transform, side, aggro, _, _)| {
                // Something no longer in the world is no longer anything. This
                // is how a Mario that has just flattened a slime goes looking
                // for the next one rather than standing over the spot.
                let live = |who: Option<Entity>| who.filter(|who| everyone.get(*who).is_ok());
                let target = live(aggro.target);
                let contender = live(aggro.contender);
                Hunt {
                    entity,
                    at: transform.translation,
                    side: *side,
                    target,
                    commitment: match target {
                        Some(_) => (aggro.commitment * keep).max(COMMITMENT_FLOOR),
                        None => 0.0,
                    },
                    contender,
                    rivalry: match contender {
                        Some(_) => aggro.rivalry * keep,
                        None => 0.0,
                    },
                    // `Entity::index` is an `EntityIndex`; the raw number the
                    // phase comes off is one call further in.
                    appraising: target.is_none()
                        || entity.index().index().wrapping_add(*tick) % ATTENTION_SPAN == 0,
                }
            }),
    );

    // Who is already coming for whom. Built from the same snapshot the scores
    // are argued over, so it is what every hunter had decided at the top of the
    // tick rather than a half-updated picture of what they are deciding now.
    //
    // **A target is not an intention.** Aggro is never given up -- see this
    // function's own preamble -- so nearly every Mario on the field has
    // something filed against it, including the ones fetching balls at the
    // other end of the lawn. What separates "coming for you" from "has you
    // written down" is the plan, and only a Mario has one: an enemy walking
    // towards its target is doing the only thing it does.
    engaged.clear();
    engaged.extend(
        hunters
            .iter()
            .filter(|(_, _, _, _, _, detail)| detailed(*detail))
            .filter_map(|(entity, _, _, aggro, ally, _)| {
                let target = aggro.target?;
                let coming =
                    ally.is_none_or(|ally| matches!(ally.plan, crate::goap::Goal::Fight { .. }));
                coming.then_some((entity, target))
            }),
    );
    engaged.sort_unstable_by_key(|(who, _)| *who);
    // Whether `who` is on its way to fight `me`, which is the one question
    // [`GameTuning::threat_focus`] is asked about. Sorted and searched for
    // [`Hunt`]'s reason: built once a tick and read once per candidate, where a
    // `HashMap` would be a SipHash of every key on both sides.
    let coming_for = |who: Entity, me: Entity| {
        engaged
            .binary_search_by_key(&who, |(who, _)| *who)
            .is_ok_and(|found| engaged[found].1 == me)
    };
    // Never below one: a creature that preferred what was *ignoring* it would
    // be the same bug this fixes, pointing the other way. The slider stops
    // there too, and this is the guard for a saved file that does not.
    let focus = tuning.threat_focus.max(1.0);

    let sight = tuning.enemy_sight;
    let earshot = tuning.enemy_alert;
    seen.fill(crowd.iter().map(|(_, at, _)| *at), sight);
    earshot_grid.fill(hunting.iter().map(|hunt| hunt.at), earshot);
    shouting.clear();

    // What each creature can see for itself. A creature with nothing to chase
    // takes the nearest outright and shouts; one already in a fight only scores
    // what it sees, and only on its own tick -- see [`ATTENTION_SPAN`].
    for (index, hunt) in hunting.iter_mut().enumerate() {
        let Hunt {
            entity,
            at,
            side,
            target,
            appraising,
            ..
        } = *hunt;
        if !appraising {
            continue;
        }
        // What something at `range` is worth per look, and the whole of
        // [`GameTuning::threat_focus`]: something with its fists up is worth
        // several of something the same distance off that is walking past.
        // Applied to the fight it is already in as well as to what it can see,
        // and both halves are load-bearing -- the boost that pulls a creature
        // onto the Mario punching it is the same boost that has to keep it
        // there once it has turned, or it turns straight back on the next look.
        let pressing = |who: Entity, range: f32| {
            tuning.threat_near
                * (1.0 - range / sight)
                * if coming_for(who, entity) { focus } else { 1.0 }
        };
        // The fight it is already in keeps earning while it is still in front
        // of it. Without this the score is only ever "who is nearest" and the
        // margin buys a delay rather than a decision: something that already
        // has a creature's attention *and* is standing on it has to be harder
        // to pull it away from than something that has only the second.
        if let Some((held, held_at)) = target.and_then(|held| {
            everyone
                .get(held)
                .ok()
                .map(|(_, held_at, _, _, _, _)| (held, held_at))
        }) {
            let range = at.distance(held_at.translation);
            if range < sight {
                hunt.commitment += pressing(held, range);
            }
        }
        seen.near(at, nearby);
        // **Weighed rather than measured.** Taking the nearest outright is what
        // sends a creature past the Mario squarely in front of it after
        // something a metre closer that has not noticed it -- and, once it is
        // chasing that, past every Mario it walks through on the way. The
        // nearest is still what wins between two things that are equally
        // interested in it, because at equal weight the score is the range.
        let quarry = nearby
            .iter()
            .map(|&other| crowd[other])
            .filter(|(_, _, theirs)| *theirs != side)
            .filter_map(|(other, theirs, _)| {
                let range = at.distance(theirs);
                (range < sight).then(|| (other, pressing(other, range)))
            })
            .max_by(|(_, a), (_, b)| a.total_cmp(b));
        let Some((quarry, pressing)) = quarry else {
            continue;
        };
        // Already credited just above, as the fight it is in.
        if Some(quarry) == target {
            continue;
        }
        match target {
            None => {
                hunt.seize(quarry, pressing);
                shouting.push(index);
            }
            Some(_) => hunt.credit(quarry, pressing),
        }
    }

    // And what has just been done in front of it. A kill is the one source loud
    // enough to take a fight off somebody who is standing right there: it is
    // worth about a second of proximity, so it takes three or four of them in
    // the same corner to turn a crowd -- which is exactly the moment a squad of
    // Marios ought to become the more pressing problem.
    for act in threats.acts.iter() {
        let Ok((_, _, done_by, _, _, _)) = everyone.get(act.by) else {
            continue;
        };
        earshot_grid.near(act.at, nearby);
        for &other in nearby.iter() {
            let Hunt {
                at, side, target, ..
            } = hunting[other];
            // Its own side's kills are good news.
            if side == *done_by {
                continue;
            }
            let range = at.distance(act.at);
            if range >= earshot {
                continue;
            }
            let weight = tuning.threat_kill * (1.0 - range / earshot);
            match target {
                None => {
                    hunting[other].seize(act.by, weight);
                    shouting.push(other);
                }
                Some(_) => hunting[other].credit(act.by, weight),
            }
        }
    }
    threats.acts.clear();

    // The switch itself, and the only place a fight changes hands. Everything
    // above this only moves numbers around.
    let margin = tuning.threat_switch.max(1.0);
    for (index, hunt) in hunting.iter_mut().enumerate() {
        let Some(contender) = hunt.contender else {
            continue;
        };
        // On its own tick, which is what keeps a crowd from turning as one
        // body when one kill pushes all of them over the line at once.
        if !hunt.appraising || hunt.rivalry <= hunt.commitment * margin {
            continue;
        }
        hunt.seize(contender, hunt.rivalry);
        // And it says so, which is what spreads a swap through a crowd
        // *gradually*: its neighbours are told about the Mario rather than
        // handed it, so they turn a second or two later, in ones and twos, and
        // only if it goes on earning.
        shouting.push(index);
    }

    // The chain: everything on the same side within earshot of a shout hears
    // what the shouter is fighting. What it does about that is the whole of the
    // difference between acquiring and switching.
    while let Some(index) = shouting.pop() {
        let Hunt {
            at: from,
            side,
            target,
            ..
        } = hunting[index];
        let Some(target) = target else {
            continue;
        };
        earshot_grid.near(from, nearby);
        for &other in nearby.iter() {
            if other == index {
                continue;
            }
            let Hunt {
                at,
                side: theirs,
                target: held,
                ..
            } = hunting[other];
            if theirs != side || at.distance_squared(from) >= earshot * earshot {
                continue;
            }
            match held {
                // Nothing to lose: it takes the alarm whole and passes it on,
                // which is the cascade this chain has always been.
                None => {
                    hunting[other].seize(target, tuning.threat_shout);
                    shouting.push(other);
                }
                // Already in a fight: it hears about this one and goes back to
                // its own. Re-pointing it here is what used to let one punch
                // swing a whole field round.
                Some(_) => hunting[other].credit(target, tuning.threat_shout),
            }
        }
    }

    // Written back by entity rather than by position in the iteration, which is
    // not a thing to bet a fight on. Sorted and searched rather than hashed:
    // this is looked up once per hunter and built once per tick, and a `HashMap`
    // is a SipHash of every key on both sides of that.
    decided.clear();
    decided.extend(hunting.iter().copied());
    decided.sort_unstable_by_key(|hunt| hunt.entity);
    let decided = |entity: Entity| {
        decided
            .binary_search_by_key(&entity, |hunt| hunt.entity)
            .ok()
            .map(|found| decided[found])
    };
    for (entity, transform, _, mut aggro, _, detail) in &mut hunters {
        // A crowd-tier enemy keeps whatever it was chasing when it was last in
        // the near tier, and is not given a new target here. Losing one by
        // walking away would break the rule that aggro is never given up.
        //
        // A target that has *died* is a different matter, and it is the one
        // thing this loop still owes the cheap tier. `crowd_step` acquires only
        // while the slot is empty and has no way to ask whether what is in it
        // still exists, so a stale entity sat there forever: the enemy chased a
        // corpse at chase speed, could never notice anything else, and -- worse
        // than either -- answered `rouse_crowd` as something that has seen the
        // player, which holds the field's alarm on for the entire crowd from
        // the first Mario that goes down. Emptying the slot is all that is
        // needed; the flow field is this tier's eyes, and it re-notices through
        // that on the next tick exactly as it did the first time.
        if !detailed(detail) {
            if aggro.target.is_some_and(|held| everyone.get(held).is_err()) {
                aggro.target = None;
                aggro.commitment = 0.0;
                aggro.contender = None;
                aggro.rivalry = 0.0;
                aggro.at = transform.translation;
            }
            continue;
        }
        let Some(verdict) = decided(entity) else {
            continue;
        };
        aggro.target = verdict.target;
        aggro.commitment = verdict.commitment;
        aggro.contender = verdict.contender;
        aggro.rivalry = verdict.rivalry;
        // Where that target is now, and how wide, for the movement step to
        // walk towards the edge of rather than into.
        match verdict.target.and_then(|target| everyone.get(target).ok()) {
            Some((_, seen, _, _, kind, building)) => {
                aggro.at = seen.translation;
                // Whatever the thing actually is: a creature's cylinder, a
                // building's footprint, or -- for the player and the Marios,
                // which are neither -- a Mario's own radius.
                aggro.room = match (kind, building) {
                    (Some(enemy), _) => enemy.kind.body().0,
                    (None, Some(structure)) => structure.radius,
                    (None, None) => crate::player::PLAYER_RADIUS,
                };
            }
            None => aggro.at = transform.translation,
        }
    }
}

/// The enemies that are held out of each other. Anything still flying the arc a
/// pipe threw it is left out, for the same reason the movement step leaves it
/// out: a shove during the launch is a launch that lands somewhere else.
///
/// The `Without`s are not decoration. This runs beside [`Marios`] and the
/// player's own query in one system, and Bevy refuses two `&mut Transform`
/// queries whose rows it cannot *prove* disjoint -- at initialisation, which in
/// a windowed build is a panic nobody sees the message for.
type Jostling<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Enemy,
        &'static mut Transform,
        Option<&'static Crawler>,
        &'static Detail,
    ),
    (
        Without<Player>,
        Without<crate::squad::Ally>,
        Without<crate::pipe::Launched>,
    ),
>;

/// The Marios, who are held out of each other and out of everything else by the
/// same pass. They were held out of nothing at all before it.
type Marios<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform),
    (
        With<crate::squad::Ally>,
        Without<Player>,
        Without<Enemy>,
        Without<crate::pipe::Launched>,
    ),
>;

/// One body in the press, reduced to what a shove needs to know.
struct Body {
    /// Who it is, which the shove does not need but the write-back checks: the
    /// pushes are applied by walking the same queries a second time in the same
    /// order, and "the same order" is worth an assertion in debug builds.
    who: Entity,
    at: Vec3,
    /// The cylinder it is resolved as, which is the one it is already fought as.
    radius: f32,
    height: f32,
    /// Which way is up *for this body*: a crawler is pushed within the surface
    /// it is stuck to, because shoving a bug off its wall is the one thing this
    /// must not do.
    up: Vec3,
    /// Whether this is a cheap-tier body. It is shoved a quarter as often, and
    /// its shove is resolved against the flow field's idea of a wall rather than
    /// against the level itself.
    crowd: bool,
}

impl Body {
    /// How many fixed ticks pass between two shoves of this body.
    fn stride(&self) -> u32 {
        match self.crowd {
            true => CROWD_SPREAD_STRIDE,
            false => 1,
        }
    }
}

/// The working set [`spread`] keeps between ticks, so that holding a field of
/// two thousand apart allocates nothing at all after the first tick.
#[derive(Default)]
pub struct Press {
    bodies: Vec<Body>,
    pushes: Vec<Vec3>,
    grid: Neighbourhood,
    near: Vec<usize>,
}

/// Holds every creature out of every other one, so that a crowd converging on a
/// player stays a crowd rather than a single stack of models.
///
/// A positional shove rather than a force: they are already resolved against the
/// player as cylinders, and this resolves them against each other the same way.
/// Crawlers are pushed within the surface they are stuck to, and everything else
/// within the horizontal plane, where its own floor query will catch it.
///
/// **The Marios are in this and were not.** [`crate::squad::move_allies`] walks
/// each one to its slot and asks nothing about what else is standing there, so a
/// squad following the player was a heap of Marios in the same place walking
/// through each other and through him. That they are held apart by the enemies'
/// pass rather than by one of their own is the point: a Mario, a slime and an
/// ant are all bodies of some radius standing on some ground, and two
/// separate answers to how bodies avoid each other is two answers that disagree
/// at the boundary between them.
///
/// **The whole field is held apart, not just the near tier.** It used to be the
/// near tier only, on the grounds that two distant slimes standing in the same
/// spot are two pixels standing in the same spot. They are, but a thousand of
/// them are a crowd that has visibly collapsed into a line of stacks, and an
/// enemy promoted out of one arrives already inside its neighbours. What made
/// that affordable is the three things a crowd of two thousand needs and a crowd
/// of two hundred does not: a [`Neighbourhood`] that hashes into flat arrays
/// instead of allocating a list per cell, a squared-distance reject so the
/// square root is only paid for pairs that actually overlap, and a
/// [`CROWD_SPREAD_STRIDE`] that shoves a quarter of the cheap tier per tick
/// rather than all of it every tick.
pub fn spread(
    level: Res<LevelData>,
    field: Res<crate::flow::FlowField>,
    mut enemies: Jostling,
    mut allies: Marios,
    player: Query<(Entity, &Transform), With<Player>>,
    mut press: Local<Press>,
    mut fixed_tick: Local<u32>,
) {
    *fixed_tick = fixed_tick.wrapping_add(1);
    let tick = *fixed_tick;
    let Press {
        bodies,
        pushes,
        grid,
        near,
    } = &mut *press;
    bodies.clear();
    for (entity, enemy, transform, crawler, detail) in &enemies {
        let (radius, height) = enemy.kind.body();
        bodies.push(Body {
            who: entity,
            at: transform.translation,
            radius,
            height,
            up: crawler.map_or(Vec3::Y, |crawler| crawler.up),
            crowd: *detail == Detail::Crowd,
        });
    }
    // Where the enemies stop and the Marios start, since the two are written
    // back through different queries.
    let enemy_count = bodies.len();
    for (entity, transform) in &allies {
        bodies.push(Body {
            who: entity,
            at: transform.translation,
            // A Mario is a Mario: the same body the player is resolved against
            // walls with, rather than a second number that would drift from it.
            radius: crate::player::PLAYER_RADIUS,
            height: crate::player::PLAYER_HEIGHT,
            up: Vec3::Y,
            crowd: false,
        });
    }
    // Solid, and last, so that everything from here on is a body that pushes
    // without being pushed.
    let fixed = bodies.len();
    for (entity, transform) in &player {
        bodies.push(Body {
            who: entity,
            at: transform.translation,
            radius: crate::player::PLAYER_RADIUS,
            height: crate::player::PLAYER_HEIGHT,
            up: Vec3::Y,
            crowd: false,
        });
    }

    let widest = bodies
        .iter()
        .fold(0.0_f32, |most, body| most.max(body.radius));
    grid.fill(
        bodies.iter().map(|body| body.at),
        widest * 2.0 + PERSONAL_SPACE,
    );
    pushes.clear();
    pushes.resize(fixed, Vec3::ZERO);
    for (index, body) in bodies.iter().enumerate().take(fixed) {
        // Whose turn it is. Offset by the body's own index rather than run for
        // the whole cheap tier on every fourth tick, so the work is a quarter of
        // the field every tick instead of the whole field in a spike.
        if !(tick + index as u32).is_multiple_of(body.stride()) {
            continue;
        }
        let mut push = Vec3::ZERO;
        grid.near(body.at, near);
        for &other in near.iter() {
            if other == index {
                continue;
            }
            let theirs = &bodies[other];
            let room = body.radius + theirs.radius + PERSONAL_SPACE - SPREAD_SLACK;
            let apart = body.at - theirs.at;
            // Squared first. Most of what a bucket hands back is not touching --
            // it is in the same cell, or in a cell that hashed to the same
            // bucket -- and this is the test that throws those away, so it is
            // the one that must not contain a square root. The length below is
            // only ever taken for a pair that really is overlapping.
            if apart.length_squared() >= room * room {
                continue;
            }
            let overlap = room - apart.length();
            // Stood in exactly the same place, which two spawned by the same
            // pipe on the same tick genuinely are, there is no direction to be
            // pushed in and one has to be invented. The golden angle again, so
            // that a pile does not unfold along one line.
            let away = tangent(apart, body.up);
            let away = if away == Vec3::ZERO {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                tangent(Vec3::new(angle.sin(), 0.0, angle.cos()), body.up)
            } else {
                away
            };
            // A pair normally closes its gap at twice the rate, because both
            // ends are shoved. Against something that will not move -- the
            // player -- there is only one end, so it takes the whole share or
            // it is walked through.
            let share = if other < fixed { 1.0 } else { 2.0 };
            // A body shoved once every four ticks has to take out four ticks'
            // worth of overlap, or a strided crowd untangles four times slower
            // than the tier it is promoted into. Capped, because a shove that
            // takes out the *whole* overlap in one go is one that overshoots and
            // comes back next time -- which is the jitter [`SPREAD_RATE`] exists
            // to prevent.
            let rate = (SPREAD_RATE * body.stride() as f32).min(SPREAD_RATE_CAP);
            push += away * overlap * rate * share;
        }
        pushes[index] = push;
    }

    // Applied by walking the same two queries again in the same order rather
    // than by looking each body up: a random `get_mut` per shoved body is a
    // hash lookup and an archetype jump, and there are ten times as many of
    // them as there were. Bevy iterates a query in archetype order and nothing
    // here changes an archetype, so the second pass lines up with the first --
    // asserted in debug builds rather than assumed.
    let shove = |body: &Body, push: Vec3| -> Vec3 {
        let push = push.clamp_length_max(SPREAD_LIMIT * FIXED_DT * body.stride() as f32);
        let moved = body.at + push;
        // A press of bodies leaning on the one at the front is enough to post it
        // through a fence, and neither the walk step nor the crowd step gets a
        // say in where the shove puts it. So the shove resolves its own result,
        // as the cylinder it was just pushed as.
        //
        // Not for crawlers: a bug is held out of walls by being *on* one, and
        // `resolve_walls` would push it off the surface it is standing on.
        if body.up != Vec3::Y {
            return moved;
        }
        match body.crowd {
            // The cheap tier asks the level nothing, here as everywhere else.
            // The flow field already knows what stands between two cells, and it
            // is the same rule [`crowd_step`] walks by -- a shove that could put
            // an enemy somewhere its own next step would refuse to go is a shove
            // that has to be refused too. Probed a body's width past where the
            // shove lands for the same reason the step is: a press of a hundred
            // leaning on the one at the front is the strongest thing on the
            // field for putting a body somewhere it does not fit, and refusing
            // it as a point is refusing it in the one place its width is what
            // matters.
            true => {
                let push = moved - body.at;
                let reach = push + push.normalize_or_zero() * body.radius;
                match field.clear(body.at, moved) && !field.walled(body.at, body.at + reach) {
                    true => moved,
                    false => body.at,
                }
            }
            // `Vec3::Y` and not `body.up`: the branch above has already
            // sent every enemy standing on anything else away.
            false => level.resolve_walls(moved, Vec3::Y, body.radius, body.height),
        }
    };
    for (index, (entity, _, mut transform, _, _)) in enemies.iter_mut().enumerate() {
        let body = &bodies[index];
        debug_assert_eq!(body.who, entity, "the jostling query changed order");
        if pushes[index] != Vec3::ZERO {
            transform.translation = shove(body, pushes[index]);
        }
    }
    for (index, (entity, mut transform)) in allies.iter_mut().enumerate() {
        let body = &bodies[enemy_count + index];
        debug_assert_eq!(body.who, entity, "the Mario query changed order");
        if pushes[enemy_count + index] != Vec3::ZERO {
            transform.translation = shove(body, pushes[enemy_count + index]);
        }
    }
}

/// A crowd bucketed by where its members are standing, so that "everyone near
/// this one" costs its neighbours rather than the whole field.
///
/// Square cells in the horizontal plane, looked up nine at a time. Height is
/// left to the caller's own distance check: enemies are spread over a castle
/// rather than a tower, and a third axis of buckets would be mostly empty.
///
/// **A flat spatial hash rather than a map of cells to lists.** The first
/// version was a `HashMap<(i32, i32), Vec<usize>>`, which is a SipHash of two
/// integers per point plus a heap allocation per occupied cell, rebuilt from
/// nothing every tick. This is a counting sort into two integer arrays that the
/// caller keeps between ticks: [`Self::begins`] says where each bucket's run
/// starts in [`Self::items`], and `items` holds the point indices sorted by
/// bucket. Two linear passes, and no allocation at all once the buffers have
/// grown to the size of the field.
///
/// Cells are *hashed* rather than addressed, so the table is the size of the
/// crowd rather than the size of the castle, and two cells at opposite ends of
/// the map can share a bucket. That makes a query's answer a list of candidates
/// rather than a list of neighbours -- every caller measures the distance
/// anyway, since nine square cells are not a circle either. What a collision may
/// *not* do is hand the same point back twice, which would let a shove count one
/// neighbour double, so the nine bucket indices are deduplicated before they are
/// read rather than the points afterwards.
#[derive(Default)]
struct Neighbourhood {
    cell: f32,
    /// One less than the bucket count, which is a power of two so that the
    /// hash is masked rather than divided.
    mask: u32,
    /// Where each bucket's run of [`Self::items`] begins. One longer than the
    /// bucket count, so bucket `b` is `begins[b]..begins[b + 1]`.
    begins: Vec<u32>,
    items: Vec<u32>,
    /// Which bucket each point landed in, kept from the counting pass to the
    /// placing one, and a write cursor per bucket. Scratch, and fields only so
    /// that they are allocated once rather than once a tick.
    keys: Vec<u32>,
    cursor: Vec<u32>,
}

impl Neighbourhood {
    /// Buckets `points` into cells of `cell` on a side, which must be at least
    /// the distance the caller intends to ask about -- [`Self::near`] looks one
    /// cell out in each direction and no further.
    ///
    /// `ExactSizeIterator` rather than any old iterator because the table is
    /// sized off the crowd before a single point is hashed, and counting them
    /// twice to find that out would cost more than the hashing.
    fn fill(&mut self, points: impl ExactSizeIterator<Item = Vec3>, cell: f32) {
        // Two buckets a point, so that a crowd of any size mostly finds its own
        // cell alone in one and the loads stay short.
        let buckets = (points.len() * 2).next_power_of_two().max(64);
        self.mask = buckets as u32 - 1;
        self.cell = cell.max(0.001);
        self.keys.clear();
        self.begins.clear();
        self.begins.resize(buckets + 1, 0);
        for point in points {
            let key = Self::bucket(self.mask, Self::at(self.cell, point));
            self.keys.push(key);
            // Counted one slot to the right, so that the prefix sum below turns
            // the counts into starts in place.
            self.begins[key as usize + 1] += 1;
        }
        for index in 1..self.begins.len() {
            self.begins[index] += self.begins[index - 1];
        }
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.begins[..buckets]);
        self.items.clear();
        self.items.resize(self.keys.len(), 0);
        for index in 0..self.keys.len() {
            let slot = &mut self.cursor[self.keys[index] as usize];
            self.items[*slot as usize] = index as u32;
            *slot += 1;
        }
    }

    fn at(cell: f32, point: Vec3) -> (i32, i32) {
        (
            (point.x / cell).floor() as i32,
            (point.z / cell).floor() as i32,
        )
    }

    /// A cell's bucket: the two coordinates stirred together by multiplication
    /// and one shift, because masking keeps the low bits and multiplication only
    /// ever carries upwards.
    fn bucket(mask: u32, (x, z): (i32, i32)) -> u32 {
        let mixed = (x as u32)
            .wrapping_mul(0x9E37_79B1)
            .wrapping_add((z as u32).wrapping_mul(0x85EB_CA6B));
        (mixed ^ (mixed >> 15)) & mask
    }

    /// Everything in the nine cells around `point`, appended to `found` after
    /// emptying it. The caller passes the same buffer back in each time rather
    /// than allocating a fresh one per member of the crowd.
    fn near(&self, point: Vec3, found: &mut Vec<usize>) {
        found.clear();
        if self.items.is_empty() {
            return;
        }
        let (x, z) = Self::at(self.cell, point);
        // Nine cells, and any two of them may share a bucket. Reading one twice
        // would return its points twice, so they are collected as they are
        // visited and skipped if they come round again.
        let mut read = [u32::MAX; 9];
        let mut count = 0;
        for z in z - 1..=z + 1 {
            for x in x - 1..=x + 1 {
                let bucket = Self::bucket(self.mask, (x, z));
                if read[..count].contains(&bucket) {
                    continue;
                }
                read[count] = bucket;
                count += 1;
                let run = self.begins[bucket as usize] as usize
                    ..self.begins[bucket as usize + 1] as usize;
                found.extend(self.items[run].iter().map(|index| *index as usize));
            }
        }
    }
}

/// Connects the AnimationPlayer created inside a GLB scene to its enemy root.
///
/// The clip node comes along with the owner because [`sync_animation_visibility`]
/// *stops* a culled enemy's animation rather than pausing it, and something
/// stopped has to be started again by name when it comes back into view. An
/// enemy has exactly one clip, so one node is the whole of what that takes.
#[derive(Component)]
pub struct EnemyAnimationRoot {
    pub owner: Entity,
    pub clip: AnimationNodeIndex,
}

/// The enemies the AI step is allowed to move.
///
/// Anything still in the air on the arc a pipe threw it is flown by `pipe::fly`
/// and left out here: a behaviour that writes its own speed every tick would
/// eat the launch within a tick or two and drop it back on the pipe it came out
/// of. `Without<Player>` is the usual disjointness proof -- Bevy takes nothing
/// on trust from a `With` filter.
type WalkingEnemies<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Enemy,
        &'static mut Transform,
        &'static mut Visibility,
        // Mutable, because the cheap tier acquires its own target from the flow
        // field rather than being told about one by `alert`.
        &'static mut Aggro,
        &'static Quirk,
        &'static mut Wander,
        Option<&'static mut Crawler>,
        &'static Detail,
        // The way to whatever it is chasing, when the straight line will not
        // do. Written by [`navigate`], filled in by [`crate::path::plan`], and
        // empty on open ground -- which is most enemies most of the time. See
        // [`navigate`].
        &'static mut crate::path::Route,
    ),
    (With<Enemy>, Without<Player>, Without<crate::pipe::Launched>),
>;

/// How much a chasing enemy minds deep water on the way, against the console's
/// `path_toll`.
///
/// The same scale [`crate::goap::Goal::caution`] puts on a Mario's jobs, and
/// set between an order and an errand: a chase goes round the pond when there
/// is a way round and through it when there is not. Nothing here is zero, so a
/// brood takes the bridge when the bridge is there; nothing here is infinite,
/// so a moat is not a wall an enemy can be parked behind for ever.
const CHASE_CAUTION: f32 = 0.9;

/// The spot a chasing enemy is actually making for.
///
/// Its own place around the target rather than the target itself, so a brood
/// arrives *around* what it is after instead of inside it -- see
/// [`Quirk::stand_off`], and `aggro.room`, which is how wide the thing being
/// chased is.
///
/// One function rather than the line written out at each site, because three
/// places want this answer -- the walk step, [`navigate`], and the overlay that
/// draws what the walk step is doing -- and a debug view that draws a different
/// spot from the one the body is walking to is worse than no debug view.
fn chase_goal(enemy: &Enemy, aggro: &Aggro, quirk: &Quirk) -> Vec3 {
    aggro.at + quirk.stand_off(aggro.room + enemy.kind.body().0)
}

/// Everything [`navigate`] needs to say where one enemy is trying to get to.
///
/// `Crawler` is optional because it is what an ant has and a slime has not, and
/// the whole of the rule below is that the two want different answers.
type Chasers<'w, 's> = Query<
    'w,
    's,
    (
        &'static Enemy,
        &'static Aggro,
        &'static Detail,
        &'static Quirk,
        Option<&'static Crawler>,
        &'static mut crate::path::Route,
    ),
>;

/// Tells the pathfinder where every chasing enemy is trying to get to.
///
/// **The enemies' half of [`crate::path`], and it is a decider rather than a
/// search.** It writes a `want` and nothing else; [`crate::path::plan`] does
/// the work, for the enemies and the Marios together, out of one budget.
///
/// The reason it is worth having at all is that the two tiers were not equally
/// well served. A [`Detail::Crowd`] enemy already navigates globally -- it
/// walks down [`crate::flow::FlowField`]'s own gradient, which is a flood from
/// the player over the whole grid, so it comes round the moat and through the
/// gate without ever being told to. A [`Detail::Full`] one walked *straight at*
/// what it had noticed, with a stride and a half of local steering to get it
/// out of trouble. So the near tier -- the enemies actually on screen, the only
/// ones the player can watch -- was the half that got stuck on walls, filed
/// along fences, and stood pressing into the outside of the courtyard it had
/// last seen a Mario in. Better simulation, worse navigation.
///
/// The flood cannot fix that, and it is worth saying why rather than reaching
/// for it: it is flooded from *the player*, so it answers exactly one question
/// -- which way is he -- and an enemy fighting the squad is chasing a Mario
/// somewhere else entirely. A route is per journey and this crowd makes a lot
/// of journeys, which is what the grouping in [`crate::path::plan`] is for: a
/// brood coming out of one pipe at one Mario is one search between all of them,
/// bucketed on where they are and where they are going.
///
/// Only the near tier is offered one. The far crowd has the flood, which is
/// better than a route for the question it can answer and costs one array read;
/// putting a thousand of them into the search queue would spend the whole
/// budget on bodies the size of a thumbnail.
///
/// **What it costs, measured rather than argued.** Two hundred near-tier
/// enemies scattered over the castle and all chasing the player -- the worst
/// arrangement for the sharing, since scattered bodies are scattered journeys
/// -- run this and [`crate::path::plan`] together in 0.29 ms a tick on average
/// and 0.56 ms at worst, against a fixed step of 33 ms. Eight of the two
/// hundred come due on a given tick, because a route is trusted for
/// `path_refresh` seconds, and four of those are served, because `path_budget`
/// says four; the rest walk last tick's route and come round next tick. That
/// is the same shape as everything else in the crowd tier: the bill is a
/// console row, not a body count.
///
/// # Who is told the way is decided by what the body can walk on
///
/// **A [`Crawler`] is never routed, and that is navigation rather than an
/// omission.** [`crate::flow::FlowField`] is a floor: one ground height a cell,
/// and an edge it refuses when the step between two cells is taller than
/// [`crate::flow::CLIMB`]. Every question it can answer is a question about
/// getting *round* things. An ant does not want the answer to that question. It
/// is stuck to whatever it is touching -- see [`crawl_towards`], where walking
/// into a wall *is* climbing it -- so the wall in front of it is not an
/// obstacle between it and the Mario, it is the next bit of floor. Routed on
/// the grid, an ant that used to go over the courtyard wall walks the long way
/// round to the gate instead, and an ant part way up a wall is handed a corner
/// on the ground and comes back down for it. Both are worse than the straight
/// line, and the straight line is already right: **what is in the way is the
/// way.**
///
/// So the rule is not "ants are a special case", it is that a route answers a
/// question only some bodies are asking. A flier will land here as the third
/// case and for the same reason -- it goes over the top of what a walker goes
/// round -- and what it wants is height rather than a floor plan, which is
/// again not a thing this grid holds. Both belong in this one `if`.
pub fn navigate(tuning: Res<GameTuning>, mut enemies: Chasers) {
    for (enemy, aggro, detail, quirk, crawler, mut route) in &mut enemies {
        // Nothing to route to, too far off to be worth routing, or a body that
        // walks on the walls and is better off without one. See the note above.
        if *detail != Detail::Full || aggro.target.is_none() || crawler.is_some() {
            // Asked before it is told, so a field of a thousand ambling
            // enemies is not marked changed every tick for having nothing to
            // do. See [`crate::path::Route::resting`].
            if !route.resting() {
                route.clear();
            }
            continue;
        }
        // The same point the walk step will make for. Routing to anywhere else
        // would hand it a last corner it is not allowed to stand on and leave
        // [`spread`] arguing with the route about where it goes.
        let goal = chase_goal(enemy, aggro, quirk);
        route.want(
            Vec2::new(goal.x, goal.z),
            crate::flow::Tolls {
                wet: tuning.path_toll * CHASE_CAUTION,
                hug: tuning.path_clearance,
            },
        );
    }
}

pub fn update(
    fixed_time: Res<Time<Fixed>>,
    level: Res<LevelData>,
    field: Res<crate::flow::FlowField>,
    player: Query<(Entity, &Transform), With<Player>>,
    mut enemies: WalkingEnemies,
    tuning: Res<GameTuning>,
    mut fixed_tick: Local<u32>,
) {
    let Ok((luna, player)) = player.single() else {
        return;
    };
    let player = player.translation;
    let elapsed = fixed_time.elapsed_secs();
    *fixed_tick = fixed_tick.wrapping_add(1);
    let tick = *fixed_tick;
    enemies.par_iter_mut().for_each(
        |(
            entity,
            enemy,
            mut transform,
            mut visibility,
            mut aggro,
            quirk,
            mut wander,
            crawler,
            detail,
            mut route,
        )| {
            let distance_squared = player.distance_squared(transform.translation);
            // Hysteresis, and it is load-bearing rather than tidy. Crossing
            // this boundary now costs a whole glTF scene being spawned or
            // thrown away -- see [`shed_scenes`] -- so an enemy stood exactly
            // on it with a hard threshold would build and destroy forty
            // entities every frame it jittered. The band means a boundary can
            // only be crossed by walking a tenth of the draw distance past it.
            let near = tuning.enemy_draw * tuning.enemy_draw;
            let far = near * SWAP_HYSTERESIS * SWAP_HYSTERESIS;
            let wanted = if distance_squared > far {
                Visibility::Hidden
            } else if distance_squared < near {
                Visibility::Visible
            } else {
                *visibility
            };
            // Written only on a change, so the far crowd is not put through
            // visibility propagation every frame for standing still.
            if *visibility != wanted {
                *visibility = wanted;
            }
            let stride = if distance_squared > tuning.enemy_lod_far * tuning.enemy_lod_far {
                4
            } else if distance_squared > tuning.enemy_lod_near * tuning.enemy_lod_near {
                2
            } else {
                1
            };
            if !(tick + entity.index().index()).is_multiple_of(stride) {
                return;
            }
            // A reduced-rate step covers the skipped fixed ticks, so far actors
            // retain the same average movement and animation-independent AI time.
            let dt = crate::player::FIXED_DT * stride as f32;
            // Where it is going, and how fast. Something it has noticed is
            // chased at the chase speed and never given up on -- at its own
            // spot around it rather than at it, so a brood arrives around what
            // it is after instead of inside it. The rest of the time it ambles
            // around its own patch, and standing about between one spot and the
            // next is a goal of `None`.
            // The cheap tier: everything it needs is one array lookup, and it
            // asks the level nothing at all.
            if *detail == Detail::Crowd {
                let walked = crowd_step(
                    &field,
                    &mut transform,
                    &mut aggro,
                    &mut wander,
                    quirk,
                    luna,
                    &tuning,
                    elapsed,
                    dt,
                    enemy.kind.lift(),
                    enemy.kind.body().0,
                );
                stand_upright(crawler, walked, dt);
                return;
            }
            let (goal, speed) = match aggro.target {
                Some(_) => (
                    // The next corner of the way there, when the straight line
                    // would not have done and the pathfinder has been round to
                    // this one. Empty on open ground, which is most enemies
                    // most of the time -- and then this is the standoff point
                    // it always walked at: outside both bodies, not a metre
                    // from the target's middle. An ant is 2.5 m of radius on
                    // its own. See [`navigate`].
                    route
                        .leg(&field, transform.translation, tuning.path_spread)
                        .or_else(|| Some(chase_goal(enemy, &aggro, quirk))),
                    tuning.enemy_speed,
                ),
                None => (wander.goal(transform.translation, dt), WANDER_SPEED),
            };
            let speed = speed * quirk.pace();
            let up = crawler.as_ref().map_or(Vec3::Y, |crawler| crawler.up);
            // A wander across the line it is walking, so that a group heading
            // for one place does not converge on it along one bearing.
            //
            // **Faded out over the last stretch, and that is not a polish
            // detail.** `across` is taken from where it is standing *now*, so
            // the offset swings round with the approach: cross the goal and the
            // sideways push crosses with you. Far off that is the weave doing
            // its job. Arrived, it is a goal that circles the spot at the weave's
            // own width, and a thing chasing it walks a ring round its standoff
            // point at full speed for the whole fight rather than standing in
            // it. That is the shuffle a crowd is made of -- every enemy that has
            // reached you orbiting, and [`spread`] shoving each one through its
            // neighbours as it goes.
            //
            // Held to half of what is left to walk, so the goal always lies
            // ahead of the thing chasing it and the approach converges. Beyond
            // twice the weave's width that clamp never binds and this is the
            // weave exactly as it was.
            let goal = goal.map(|goal| {
                let there = goal - transform.translation;
                let across = there.cross(up).normalize_or_zero();
                // How far there still is to walk *along the surface*, which is
                // the plane the offset is added in.
                let room = (there - up * there.dot(up)).length() * 0.5;
                goal + across * quirk.weave(elapsed).clamp(-room, room)
            });
            // `walk` and `settle` answer in contact points -- where the feet go
            // -- and the transform is the model's origin, which for these
            // actors is not the same place. Converted here rather than inside
            // them, so both stay pure functions of the level that a test can
            // walk an enemy across without knowing what it looks like.
            let lift = Vec3::Y * enemy.kind.lift();
            let Some(mut crawler) = crawler else {
                // The plain walkers stay in the horizontal plane, are stopped by
                // anything too steep to walk, and follow the ground under them.
                let mut at = transform.translation - lift;
                if let Some(goal) = goal {
                    let there = goal - at;
                    let dir = tangent(there, Vec3::Y);
                    // Never further than the goal actually is. `tangent`
                    // answers a *direction*, so without the clamp a full step
                    // is taken however near the goal has got -- and something
                    // that has arrived spends every tick stepping past its spot,
                    // turning round, and stepping past it again. That is the
                    // shuffle a crowd holds you in: an enemy at its standoff
                    // distance is exactly the enemy that has arrived, so the
                    // whole ring round the player does it at once, and each one
                    // flips its facing a full half-turn every tick doing it.
                    //
                    // The amble has never had this problem because [`Wander`]
                    // stops asking once it is within [`WANDER_ARRIVE`]. A chase
                    // has no arrival to declare -- the goal moves with what it
                    // is chasing -- so what stands in for one is refusing to
                    // overshoot.
                    let step = Vec2::new(there.x, there.z).length().min(dt * speed);
                    if step > ARRIVED {
                        at = walk(&level, at, dir * step, enemy.kind.body().0);
                        transform.rotation = Quat::from_rotation_y(dir.x.atan2(dir.z));
                    }
                }
                transform.translation = settle(&level, at, dt) + lift;
                return;
            };
            // A crawler heads for the same goal, but only along the surface it
            // is on, and only as fast as it can turn. Standing about, it is
            // still asked to walk nowhere in the direction it already faces:
            // that re-seats it on ground that may have shifted under it.
            let (goal, speed) = match goal {
                Some(goal) => (goal, speed),
                None => (transform.translation + crawler.heading, 0.0),
            };
            transform.translation = crawl_towards(
                &level,
                transform.translation,
                &mut crawler,
                goal,
                speed,
                dt,
                enemy.kind.lift(),
            );
            transform.rotation =
                orientation(crawler.up, crawler.heading).unwrap_or(transform.rotation);
        },
    );
}

/// One tick of a crowd-tier enemy: the whole of what a distant enemy costs.
///
/// No ray casts, no floor queries, no neighbours -- one lookup into
/// [`crate::flow::FlowField`], which answered all of those questions once for
/// everybody -- walls and cliffs included, so a crowd enemy is stopped by a
/// fence and refused a cliff exactly as [`walk`] would stop and refuse it, just
/// out of a table rather than a ray cast. What it gives up is resolution: the
/// grid cannot hold anything thinner than itself, so a wall that does not
/// separate two cell centres is a wall this does not know about, and a body is
/// held out of one by the cell it is in rather than by its own width.
///
/// The two behaviours it picks between are the same two the near tier has, so
/// an enemy crossing the boundary carries on doing what it was doing rather
/// than visibly changing its mind.
#[allow(clippy::too_many_arguments)]
fn crowd_step(
    field: &crate::flow::FlowField,
    transform: &mut Transform,
    aggro: &mut Aggro,
    wander: &mut Wander,
    quirk: &Quirk,
    player: Entity,
    tuning: &GameTuning,
    elapsed: f32,
    dt: f32,
    lift: f32,
    radius: f32,
) -> Vec3 {
    // Which way it ended up walking, for [`stand_upright`] to keep a crawler's
    // own heading in step with. `Vec3::ZERO` when it stood still.
    let mut walked = Vec3::ZERO;
    let guide = field.at(transform.translation);
    // Noticing the player, without [`alert`]. The field already knows how far
    // away he is *along walkable ground*, which is a better question than the
    // straight-line one the near tier asks -- something on the far side of the
    // moat is not as close as it looks. Once noticed he is never given up on,
    // exactly as in the near tier.
    let earshot = tuning.enemy_sight * CROWD_SIGHT;
    if aggro.target.is_none() {
        let noticed = guide.steps.is_some_and(|steps| {
            // Either it can see him for itself, or word has reached it.
            steps as f32 * field.cell_size() < earshot || field.alarmed(steps)
        });
        if noticed {
            aggro.target = Some(player);
            // Set rather than left alone: this one may have been chasing a
            // Mario, or an ant, before it was demoted out of the near tier, and
            // a stale width is a standoff distance for the wrong creature.
            aggro.room = crate::player::PLAYER_RADIUS;
        }
    }
    let speed = quirk.pace()
        * if aggro.target.is_some() {
            tuning.enemy_speed
        } else {
            WANDER_SPEED
        };
    // Chasing follows the field; ambling heads for its own patch, which is
    // ground it was placed on and so ground it can stand on.
    let towards = if aggro.target.is_some() {
        // Across the route rather than along it, so a thousand enemies handed
        // the same direction spread into a crowd instead of a queue.
        let across = Vec2::new(-guide.towards.y, guide.towards.x);
        (guide.towards + across * quirk.weave(elapsed) * CROWD_WEAVE).normalize_or_zero()
    } else {
        let goal = wander.goal(transform.translation, dt);
        goal.map_or(Vec2::ZERO, |goal| {
            Vec2::new(
                goal.x - transform.translation.x,
                goal.z - transform.translation.z,
            )
            .normalize_or_zero()
        })
    };
    if towards != Vec2::ZERO {
        let step = towards * speed * dt;
        // The same three candidates [`walk`] tries, against the same idea of
        // what a wall is, read out of the survey instead of ray-cast: a slime
        // that meets a fence at an angle slides along it rather than standing
        // there pushing at it, and one offered the top of a wall as its next
        // step is refused it.
        //
        // **Asked a body's width past where the step lands, exactly as [`walk`]
        // asks it.** Without that this tier walks a *point* around the grid,
        // and a point stops with the wall running through the middle of it. It
        // is worse than it sounds, because the two lengths involved are the
        // wrong way round: the grid's cells are about 1.7 m and an ant's own
        // radius is 2.5, so a bug whose centre is on a perfectly legal cell has
        // most of itself inside the fence next door -- and `clear` waves
        // through every step that stays within one cell, which at a walking
        // pace is nearly all of them. That is the crowd walking through walls.
        // Reaching a radius ahead asks the grid about the cell the *body* is
        // about to occupy rather than the one its centre is about to stand on.
        //
        // Two questions rather than one, and they are asked at two distances:
        // where the feet land has to be ground worth standing on, and a body's
        // width past that only has to be free of walls. Asking for both at
        // arm's length holds the crowd a body back from every pond and lawn
        // edge on the map instead of from the fences.
        for candidate in [step, Vec2::new(step.x, 0.0), Vec2::new(0.0, step.y)] {
            if candidate == Vec2::ZERO {
                continue;
            }
            let reach = candidate + candidate.normalize_or_zero() * radius;
            let ahead = transform.translation + Vec3::new(candidate.x, 0.0, candidate.y);
            let face = transform.translation + Vec3::new(reach.x, 0.0, reach.y);
            if field.clear(transform.translation, ahead)
                && !field.walled(transform.translation, face)
            {
                transform.translation = ahead;
                break;
            }
        }
        transform.rotation = Quat::from_rotation_y(step.x.atan2(step.y));
        walked = Vec3::new(towards.x, 0.0, towards.y);
    }
    // Settled onto the cell's ground at a pace rather than snapped to it, for
    // the same reason [`settle`] does: a column's answer jumps at a cell
    // boundary, and taking it whole makes an enemy climbing a slope appear to
    // teleport up it a step at a time.
    let guide = field.at(transform.translation);
    if guide.walkable {
        let (rise, drop) = (CLIMB_SPEED * dt, FALL_SPEED * dt);
        // The field answers where the ground is; the model's origin belongs
        // `lift` above that, the same as in the near tier. Getting this wrong in
        // one tier and right in the other is an enemy that sinks into the floor
        // as you walk away from it.
        let wanted = guide.ground + lift;
        transform.translation.y += (wanted - transform.translation.y).clamp(-drop, rise);
    }
    walked
}

/// Puts a crawler the right way up, and is the only thing that ever rescues one
/// from under the floor.
///
/// An ant carries its own idea of which way is up, and walking onto a
/// ceiling turns that idea upside down -- which is the feature. The trouble is
/// that hanging under a ceiling and being buried under a floor are *the same
/// state*: a surface overhead, the body against its underside, `up` pointing
/// down. Nothing local can tell them apart, so nothing local can undo the
/// second, and [`crawl`]'s probes are perfectly happy to keep a bug there
/// forever -- it finds the floor's underside every tick and clings to it. On
/// screen that is an ant swimming about inside the lawn.
///
/// What puts one there is any move it did not make itself: the crowd tier
/// planting it on the ground at [`crowd_step`], a knockback, a shove from
/// [`spread`]. It was moved without being told which way was up, and it kept the
/// answer it had.
///
/// So the fix belongs at the transition rather than in the crawl, and the crowd
/// tier is a good place to put it: everything past the simulation budget is
/// walking the flow field on level ground, where up is up by definition. It also
/// makes the game self-healing -- a bug that has got itself stuck under the
/// world is stood back up the moment you walk far enough away for it to become
/// crowd, which is the opposite of how these usually go.
///
/// It also keeps the bug's own heading pointed the way the crowd tier has been
/// walking it, which is the other half of the same problem and the half that is
/// visible from further away.
///
/// [`crowd_step`] turns a body by writing a plain yaw onto its transform, which
/// is right for a walker and is all a walker has. A crawler's transform is built
/// out of [`Crawler::up`] and [`Crawler::heading`] instead, and the crowd tier
/// touched neither -- so the heading a bug carried out past the budget was the
/// heading it came back with, however far round the field it had walked
/// meanwhile. It showed up twice over: the tick it is promoted, [`orientation`]
/// rebuilds its transform from that stale heading and the bug spins on the spot;
/// and for the whole time it is out there, [`crate::impostor`] picks which
/// picture to draw it from out of the same transform, so a bug walking one way
/// is drawn from the bearing it was walking a minute ago.
///
/// Written only when it is wrong, so the far crowd is not marked changed every
/// tick for already being upright and already pointed the right way.
fn stand_upright(crawler: Option<Mut<Crawler>>, walked: Vec3, dt: f32) {
    let Some(mut crawler) = crawler else {
        return;
    };
    if crawler.up != Vec3::Y {
        // Rolled rather than snapped, at the same rate as any other change
        // of surface: a bug righting itself is a bug going round a corner.
        crawler.up = lean(crawler.up, Vec3::Y, crawler.heading, ROLL_RATE * dt);
    }
    // Turned at a bug's own pace rather than snapped, for the same reason
    // [`crawl_towards`] turns it at one: a heading that can change instantly is
    // a heading that ping-pongs. Standing still leaves it alone -- something
    // that has stopped is still facing the way it stopped facing.
    let wanted = tangent(walked, crawler.up);
    if wanted != Vec3::ZERO && wanted != crawler.heading {
        crawler.heading = steer(crawler.heading, wanted, crawler.up, TURN_RATE * dt);
    }
}

/// Takes one step along the ground, if there is ground to take it onto.
///
/// Two questions, and between them they are the whole of what a walking enemy
/// may do with a cliff:
///
///  * is there something too steep to walk in the way? Probed at [`STEP_UP`]
///    above its feet, so that a kerb it could step up passes underneath the
///    probe while a wall, a cliff face or a slope past
///    [`crate::level::GROUND_NORMAL_Y`] does not. Steepness is measured with the
///    same threshold the collision grid sorts walls by, because a walker's idea
///    of too steep and the level's had better be one idea.
///  * is there ground at the far end to put its feet on? Ground it can get up
///    onto in a step, which is what [`LevelData::ground_at`] answers.
///
/// A refused step is retried one axis at a time, so a slime that walks into a
/// wall at an angle slides along it instead of standing there pushing.
///
/// This replaced snapping to whatever the floor query answered, which is how a
/// slime at the bottom of a cliff used to arrive at the top of it: the query
/// was happy to hand back a surface two body-heights up, and being handed it was
/// the same thing as standing on it.
fn walk(level: &LevelData, position: Vec3, step: Vec3, radius: f32) -> Vec3 {
    let knee = Vec3::Y * STEP_UP;
    for candidate in [
        step,
        Vec3::new(step.x, 0.0, 0.0),
        Vec3::new(0.0, 0.0, step.z),
    ] {
        if candidate.length_squared() < 1e-12 {
            continue;
        }
        // A body's width past where it would end up, so it stops with its face
        // at the wall rather than inside it.
        let reach = candidate + candidate.normalize() * radius;
        let blocked = level
            .surface_hit(position + knee, position + knee + reach)
            .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y);
        if blocked {
            continue;
        }
        let there = position + candidate;
        if level.ground_at(there).is_some() {
            return there;
        }
    }
    position
}

/// Walks `position` towards whatever it should be standing on, at a pace rather
/// than by teleport.
///
/// Three cases, and which one applies is the whole of how a walker treats a
/// cliff:
///
///  * there is ground under it -- *ground*, meaning something it could walk up
///    onto rather than merely something solid. [`LevelData::ground_at`] refuses
///    both the too-steep and the too-high, so what comes back is a step and not
///    a climb, and a slime at the foot of a cliff is not offered the top of it.
///  * there is something under it but nothing it can stand on: a cliff face, the
///    slope it just walked off. It falls, and no further than the thing it fell
///    onto.
///  * there is nothing under it at all, which over this castle means it is
///    *inside* something rather than above it -- several of the level's own
///    placements are authored below the lawn they belong on. What is over its
///    head is the underside of that lawn, so it climbs out onto it.
///
/// The pace matters as much as the choice. The floor query answers a column and
/// a column's answer jumps; taking it whole is what made slimes appear to morph
/// between elevations rather than walk between them.
fn settle(level: &LevelData, position: Vec3, dt: f32) -> Vec3 {
    let (rise, drop) = (CLIMB_SPEED * dt, FALL_SPEED * dt);
    let towards = |height: f32, most: f32| {
        Vec3::new(
            position.x,
            position.y + (height - position.y).clamp(-drop, most),
            position.z,
        )
    };
    if let Some((ground, _)) = level.ground_at(position) {
        return towards(ground, rise);
    }
    match level.floor_height(position + Vec3::Y * STEP_UP) {
        // Nothing to stand on, but something to land on.
        Some(floor) => towards(floor, 0.0),
        // Nothing either: it is in the hill rather than on it.
        None => match level.ceiling_height(position, 0.0) {
            Some(top) => towards(top, rise),
            None => position,
        },
    }
}

/// Is there anything at all for an enemy to stand on here?
///
/// Asked of the crawlers' fallback, which needs to know whether the bug it is
/// dropping has landed rather than what height that landing is at.
fn ground_under(level: &LevelData, position: Vec3) -> bool {
    level.ground_at(position).is_some()
}

/// Walks a crawler one tick towards `goal`, turning it as it goes, and reports
/// where it ended up.
///
/// This is the whole of what an ant does, kept out of [`update`] so that a
/// bug can be walked across a level in a test without an app around it.
/// `position` and the result are the model's *origin*; `lift` is how far its
/// geometry hangs below that ([`Kind::lift`]). Everything in between works in
/// contact points -- the spot the body is actually resting on -- because that is
/// what the probes have to be cast from and what the surface arithmetic means.
/// Mixing the two is how the bug ended up underground in the first place.
fn crawl_towards(
    level: &LevelData,
    position: Vec3,
    crawler: &mut Crawler,
    goal: Vec3,
    speed: f32,
    dt: f32,
    lift: f32,
) -> Vec3 {
    let contact = position - crawler.up * lift;
    let toward = goal - contact;
    // **What to head for, which once it is on a wall is not where the goal
    // is.** The part of the way to the goal that lies in the surface is the
    // right answer on any floor and the wrong one on a wall, and getting that
    // wrong is the whole of "ants will not go over anything".
    //
    // Work the case through. A bug on the lawn walks into the courtyard wall
    // and climbs it, because [`crawl`] hands it the face it ran into and
    // [`lean`] carries its heading round the corner -- so far, so right. It is
    // now on a vertical face with the Mario it is after standing on the ground
    // on the other side. Project that direction into the face and what comes
    // out points *down*: the only part of "over there and below me" the wall
    // can hold is the below. So it turns round inside a second -- [`TURN_RATE`]
    // is three radians of it -- walks back down the metre it climbed, reaches
    // the bottom, walks into the wall, and climbs again. Watched, that is an
    // ant bobbing at the foot of a wall for the rest of the session, and every
    // part of it is the projection doing exactly what it says.
    //
    // The missing sentence is that **a wall between a bug and what it wants is
    // not something to walk along, it is something to go over, and the way
    // over is up.** Two conditions, and both are needed:
    //
    //   * the surface is *steep* -- [`CLIMB_STEEP`] -- so this cannot fire on a
    //     lawn, where the goal is a hair above or below the plane and the sign
    //     of that is noise;
    //   * and the goal is *behind* it, on the far side of the face it is stuck
    //     to. On a slope with the goal at the foot of it that is false, because
    //     a slope's normal leans out over its own foot, so a bug walks down a
    //     hill rather than climbing it.
    //
    // What it hands back then is straight up the face, and everything after is
    // machinery that already worked: over the lip, across the top, and down the
    // far side -- where the goal is in front of the surface again and this
    // stops firing on its own.
    let wanted = match climbing(crawler.up, toward) {
        true => tangent(Vec3::Y, crawler.up),
        false => tangent(toward, crawler.up),
    };
    crawler.heading = steer(crawler.heading, wanted, crawler.up, TURN_RATE * dt);
    match crawl(level, contact, crawler.up, crawler.heading * speed * dt) {
        Some((moved, found)) => {
            // Rolled towards the surface it found rather than snapped onto it,
            // so a corner is something it goes round over a few ticks.
            let up = lean(crawler.up, found, crawler.heading, ROLL_RATE * dt);
            // The heading is carried round the corner rather than recomputed:
            // a bug that walks over the lip of a table is still walking the
            // way it was, it is just that the way it was has been bent by the
            // edge it went round. By however much of the edge it has got round
            // so far, which is what the roll rate has just decided.
            crawler.heading = tangent(
                Quat::from_rotation_arc(crawler.up, up) * crawler.heading,
                up,
            );
            crawler.up = up;
            // Lifted along the *same* axis it was lowered along at the top --
            // the bug's own up, not the surface's. Mid-roll those differ, and
            // taking one for the other leaves a residue every tick that the
            // next tick then walks on: measured, a bug going round the castle's
            // corners drifted 1.2 m in a single step that way.
            //
            // The origin does still move as the body rolls, because a body
            // pivoting on its feet moves its middle. That is the thing itself,
            // not an error in it.
            moved + up * lift
        }
        // Nothing within reach in any direction, so there is no surface to walk
        // and nothing to be the right way up for: it is over open space, or
        // under the map at a spawn point placed below the hill it was meant to
        // be on. It carries on the way the plain walkers do -- straight there,
        // and dropped onto the first floor that appears under it -- which is
        // also how it gets out from under that hill.
        None => {
            let drifted = contact + crawler.heading * speed * dt;
            if ground_under(level, drifted) {
                crawler.up = lean(crawler.up, Vec3::Y, crawler.heading, ROLL_RATE * dt);
            }
            settle(level, drifted, dt) + crawler.up * lift
        }
    }
}

/// Whether a crawler is going *over* what is between it and where it wants to
/// be, rather than walking along it.
///
/// The two conditions [`crawl_towards`] sets out, as one question, so that the
/// walk step and the overlay that draws it cannot come to different answers.
/// See [`CLIMB_STEEP`] for the first and [`draw_crawlers`] for who else asks.
fn climbing(up: Vec3, toward: Vec3) -> bool {
    up.y.abs() < CLIMB_STEEP && toward.dot(up) < 0.0
}

/// Rolls `up` towards `wanted` by at most `most` radians, and is the whole of
/// why an ant going round a corner has frames in it.
///
/// `heading` is only the fallback axis, for the one case that has no other
/// answer: a surface exactly opposite the one it is on -- walking off a lip onto
/// the underside of the very slab it was standing on -- where every axis is a
/// shortest path and the cross product is zero. Tipping over its own nose is the
/// way an animal does that, so the axis is the one across its travel.
fn lean(up: Vec3, wanted: Vec3, heading: Vec3, most: f32) -> Vec3 {
    let angle = up.angle_between(wanted);
    if angle <= most || !angle.is_finite() {
        return wanted;
    }
    let across = up.cross(wanted);
    let axis = if across.length_squared() > 1e-9 {
        across.normalize()
    } else {
        match up.cross(heading).try_normalize() {
            Some(axis) => axis,
            // Facing exactly the way it is standing, which `orientation` also
            // has no answer for. Leave it be for a tick; the walk step will
            // have moved it by the next one.
            None => return up,
        }
    };
    Quat::from_axis_angle(axis, most) * up
}

/// Turns `heading` towards `target` within the surface `up` names, by at most
/// `most` radians.
fn steer(heading: Vec3, target: Vec3, up: Vec3, most: f32) -> Vec3 {
    let heading = tangent(heading, up);
    if heading == Vec3::ZERO {
        return target;
    }
    if target == Vec3::ZERO {
        return heading;
    }
    let angle = heading.angle_between(target);
    if angle <= most {
        return target;
    }
    // Which way round the shorter turn is. Dead astern it is neither way, and
    // the bug picks one rather than standing there unable to choose.
    let sign = if heading.cross(target).dot(up) < 0.0 {
        -1.0
    } else {
        1.0
    };
    Quat::from_axis_angle(up, sign * most) * heading
}

/// The part of `vector` that lies in the surface whose normal is `up`, as a
/// direction. Zero when `vector` has nothing to say about where to go along the
/// surface, which is the case a caller has to handle rather than normalise.
fn tangent(vector: Vec3, up: Vec3) -> Vec3 {
    (vector - up * vector.dot(up)).normalize_or_zero()
}

/// Stands a model on `up` and turns it to face `forward`, which is the thing
/// `from_rotation_y` cannot express once up has stopped being up.
///
/// `None` when the two are the same direction and there is no facing to build
/// out of them -- a bug walking exactly into the surface it is stuck to, which
/// the caller answers by leaving it turned the way it already was.
fn orientation(up: Vec3, forward: Vec3) -> Option<Quat> {
    let forward = tangent(forward, up);
    if forward == Vec3::ZERO {
        return None;
    }
    // Right-handed, and orthonormal because up and forward are perpendicular
    // unit vectors by construction: exactly what `from_mat3` requires.
    let right = up.cross(forward);
    Some(Quat::from_mat3(&Mat3::from_cols(right, up, forward)))
}

/// Walks a crawler one step of `step` along whatever it is stuck to, and
/// reports where it ended up and which way is up there.
///
/// Three questions, and the order they are asked in is the whole of it:
///
/// * is something in the way? An inside corner -- the foot of a wall, or the
///   top of one where it meets the ceiling. The surface it ran into becomes the
///   surface it is standing on, which is how a bug gets off the floor and,
///   eventually, onto the ceiling.
/// * is there anything under the step? The ordinary case, and every slope with
///   it, since the answer carries the new surface's normal.
/// * is there anything under the *lip* it just walked over? An outside corner --
///   the edge of a table -- where the far side of the edge becomes the floor and
///   the bug carries on down it upside down relative to where it started.
///
/// `None` when all three miss, which means open space rather than a surface.
fn crawl(level: &LevelData, position: Vec3, up: Vec3, step: Vec3) -> Option<(Vec3, Vec3)> {
    let distance = step.length();
    if distance < 1e-6 {
        // Still worth asking what it is standing on -- the ground under a bug
        // that has stopped is the ground it should be lying along.
        let start = position + up * PROBE_RISE;
        return level
            .surface_hit(start, position - up * PROBE_DROP)
            .map(|(hit, normal)| (hit + normal * CRAWL_SKIN, normal))
            .or(Some((position, up)));
    }
    let direction = step / distance;
    // Cast from just off the surface: a probe starting on the floor it is
    // standing on finds that floor and nothing else.
    let eye = position + up * PROBE_EYE;
    if let Some((hit, normal)) = level.surface_hit(eye, eye + direction * (distance + PROBE_REACH))
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    let ahead = position + step;
    if let Some((hit, normal)) = level.surface_hit(ahead + up * PROBE_RISE, ahead - up * PROBE_DROP)
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    // Under and past the edge, looking back the way it came: what it finds is
    // the far face of the lip it just walked off. Reaching back exactly as far
    // as the edge can be and no further, because whatever this finds the bug is
    // put on, and a longer reach is a longer hop round the corner.
    let under = ahead - up * PROBE_DROP;
    if let Some((hit, normal)) =
        level.surface_hit(under, under - direction * (distance + PROBE_REACH))
    {
        return Some((hit + normal * CRAWL_SKIN, normal));
    }
    None
}

/// Draws what a crawler is doing, on the same `path_debug` switch the routes
/// are on.
///
/// **It is here because the route overlay draws nothing for an ant, and cannot
/// be made to.** [`crate::path::draw`] shows a body's corners and a cross on
/// the spot it was told to be, and a crawler has neither: it is deliberately
/// never routed -- see [`navigate`] -- so at `path_debug 1` an ant is a blank
/// where a slime is a green line, and a blank looks exactly like a bug.
///
/// What an ant's navigation actually consists of is three things, so they are
/// the three things drawn:
///
///   * **which way is up for it**, a stalk out of its back along
///     [`Crawler::up`]. This is the whole of "it is on the wall": upright on the
///     lawn, flat out sideways half way up the courtyard, pointing at the floor
///     once it is on the ceiling. Watch it swing over two or three frames at
///     every corner, which is [`ROLL_RATE`] doing its job.
///   * **which way it is walking**, a spur along [`Crawler::heading`], which
///     lies in that same surface. On a wall this is the one to watch: it is
///     what gets carried round the corner rather than recomputed, and an ant
///     that will not climb is an ant whose heading keeps being turned back
///     downwards.
///   * **what it is after**, a line to [`chase_goal`] with a cross on it. The
///     line goes through the wall, and that is the point -- it is the straight
///     line the ant is not able to walk, and the gap between it and the heading
///     is the detour the climb is making.
///
/// **The colour is the rule.** Cyan while it is walking along the surface it is
/// on, magenta while [`climbing`] says the thing it wants is behind that
/// surface and the way there is up. Turning magenta at the foot of a wall and
/// back to cyan on the far side of it is the whole behaviour in two colour
/// changes; an ant that stays cyan against a wall is one this rule is not
/// firing for.
///
/// Immediate mode and per frame, exactly as [`crate::path::draw`] is, so it
/// costs nothing while `path_debug` is nought.
pub fn draw_crawlers(
    tuning: Res<GameTuning>,
    mut gizmos: Gizmos,
    crawlers: Query<(&Enemy, &Transform, &Crawler, &Aggro, &Quirk)>,
) {
    if tuning.path_debug.round() as u32 == 0 {
        return;
    }
    for (enemy, at, crawler, aggro, quirk) in &crawlers {
        let contact = at.translation - crawler.up * enemy.kind.lift();
        // Nothing to chase is a bug ambling, and the wander goal is not a fact
        // about surfaces. The stalk and the spur are still worth drawing: which
        // way is up for an idle ant is the thing this view is for.
        let goal = aggro.target.map(|_| chase_goal(enemy, aggro, quirk));
        let colour = match goal.is_some_and(|goal| climbing(crawler.up, goal - contact)) {
            true => Color::srgb(1.0, 0.3, 0.9),
            false => Color::srgb(0.25, 0.9, 1.0),
        };
        // Off the surface along its own up rather than along the world's, or
        // the marks on a bug under a ledge are drawn inside the ledge.
        gizmos.line(contact, contact + crawler.up * CRAWL_DRAW, colour);
        gizmos.line(contact, contact + crawler.heading * CRAWL_DRAW, colour);
        if let Some(goal) = goal {
            gizmos.line(contact + crawler.up * CRAWL_SKIN, goal, colour);
            gizmos.cross(goal, 0.7, colour);
        }
    }
}

/// How long the up stalk and the heading spur are drawn, in metres.
///
/// A metre and a half: long enough to read the angle across a courtyard,
/// short enough that a wall covered in ants is still a wall rather than a
/// hedgehog.
const CRAWL_DRAW: f32 = 1.5;

/// Resolves the player against every enemy once a tick: a swing defeats what
/// is in front of him, coming down on one stomps it, and touching one any
/// other way throws him back.
///
/// Ported from `Interactions.resolve` in `sm64py/objects.py`, including the
/// three things that make it a fight rather than a mutual accident.
#[allow(clippy::type_complexity)]
pub fn combat(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut threats: ResMut<Threats>,
    mut drops: ResMut<crate::nuclonium::Drops>,
    mut player: Query<(Entity, &Transform, &mut Controller, &mut Health), With<Player>>,
    mut enemies: Query<(Entity, &Enemy, &Transform, &Detail, Option<&mut Health>), Without<Player>>,
) {
    let Ok((luna, player_transform, mut controller, mut luna_health)) = player.single_mut() else {
        return;
    };
    // The cooldown gates the whole resolution rather than only the damage.
    // That is not a detail: a knocked-back player is thrown up and off the
    // enemy that hit him and comes down on its head, so without this every
    // enemy that touches somebody standing perfectly still stomps *itself*
    // within a couple of seconds. A warp pipe whose every slime destroys
    // itself before you turn round is a warp pipe that appears to spawn
    // nothing at all.
    if controller.invulnerable_left > 0.0 {
        controller.invulnerable_left = (controller.invulnerable_left - FIXED_DT).max(0.0);
        return;
    }
    let here = player_transform.translation;
    let facing = player_transform.rotation * Vec3::Z;
    for (entity, enemy, transform, detail, mut health) in &mut enemies {
        let offset = transform.translation - here;
        // The cheap rejection this loop needs, and it has to be a *distance*
        // rather than a tier. It was `detail == Crowd`, on the grounds that the
        // crowd tier is by definition further away than the nearest
        // `sim_budget` enemies and the player's reach is two metres -- which
        // held while a field was hundreds and stops holding the moment it is
        // thousands. Pack enough of them around the player and the two
        // hundred-and-first nearest is *standing on him*: the sword swings
        // through it, it cannot be stomped, and it cannot hurt him either. The
        // enemy the crowd is thickest around is the one that stops being in the
        // fight.
        //
        // A cheap tier may look worse; it must not behave differently. The tier
        // is still what the rejection is *tuned* against -- nothing outside the
        // budget is normally within arm's length -- so in the ordinary case this
        // rejects exactly what the old test did, for the price of a squared
        // length that the loop was about to take anyway.
        if *detail == Detail::Crowd && offset.length_squared() > COMBAT_REACH * COMBAT_REACH {
            continue;
        }
        let horizontal = Vec3::new(offset.x, 0.0, offset.z);
        let distance_squared = horizontal.length_squared();
        let bearing = horizontal.normalize_or_zero();
        let (radius, height) = enemy.kind.body();
        // From its surface, exactly as the touch below already is. As a
        // distance to its centre the sword could not reach an ant at all:
        // `spread` holds one 3.27 m from the player and the swing was 2.2.
        let swing = radius + ATTACK_REACH;
        if controller.attack_left > 0.0
            && distance_squared < swing * swing
            && facing.dot(bearing) > -0.15
        {
            // A swing that does not finish it leaves it standing and angry: the
            // threat is filed either way, because what a crowd reacts to is
            // being hit rather than one of them dying.
            let felled = health::strike(health.as_deref_mut(), health::PLAYER_DAMAGE);
            if felled {
                commands.entity(entity).despawn();
                drops.maybe(transform.translation);
                sounds.push_at(Sfx::Defeat, transform.translation);
            } else {
                sounds.push_at(Sfx::Hurt, transform.translation);
            }
            // Everything near enough to have seen that gets angrier at him for
            // it, which is the other half of why a squad can pull a fight off
            // him: he is on the same ledger they are.
            threats.kill(luna, transform.translation);
            continue;
        }
        let reach = radius + PLAYER_REACH;
        if distance_squared > reach * reach {
            continue;
        }
        // Vertical overlap: his feet below its head, his head above its feet.
        // Without this he is "touching" it from a storey up.
        //
        // Which end of a crawler is its head depends on what it is stuck to --
        // one hanging from a ceiling has its head *below* its feet -- so the
        // band it occupies is measured from the direction its own model is
        // stood on rather than assumed to run upwards. For everything that does
        // stand upright that reads as it always did.
        let head = transform.translation + (transform.rotation * Vec3::Y) * height;
        let bottom = transform.translation.y.min(head.y);
        let top = transform.translation.y.max(head.y);
        if here.y > top || here.y + PLAYER_HEIGHT < bottom {
            continue;
        }
        if controller.velocity.y < 0.0 && here.y > bottom + (top - bottom) * STOMP_MARGIN {
            // Coming down on one costs it the same as swinging at it. The
            // bounce is unconditional, and that is deliberate: a stomp that
            // failed to kill and did not bounce would drop him onto a live
            // enemy that immediately hurts him, which reads as the stomp
            // having done nothing at all.
            if health::strike(health.as_deref_mut(), health::PLAYER_DAMAGE) {
                commands.entity(entity).despawn();
                drops.maybe(transform.translation);
                sounds.push_at(Sfx::Defeat, transform.translation);
            } else {
                sounds.push_at(Sfx::Hurt, transform.translation);
            }
            threats.kill(luna, transform.translation);
            controller.velocity.y = BOUNCE_VELOCITY;
            controller.grounded = false;
            continue;
        }
        controller.velocity = -bearing * KNOCKBACK_SPEED + Vec3::Y * KNOCKBACK_RISE;
        controller.grounded = false;
        controller.invulnerable_left = INVULNERABLE_SECONDS;
        // What it costs him depends on what touched him, which is the only
        // thing that makes one kind of enemy more dangerous than another.
        luna_health.hurt(enemy.kind.damage());
        sounds.push(Sfx::Hurt);
        // One hit a tick: walking into a cluster of them costs one heart, not
        // one per enemy in the cluster.
        return;
    }
}

/// How long a Mario's punch takes from the moment he starts it, and how far
/// past its target's body it lands. The reach is the player's sword's less the
/// difference between a sword and a fist.
///
/// It has to be at least as long as [`crate::squad::STRIKE_RANGE`] plus a
/// Mario's own radius, or a Mario walks to where it is allowed to stand and
/// then cannot reach what it walked to -- a squad that surrounds a slime and
/// then swings at the air forever.
const MARIO_SWING: f32 = 0.45;
const MARIO_REACH: f32 = 1.3;

/// What a Mario's fist can land on: the crowd, and the buildings on the other
/// side.
///
/// Two aliases rather than two inline query types, because between them they
/// carry four filters and every one is load-bearing. The `Without`s against each
/// other are what let both hold `Health` in one system -- Bevy proves two
/// mutable accesses disjoint from the filters alone, and an enemy is never a
/// building. `Without<Ally>` is the one about sides, and it is
/// [`crate::weapon::Targets`]'s: the Marios are hostile to the same things the
/// player is, and one must never be able to punch another.
type Quarry<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Enemy,
        Option<&'static mut Health>,
    ),
    (Without<Ally>, Without<crate::structure::Structure>),
>;

/// The nests, as a Mario sees them. See [`Quarry`] for what the filters buy.
type Nests<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static crate::structure::Structure,
        &'static mut Health,
    ),
    (Without<Ally>, Without<Enemy>),
>;

/// The Marios' half of the fight: a Mario stood over what it has noticed hits
/// it, and what it hits dies.
///
/// The blow lands at the *end* of the swing rather than the moment it starts,
/// so what kills a slime is the punch connecting rather than the decision to
/// punch -- one that wanders out of reach mid-swing is missed, which is the
/// only thing that makes standing next to one of these dangerous at all.
///
/// Runs after the allies have been walked, because the swing overrides what
/// they were doing: a Mario in the middle of a punch is punching, whatever the
/// walk step made of it.
pub fn ally_combat(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut threats: ResMut<Threats>,
    mut drops: ResMut<crate::nuclonium::Drops>,
    mut allies: Query<(Entity, &mut Ally, &Transform, &Aggro)>,
    mut enemies: Quarry,
    // The nests. Folded into this system rather than run beside it as a second
    // one, and that is not tidiness: a Mario has *one* swing timer and now two
    // kinds of thing it might be swinging at, and two systems counting the same
    // timer down is a Mario that throws every punch twice.
    mut buildings: Nests,
) {
    for (mario, mut ally, transform, aggro) in &mut allies {
        // What it is standing over, as one answer whichever kind of thing it
        // turned out to be: where it is, how wide it is, and whether hitting it
        // goes through a building's recovery window.
        let quarry = aggro.target.and_then(|target| {
            if let Ok((at, enemy, _)) = enemies.get(target) {
                return Some((target, at.translation, enemy.kind.body().0, false));
            }
            buildings
                .get(target)
                .ok()
                .map(|(at, structure, _)| (target, at.translation, structure.radius, true))
        });
        let in_reach = quarry.is_some_and(|(_, at, radius, _)| {
            let apart = at - transform.translation;
            // Past its body, so a punch reaches whatever the squad is allowed
            // to stand next to. See [`MARIO_REACH`].
            Vec3::new(apart.x, 0.0, apart.z).length() < radius + MARIO_REACH
        });
        if ally.swing_left > 0.0 {
            ally.swing_left = (ally.swing_left - FIXED_DT).max(0.0);
            ally.state.motion = Motion::Attack;
            ally.state.still_for = 0.0;
            if ally.swing_left == 0.0 && in_reach {
                let (target, at, _, is_building) = quarry.expect("in reach of nothing");
                // A Mario's fist is worth half what the player's sword is, so
                // an ant takes two of them and is hitting back in between. That
                // gap is what a squad is *for*: alone a Mario trades with an
                // ant, and four of them do not.
                //
                // Against a warp pipe the same fist is worth the same five
                // points, spent against a pool an order of magnitude bigger:
                // clearing a nest is what a squad is sent to do rather than
                // something that happens on the way past.
                let felled = if is_building {
                    let Ok((_, _, mut health)) = buildings.get_mut(target) else {
                        continue;
                    };
                    // Straight against the pool. A punch has already been paced
                    // by the Mario's own swing timer, so putting it through the
                    // building's [`crate::structure::RECOVERY`] as well would be
                    // pacing it twice -- and would mean a squad of eight took a
                    // nest down no faster than one of them. See that module's
                    // preamble for the split.
                    health.hurt(health::MARIO_DAMAGE)
                } else {
                    let health = enemies
                        .get_mut(target)
                        .ok()
                        .and_then(|(_, _, health)| health);
                    let felled = health::strike(health.map(Mut::into_inner), health::MARIO_DAMAGE);
                    if felled {
                        // One kill in twenty leaves something behind, and a
                        // Mario's kills are most of a field's. See
                        // [`crate::nuclonium::Drops::maybe`].
                        drops.maybe(at);
                    }
                    felled
                };
                if felled {
                    commands.entity(target).despawn();
                    // The squad fights spread out across the field, so where one
                    // of them landed a punch is worth hearing.
                    sounds.push_at(Sfx::Defeat, at);
                } else {
                    sounds.push_at(Sfx::Hurt, at);
                }
                // And worth *noticing*: this is the act the whole attention
                // model is built around. A Mario hitting things in the middle
                // of a crowd is how that crowd comes to be fighting Marios
                // rather than the player, over the next second or two.
                threats.kill(mario, at);
            }
            continue;
        }
        if in_reach {
            ally.swing_left = MARIO_SWING;
            // Punches alternate, so a Mario stood over a crowd is throwing a
            // combination rather than the same punch on a loop.
            ally.state.combo ^= 1;
            ally.state.motion = Motion::Attack;
            ally.state.still_for = 0.0;
        }
    }
}

/// The other half of a squad fight: an enemy stood over a Mario wears it down.
///
/// Before hit points a Mario was invincible and an enemy that reached one
/// simply stood in it, which made sending the squad at anything a decision with
/// no cost -- twenty Marios cleared a nest as reliably as one did, slower. What
/// makes an order worth thinking about is that the thing being ordered can be
/// lost.
///
/// **An enemy only hurts what it is actually fighting.** The loop is over
/// enemies rather than over allies, and it does nothing at all for one whose
/// [`Aggro::target`] is not an ally -- which is a single archetype check for
/// every enemy chasing the player, and no distance work whatever for the
/// thousands chasing nothing. Resolving this the other way round, as a sweep of
/// the field around each Mario, is the same answer for a great deal more work,
/// and it would also hurt a Mario that an ambling slime happened to bump into
/// on its way somewhere else.
///
/// The cooldown sits on the *victim*, exactly as the player's does, so what it
/// limits is how fast a Mario can be worn down rather than how fast any one
/// enemy swings. Standing in the middle of six ants is dangerous because they
/// take turns, not because they all land at once.
pub fn maul(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut threats: ResMut<Threats>,
    mut allies: Query<(&mut Ally, &Transform, &mut Health), Without<Enemy>>,
    enemies: Query<(Entity, &Enemy, &Transform, &Aggro), Without<Ally>>,
) {
    // Every Mario's window shortens once a tick whether or not anything is near
    // it, which is what makes the window a window rather than a thing that only
    // expires when it is next tested.
    for (mut ally, _, _) in &mut allies {
        ally.hurt_left = (ally.hurt_left - FIXED_DT).max(0.0);
    }
    for (enemy, body, transform, aggro) in &enemies {
        let Some(target) = aggro.target else {
            continue;
        };
        let Ok((mut ally, at, mut health)) = allies.get_mut(target) else {
            continue;
        };
        if ally.hurt_left > 0.0 {
            continue;
        }
        // From the enemy's surface to the Mario's, in the horizontal plane, the
        // same way every other reach in this module is measured. Reusing
        // `MARIO_REACH` would be wrong in the one case it matters: the Mario is
        // the thing being reached *for* here, so what has to cover the gap is
        // the enemy's own body plus a fist's worth.
        let apart = at.translation - transform.translation;
        let reach = body.kind.body().0 + crate::player::PLAYER_RADIUS + TOUCH_MARGIN;
        if Vec3::new(apart.x, 0.0, apart.z).length_squared() > reach * reach {
            continue;
        }
        ally.hurt_left = INVULNERABLE_SECONDS;
        if health.hurt(body.kind.damage()) {
            // `despawn` takes the actor scene hanging under it with it, which
            // is the same thing `squad::maintain_population` relies on when the
            // console thins the crowd.
            commands.entity(target).despawn();
            sounds.push_at(Sfx::Defeat, at.translation);
            // A Mario going down is an act like any other, and the enemy that
            // did it is what the rest of the squad comes for.
            threats.kill(enemy, at.translation);
        } else {
            sounds.push_at(Sfx::Hurt, at.translation);
        }
    }
}

/// Hidden skinned actors do not need their bones evaluated.
///
/// **Stopped rather than paused, and the difference is the whole saving.**
/// Pausing was what this did, and it saved nothing at all: in
/// `bevy_animation`, `advance_animations` checks `paused` only to skip the
/// clock, and `animate_targets` checks it only to skip firing animation
/// events. A paused player still has its curves sampled and still writes a
/// pose onto every joint it owns, every rendered frame -- for a field of two
/// thousand that is tens of thousands of skeleton entities being posed to
/// exactly the value they already held.
///
/// Worse than the wasted sampling is what the writes cost downstream.
/// `bevy_transform` skips whole subtrees whose transforms have not changed,
/// and a pose written every frame keeps every joint permanently dirty, so
/// propagation walks the entire crowd as well.
///
/// Stopping empties the player's active animations, so the clip lookup finds
/// nothing and returns immediately, no joint is written, and the dirty-tree
/// skip finally engages. What it costs is that resuming has to start the clip
/// again rather than unpause it -- which is what `restart` below does, from
/// the graph node the player was given when its scene arrived.
pub fn sync_animation_visibility(
    console: Res<crate::console::ConsoleState>,
    roots: Query<(&Visibility, &Quirk), With<Enemy>>,
    mut players: Query<(&EnemyAnimationRoot, &mut AnimationPlayer)>,
) {
    for (root, mut player) in &mut players {
        let Ok((visibility, quirk)) = roots.get(root.owner) else {
            continue;
        };
        // Three states, and which one an enemy wants is worth naming because
        // the console's case and the culled case are deliberately *not* the
        // same. Culled means stopped, because nobody is looking and the whole
        // point is to stop paying for the pose. The console open means merely
        // paused, because you are looking straight at the crowd and stopping it
        // would restart every clip from its first frame the moment you shut the
        // console -- turning a field of individuals into a marching band.
        let hidden = *visibility == Visibility::Hidden;
        // Read through the shared reference: merely *asking* what a player is
        // doing must not mark it changed. Every branch below is guarded on
        // this, because taking `&mut` unconditionally would flag the component
        // on every enemy on every frame, which is a change-detection storm in
        // place of the one being removed.
        let started = player.playing_animations().next().is_some();
        let paused = player.all_paused();
        match (hidden, console.open, started, paused) {
            // Out of view and still costing something: stop it.
            (true, _, true, _) => {
                player.stop_all();
            }
            // Nothing to do -- already in the state it wants.
            (true, _, false, _) => {}
            (false, open, true, held) if open == held => {}
            // In view, and the console has just opened or shut over it.
            (false, true, true, _) => {
                player.pause_all();
            }
            (false, false, true, _) => {
                player.resume_all();
            }
            // Back into view after being culled. An enemy's whole animation is
            // the single looping clip its kind loads, and `EnemyAnimationRoot`
            // carries the node it sits at in the graph its scene was given.
            //
            // Seeded from the enemy's own quirk rather than started at zero, so
            // a crowd that walks back into the draw distance together does not
            // arrive in lockstep -- the same reason nothing else about these is
            // shared either.
            (false, _, false, _) => {
                let phase = quirk.animation_phase();
                player.play(root.clip).repeat().seek_to(phase);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The glTF JSON chunk of an asset, so a claim about a model can be checked
    /// without a renderer. Same trick as `billboard.rs`, for the same reason.
    fn gltf(path: &str) -> serde_json::Value {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let bytes = std::fs::read(root.join(path)).expect("missing glb");
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[20..20 + length]).expect("bad glb json")
    }

    /// [`Kind::clip`] is an index into a file, and an index into a file is a
    /// thing a re-export can silently move.
    ///
    /// This is the whole reason that method has a doc comment naming the clip:
    /// the name is the intent and the number is only how glTF asset labels
    /// happen to address it. Blender writes its actions out in alphabetical
    /// order, so inserting one animation called `Attack` renumbers everything
    /// after it -- and the failure that causes is a crowd of slimes playing
    /// `Squish_Start` on a loop while sliding across the ground, which is a
    /// thing somebody has to notice by eye. Here it is a failing test naming
    /// both clips.
    #[test]
    fn the_clip_index_is_still_the_walk() {
        for (kind, wanted) in [(Kind::Slime, "Scoot_Move"), (Kind::Ant, "walk")] {
            let doc = gltf(kind.model());
            let clips = doc["animations"].as_array().expect("no animations");
            let found = clips
                .get(kind.clip())
                .and_then(|clip| clip["name"].as_str())
                .unwrap_or("<past the end of the file>");
            assert_eq!(
                found,
                wanted,
                "{kind:?} walks on animation {} of {}, which is now {found:?} \
                 rather than {wanted:?}",
                kind.clip(),
                kind.model()
            );
        }
    }

    /// An actor that exports with no material at all is a white plastic actor.
    ///
    /// glTF's defaults are `baseColorFactor` white and `metallicFactor` 1.0, so
    /// a material written with no `pbrMetallicRoughness` at all is not a subtle
    /// wrong -- it is the model rendered in white plastic, and it is what
    /// Blender's exporter silently produces when it cannot find a surface to
    /// follow. See `aim_material_outputs_at_everything` in
    /// `tools/blend_to_glb.py` for the one that caused this: a Material Output
    /// node targeted at EEVEE, which the exporter skips without a word.
    ///
    /// Cheap to guard and expensive to notice by eye, because the near tier
    /// goes white while the far tier keeps whatever the sheets were baked from
    /// -- so the crowd looks right until you walk up to one.
    #[test]
    fn every_actor_exports_a_material_it_can_be_lit_by() {
        for kind in [Kind::Slime, Kind::Ant] {
            let doc = gltf(kind.model());
            let materials = doc["materials"].as_array().expect("no materials");
            for material in materials {
                let name = material["name"].as_str().unwrap_or("<unnamed>");
                let pbr = &material["pbrMetallicRoughness"];
                assert!(
                    pbr.get("baseColorFactor").is_some() || pbr.get("baseColorTexture").is_some(),
                    "{kind:?}'s {name:?} carries no base colour, so glTF's own \
                     default applies and it draws as white plastic"
                );
            }
        }
    }

    /// How far a model's geometry reaches below its own origin.
    ///
    /// The same measurement [`Kind::lift`] is, and deliberately so now: the
    /// placement holds a model up by what the model says it hangs, so the two
    /// cannot disagree. What that leaves this useful for is the thing it is
    /// actually used for -- checking that the *placement* keeps a model's bottom
    /// out of the ground while it walks, which the lift is only one term of.
    /// `tools/measure_actor_hang.py` is the independent instrument, and it reads
    /// the posed mesh rather than the bind pose.
    fn hang_in_model(kind: Kind) -> f32 {
        measure(kind.model())
            .expect("a shipped actor could not be measured")
            .lift
    }

    /// That the game can measure both actors, and that what it measures is a
    /// model rather than a units mix-up.
    ///
    /// There is nothing here to compare [`Kind::body`] against: it *is* the
    /// measurement, which is the point -- an actor is drawn at the size it was
    /// authored at and spaced, shadowed and fought at the size it is drawn.
    /// **How big an actor should be is a decision that belongs in Blender**, so
    /// this deliberately does not have an opinion about it; a big enemy is a
    /// design choice and not a bug.
    ///
    /// What it does check is that the measurement happened at all. A model
    /// missing from the install leaves [`sizes`] on [`UNMEASURED`], and a
    /// silently plausible fallback is the failure this codebase has already had
    /// twice -- the impostor sheets missing from the Windows package drew no
    /// sprites at all and said so only on a stderr nobody was attached to.
    ///
    /// The band around that is wide enough to be about units rather than taste:
    /// a model exported at a hundred units to the metre, the way every decomp
    /// actor was, or at Blender's default two-metre cube when it was meant to be
    /// a beetle. Anything a person could plausibly want on this map passes.
    /// What catches a *stale* size -- an actor re-exported bigger without its
    /// sheet being re-baked -- is
    /// `impostor::tests::the_sheets_agree_with_the_models_they_were_baked_from`,
    /// which compares the model against something the renderer drew.
    #[test]
    fn an_actor_is_the_size_it_was_authored_at() {
        for kind in KINDS {
            let size = measure(kind.model()).unwrap_or_else(|| {
                panic!(
                    "{kind:?}'s model could not be measured, so the game would \
                        silently use {UNMEASURED:?}"
                )
            });
            assert!(
                (0.05..20.0).contains(&size.radius) && (0.05..20.0).contains(&size.height),
                "{kind:?} is authored {:.3} m across and {:.3} m tall, which is a \
                 units problem rather than a size. Set the size in Blender -- \
                 tools/resize_actor.py measures and changes it.",
                size.radius * 2.0,
                size.height,
            );
            // Feet on the origin: the pipeline's rule, and the thing that lets
            // every placement in the game seat a translation on the ground
            // without a correction. `Kind::lift` still carries one for an actor
            // that arrives without it, so this is a nudge rather than a
            // requirement -- but an actor whose origin is not its feet gets its
            // shadow, its stomp height and its sprite's footing from a guess.
            assert!(
                size.lift < 0.05,
                "{kind:?}'s art hangs {:.3} m below its own origin. It works -- \
                 `Kind::lift` holds it up -- but the model wants its origin moved \
                 to its feet in Blender.",
                size.lift,
            );
        }
    }

    /// A room with a floor, one wall across the far end of it and a ceiling:
    /// the three surfaces an ant is supposed to treat as the same thing.
    ///
    /// The wall is at `x = 4` and the ceiling at `y = 4`, both spanning the
    /// floor's footprint, so a bug that keeps walking in `+x` meets each of
    /// them in turn.
    fn room() -> LevelData {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let base = vertices.len() as u32;
            vertices.extend([a, b, c, d]);
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        };
        quad(
            Vec3::new(-4., 0., -4.),
            Vec3::new(4., 0., -4.),
            Vec3::new(4., 0., 4.),
            Vec3::new(-4., 0., 4.),
        );
        quad(
            Vec3::new(4., 0., -4.),
            Vec3::new(4., 4., -4.),
            Vec3::new(4., 4., 4.),
            Vec3::new(4., 0., 4.),
        );
        quad(
            Vec3::new(-4., 4., -4.),
            Vec3::new(4., 4., -4.),
            Vec3::new(4., 4., 4.),
            Vec3::new(-4., 4., 4.),
        );
        LevelData::new(vertices, triangles, Vec::new())
    }

    /// Walks a bug towards `goal` the way [`update`] does, and reports every
    /// place it stood on the way and which way was up there.
    fn trail(
        level: &LevelData,
        start: (Vec3, Vec3),
        goal: Vec3,
        steps: usize,
    ) -> Vec<(Vec3, Vec3)> {
        let (mut position, up) = start;
        let mut crawler = Crawler {
            up,
            heading: tangent(goal - position, up),
        };
        (0..steps)
            .map(|_| {
                // No lift: this walks the abstract [`room`] and asserts where
                // the contact point lands, which is what `crawl` answers in.
                position = crawl_towards(level, position, &mut crawler, goal, 3.0, FIXED_DT, 0.0);
                (position, crawler.up)
            })
            .collect()
    }

    /// The whole point of the thing: an ant chasing something it cannot
    /// reach along the floor climbs the wall in its way, carries on over the
    /// top of it onto the ceiling, and walks that upside down.
    #[test]
    fn a_crawler_walks_up_a_wall_and_across_the_ceiling() {
        let level = room();
        // Beyond the wall, so that the way to it is through the wall rather
        // than across the floor.
        let climb = trail(
            &level,
            (Vec3::new(-3., 0., 0.), Vec3::Y),
            Vec3::new(10., 4., 0.),
            200,
        );
        let climbed = climb
            .iter()
            .find(|(position, up)| up.x < -0.99 && position.y > 0.5)
            .unwrap_or_else(|| panic!("it never climbed the wall: {:?}", climb.last()));
        assert!(
            (climbed.0.x - 4.).abs() < 0.1,
            "climbing thin air: {climbed:?}"
        );
        let hanging = climb
            .iter()
            .find(|(_, up)| up.y < -0.99)
            .unwrap_or_else(|| panic!("it never made it onto the ceiling: {:?}", climb.last()));
        assert!(
            (hanging.0.y - 4.).abs() < 0.1,
            "hanging off nothing: {hanging:?}"
        );
        // And once there it walks the ceiling like any other floor, which the
        // corner it climbed in at would hide.
        let across = trail(&level, *hanging, Vec3::new(-10., 4., 0.), 40);
        let (position, up) = *across.last().unwrap();
        assert!(
            up.y < -0.99 && (position.y - 4.).abs() < 0.1 && position.x < hanging.0.x - 1.0,
            "it did not cross the ceiling: at {position:?} with up {up:?}"
        );
    }

    /// A bug meeting a new surface rolls onto it over several ticks.
    ///
    /// Asked for directly: "they should not snap so immediately to the angle of
    /// the surface they are walking on". Before [`ROLL_RATE`] the normal the
    /// probe returned became the bug's `up` the same tick, so the floor-to-wall
    /// corner was a ninety-degree flip with no frames in it.
    #[test]
    fn a_crawler_rolls_onto_a_new_surface_rather_than_snapping() {
        let level = room();
        let most = ROLL_RATE * FIXED_DT;
        let mut position = Vec3::new(-3., 0., 0.);
        let mut crawler = Crawler::default();
        let (mut steepest, mut rolling) = (0.0_f32, 0);
        for _ in 0..200 {
            let was = crawler.up;
            position = crawl_towards(
                &level,
                position,
                &mut crawler,
                Vec3::new(10., 4., 0.),
                3.0,
                FIXED_DT,
                0.0,
            );
            let turned = was.angle_between(crawler.up);
            steepest = steepest.max(turned);
            if turned > 1e-4 {
                rolling += 1;
            }
        }
        assert!(
            steepest <= most + 1e-3,
            "up swung {:.1} degrees in one tick, past the {:.1} it is allowed",
            steepest.to_degrees(),
            most.to_degrees()
        );
        // Floor to wall is a right angle and wall to ceiling another, so the
        // climb cannot be done in fewer than six ticks of turning at this rate.
        assert!(
            rolling >= 6,
            "the whole climb was done in {rolling} ticks of turning"
        );
        // And it still got there, which is the point of a limit rather than a ban.
        assert!(
            crawler.up.y < -0.99,
            "it never reached the ceiling: up {:?}",
            crawler.up
        );
    }

    /// A crawler walking the castle keeps its body out of the floor.
    ///
    /// This is the report, and it took three passes to find because every guess
    /// was about *behaviour* -- stale surface normals, walls, ceilings, the tier
    /// boundary -- when the cause was arithmetic and constant. The scuttlebug
    /// this was first found on had a rig root up inside its body, so the model
    /// hung 31 cm below its own transform origin, and every placement in the
    /// game seated that origin on the ground. The bug was buried to its belly
    /// on flat stone with nothing steep within twenty metres.
    ///
    /// What finally found it was looking at one: a screenshot of a single bug
    /// on the castle courtyard, where the floor cut a flat line across the
    /// bottom of its shell. The ant that replaced it is rigged the same way and
    /// hangs 22 cm, so the test outlived the actor it was written about.
    ///
    /// The test drives [`update`] rather than [`crawl_towards`] on purpose. A
    /// test that passes a lift in and then subtracts the same lift back out
    /// proves nothing -- it passes just as happily with the lift set to zero,
    /// which is exactly what the first version of this did. What has to be
    /// checked is that the *game's own placement* seats the model clear of the
    /// ground, so the number comes from [`Kind::lift`] here and from whatever
    /// `update` chooses to do there. Between this and
    /// `impostor::tests::the_lift_matches_what_the_baked_sheets_show` -- which
    /// pins `Kind::lift` to the pixels of the baked art -- neither half can
    /// drift without a failure.
    ///
    /// Both tiers are in the field at once, because a model correctly seated by
    /// one tier and buried by the other is an enemy that sinks into the ground
    /// as you walk away from it.
    #[test]
    fn a_crawler_keeps_its_body_out_of_the_floor() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let (level, _) = crate::level::load();
        let mut world = World::new();
        let spots = crowd_spots(64, &level);
        world.insert_resource(GameTuning {
            // Half the field near, half crowd.
            sim_budget: 32.0,
            ..GameTuning::default()
        });
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(Time::<Fixed>::default());
        world.insert_resource(Time::<()>::default());
        world.spawn((Player, Transform::from_xyz(-13.28, 3.0, 46.64)));
        let bugs: Vec<Entity> = spots
            .iter()
            .enumerate()
            .map(|(index, at)| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Ant,
                            animation: Handle::default(),
                        },
                        Transform::from_translation(*at),
                        Visibility::Visible,
                        Side::Hostile,
                        Aggro::default(),
                        Quirk::new(index as f32 * crate::squad::GOLDEN_ANGLE),
                        Wander::new(*at, index as f32),
                        Crawler::default(),
                        Detail::Full,
                    ))
                    .id()
            })
            .collect();
        let sweep = world.register_system(crate::flow::rebuild);
        let rank = world.register_system(assign_detail);
        let step = world.register_system(update);

        // How far the art hangs below the origin, which is the thing the
        // placement has to hold clear of the ground. Measured off the model
        // rather than taken from `Kind::lift`: a test that subtracts back out
        // the very number the placement put in passes with the lift set to
        // zero, which is how the first two versions of this quietly proved
        // nothing.
        //
        // It used to be read off the baked sheet, which was the right
        // instrument for the scuttlebug this was written about and is the
        // wrong one for the ant -- a cell is a silhouette under a camera tilted
        // 15 degrees down, and the near rim of a body 1.2 m long projects a
        // good 15 cm below where its own origin does. See
        // `impostor::tests::the_lift_matches_what_the_baked_sheets_show`.
        let hang = hang_in_model(Kind::Ant);
        // Counted per tier, because they fail differently and only one of them
        // is under test here.
        let (mut standing, mut sunk) = ([0; 2], [0; 2]);
        let mut deepest = 0.0_f32;
        for _ in 0..600 {
            world.run_system(sweep).expect("no sweep");
            world.run_system(rank).expect("no rank");
            world.run_system(step).expect("no step");
            for bug in &bugs {
                // Only while it is the right way up. Clinging to a wall or a
                // ceiling, the ground below is not what it is resting on.
                if world.get::<Crawler>(*bug).expect("gone").up.y < 0.99 {
                    continue;
                }
                let at = world.get::<Transform>(*bug).expect("gone").translation;
                let tier = usize::from(*world.get::<Detail>(*bug).expect("gone") == Detail::Crowd);
                let level = world.resource::<LevelData>();
                let Some((ground, _)) = level.ground_at(at) else {
                    continue;
                };
                standing[tier] += 1;
                let depth = ground - (at.y - hang);
                if depth > 0.05 {
                    sunk[tier] += 1;
                    deepest = deepest.max(depth);
                }
            }
        }
        assert!(
            standing[0] > 2_000 && standing[1] > 2_000,
            "not enough upright ticks in both tiers: {standing:?}"
        );
        // The near tier is the one under test, and it has to be clean: with the
        // lift taken out, 98% of these ticks have the model underground.
        assert!(
            sunk[0] * 200 < standing[0],
            "{} of {} near-tier ticks had the model in the ground, the worst by \
             {deepest:.2} m",
            sunk[0],
            standing[0]
        );
        // The crowd tier is allowed a little, and it is not this bug. It stands
        // on the flow field's bilinear height rather than on the collision mesh,
        // and bilinear interpolation between cell centres cuts the corner off a
        // convex ridge -- so a bug on a crest reads as sunk by a few tenths.
        // Pre-existing, documented on `FlowField::at`, and invisible at the
        // range that tier is drawn at. Bounded here so it cannot grow.
        assert!(
            sunk[1] * 20 < standing[1],
            "{} of {} crowd ticks had the model in the ground",
            sunk[1],
            standing[1]
        );
    }

    /// The other kind of corner. Walking off the end of a slab, the bug follows
    /// the far side of it down rather than stepping into the air.
    #[test]
    fn a_crawler_follows_an_edge_round_onto_the_underside() {
        let level = LevelData::new(
            vec![
                Vec3::new(-4., 0., -4.),
                Vec3::new(4., 0., -4.),
                Vec3::new(4., 0., 4.),
                Vec3::new(-4., 0., 4.),
                // The slab's outer face, hanging below its edge.
                Vec3::new(4., 0., -4.),
                Vec3::new(4., -4., -4.),
                Vec3::new(4., -4., 4.),
                Vec3::new(4., 0., 4.),
            ],
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            Vec::new(),
        );
        let (position, up) = crawl(&level, Vec3::new(4.0, 0., 0.), Vec3::Y, Vec3::X * 0.3)
            .expect("the bug stepped into the air rather than round the edge");
        assert!(up.x > 0.99, "not clinging to the outer face: up {up:?}");
        assert!(position.y < 0.0, "still on top of the slab: {position:?}");
    }

    /// Turned loose on the real castle, a bug chasing a player it cannot reach
    /// walks the place rather than fighting it.
    ///
    /// The failure this pins is the one the turn rate is there for. Deciding
    /// afresh every tick which way the player is, a bug at the foot of a wall
    /// climbs it (the way to the player is up), immediately steps back down
    /// (the way to the player is along the floor), and repeats: six hundred
    /// changes of surface in half a minute, which on screen is an ant
    /// spinning like a top in the corner.
    #[test]
    fn a_crawler_on_the_castle_does_not_spin_between_surfaces() {
        let (level, _) = crate::level::load();
        for start in [Vec3::new(-29., 3., 21.), Vec3::new(4., 3., 19.)] {
            // Somewhere it has to cross the castle to get to, so the walk takes
            // in walls, corners and the courtyard rather than open lawn.
            let goal = Vec3::new(0., 8., -30.);
            let mut position = start;
            let mut crawler = Crawler::default();
            // A change of surface is counted where the roll *finishes*, not per
            // tick. Since [`ROLL_RATE`] a corner takes several ticks to go
            // round, and each of those ticks moves `up` further than the old
            // per-tick threshold -- so the old counter would have called one
            // corner three surfaces.
            let mut was = crawler.up;
            let mut resting = crawler.up;
            let (mut flips, mut furthest) = (0, 0.0_f32);
            for _ in 0..900 {
                let before = position;
                position = crawl_towards(
                    &level,
                    position,
                    &mut crawler,
                    goal,
                    4.0,
                    FIXED_DT,
                    Kind::Ant.lift(),
                );
                if crawler.up.distance(was) < 1e-4 && crawler.up.distance(resting) > 0.5 {
                    flips += 1;
                    resting = crawler.up;
                }
                was = crawler.up;
                // The floor snap is allowed its jump; a step along a surface is
                // not, and going round an edge is the longest of them.
                if crawler.up != Vec3::Y {
                    furthest = furthest.max(position.distance(before));
                }
            }
            assert!(
                flips < 30,
                "from {start:?} it changed surface {flips} times in 900 ticks"
            );
            assert!(
                furthest < 1.0,
                "from {start:?} it jumped {furthest:.2} in one tick"
            );
            assert!(
                position.distance(start) > 5.0,
                "from {start:?} it never got anywhere, ending at {position:?}"
            );
        }
    }

    /// Nothing within reach in any direction is not a surface, and the caller
    /// puts a bug that finds itself there back on the world's floor.
    #[test]
    fn a_crawler_over_open_space_finds_nothing() {
        assert!(crawl(&room(), Vec3::new(0., 20., 0.), Vec3::Y, Vec3::X * 0.1).is_none());
    }

    /// A bug on the ceiling is stood on its head, and its model has to be too.
    #[test]
    fn a_crawler_is_stood_on_the_surface_it_is_stuck_to() {
        let rotation = orientation(Vec3::NEG_Y, Vec3::X).expect("no facing was built");
        assert!((rotation * Vec3::Y).abs_diff_eq(Vec3::NEG_Y, 1e-5));
        assert!((rotation * Vec3::Z).abs_diff_eq(Vec3::X, 1e-5));
        // Walking straight into the surface it is stuck to says nothing about
        // which way it is facing, and it keeps the facing it had.
        assert!(orientation(Vec3::Y, Vec3::Y).is_none());
    }

    /// The benchmark field: the size that was asked for, on real ground, in the
    /// same places every run.
    ///
    /// Reproducibility is the whole value of the command. A crowd scattered by
    /// a random number generator would make two runs of the benchmark differ by
    /// the layout as well as by the build, which is exactly the comparison it
    /// exists to make.
    #[test]
    fn a_benchmark_crowd_is_the_size_asked_for_and_stands_on_the_castle() {
        let (level, _) = crate::level::load();
        for count in [200, 2000] {
            let spots = crowd_spots(count, &level);
            assert_eq!(spots.len(), count, "a crowd of {count} came up short");
            for spot in &spots {
                assert!(
                    level.floor_height(*spot + Vec3::Y * 0.5).is_some(),
                    "a benchmark enemy at {spot:?} is standing on nothing"
                );
            }
        }
        assert_eq!(
            crowd_spots(500, &level),
            crowd_spots(500, &level),
            "two runs of the benchmark got two different fields"
        );
    }

    /// It spreads rather than piling up: a crowd stacked on one spot measures
    /// the cost of a stack, and `spread` would spend the whole run untangling it.
    #[test]
    fn a_benchmark_crowd_is_spread_over_the_grounds() {
        let (level, _) = crate::level::load();
        let spots = crowd_spots(1000, &level);
        let far = spots
            .iter()
            .map(|spot| Vec2::new(spot.x, spot.z).distance(CROWD_CENTRE))
            .fold(0.0_f32, f32::max);
        assert!(far > CROWD_REACH * 0.5, "the crowd only reached {far}");
        // And no two of them are placed in exactly the same spot.
        let mut apart = f32::MAX;
        for (index, spot) in spots.iter().enumerate().take(200) {
            for other in &spots[index + 1..] {
                apart = apart.min(spot.distance(*other));
            }
        }
        assert!(apart > 0.01, "two benchmark enemies are {apart} apart");
    }

    /// The budget is a count, and it is the *nearest* that count which is kept.
    ///
    /// The whole promise of the crowd work is that the expensive tier has a
    /// fixed size, so a field of five thousand costs what a field of five
    /// hundred does. A budget that leaked with the field size would be no
    /// budget at all.
    #[test]
    fn the_nearest_enemies_are_the_ones_simulated_in_full() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(GameTuning {
            sim_budget: 10.0,
            ..GameTuning::default()
        });
        world.spawn((Player, Transform::default()));
        // Placed at 1, 2, 3 ... metres out, shuffled by the golden angle so the
        // spawn order is nothing like the distance order.
        let mut placed: Vec<(f32, Entity)> = (1..=50)
            .map(|step| {
                let away = step as f32;
                let angle = step as f32 * crate::squad::GOLDEN_ANGLE;
                let at = Vec3::new(angle.sin(), 0.0, angle.cos()) * away;
                (
                    away,
                    world
                        .spawn((
                            Enemy {
                                kind: Kind::Slime,
                                animation: Handle::default(),
                            },
                            Transform::from_translation(at),
                        ))
                        .id(),
                )
            })
            .collect();
        world.run_system_once(assign_detail).expect("no run");
        placed.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (rank, (away, entity)) in placed.iter().enumerate() {
            let detail = *world.get::<Detail>(*entity).expect("no tier");
            let wanted = if rank < 10 {
                Detail::Full
            } else {
                Detail::Crowd
            };
            assert_eq!(
                detail, wanted,
                "the enemy {away} metres out, ranked {rank}, got {detail:?}"
            );
        }
    }

    /// A budget nobody exceeds demotes nobody: a small field is simulated in
    /// full, exactly as it was before any of this existed.
    #[test]
    fn a_field_inside_the_budget_is_all_simulated_in_full() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(GameTuning::default());
        world.spawn((Player, Transform::default()));
        let enemies: Vec<Entity> = (0..20)
            .map(|step| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Slime,
                            animation: Handle::default(),
                        },
                        Transform::from_xyz(step as f32 * 3.0, 0.0, 0.0),
                    ))
                    .id()
            })
            .collect();
        world.run_system_once(assign_detail).expect("no run");
        for entity in enemies {
            assert_eq!(*world.get::<Detail>(entity).unwrap(), Detail::Full);
        }
    }

    /// An enemy is born with a tier whether or not anybody remembered to give
    /// it one. `Detail` is a required component of `Enemy` precisely so that a
    /// spawn site that forgets is impossible rather than merely unlikely.
    #[test]
    fn every_enemy_has_a_tier_without_being_given_one() {
        let mut world = World::new();
        let enemy = world
            .spawn(Enemy {
                kind: Kind::Slime,
                animation: Handle::default(),
            })
            .id();
        assert_eq!(world.get::<Detail>(enemy).copied(), Some(Detail::Full));
    }

    /// A player at the origin and enemies stood where they are asked for.
    /// Open ground and nothing else, for the tests that are about bodies rather
    /// than about the level: [`spread`] resolves what it shoves against the
    /// walls, and a world with no [`LevelData`] in it has no walls to resolve
    /// against and no resource for the system to take.
    fn lawn() -> LevelData {
        LevelData::new(
            vec![
                Vec3::new(-200., 0., -200.),
                Vec3::new(200., 0., -200.),
                Vec3::new(200., 0., 200.),
                Vec3::new(-200., 0., 200.),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
            Vec::new(),
        )
    }

    /// A world with the lawn in it and the flow field over it.
    ///
    /// [`spread`] wants the field as well as the level now: a cheap-tier body's
    /// shove is resolved against the same table [`crowd_step`] walks by, so a
    /// world that can be shoved in is a world that has one.
    fn lawn_world() -> World {
        let mut world = World::new();
        world.init_resource::<Threats>();
        let level = lawn();
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world
    }

    fn field(placed: &[Vec3]) -> (World, Entity, Vec<Entity>) {
        let mut world = lawn_world();
        world.insert_resource(GameTuning::default());
        let player = world
            .spawn((Player, Side::Friendly, Transform::default()))
            .id();
        let enemies = placed
            .iter()
            .map(|at| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Slime,
                            animation: Handle::default(),
                        },
                        Side::Hostile,
                        Aggro::default(),
                        Wander::new(*at, 0.0),
                        Transform::from_translation(*at),
                    ))
                    .id()
            })
            .collect();
        (world, player, enemies)
    }

    fn aggro(world: &mut World, enemy: Entity) -> Option<Entity> {
        world.get::<Aggro>(enemy).expect("no aggro").target
    }

    /// The chain reaction. One enemy sees the player; the one behind it hears
    /// that, and the one behind that hears it in turn -- all on the tick the
    /// first one noticed, and all pointed at the same player. The fourth is a
    /// stride too far back and hears nothing.
    #[test]
    fn one_enemy_noticing_the_player_turns_the_whole_line_round() {
        let tuning = GameTuning::default();
        let (sight, earshot) = (tuning.enemy_sight, tuning.enemy_alert);
        let line = [
            Vec3::new(sight - 1.0, 0., 0.),
            Vec3::new(sight - 1.0 + earshot - 1.0, 0., 0.),
            Vec3::new(sight - 1.0 + (earshot - 1.0) * 2.0, 0., 0.),
            Vec3::new(sight - 1.0 + (earshot - 1.0) * 2.0 + earshot + 2.0, 0., 0.),
        ];
        let (mut world, player, enemies) = field(&line);
        world.run_system_once(alert).expect("alert could not run");
        for (index, enemy) in enemies.iter().take(3).enumerate() {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(player),
                "number {index} in the line never heard the alarm"
            );
        }
        assert_eq!(
            aggro(&mut world, enemies[3]),
            None,
            "the alarm carried past the gap in the line"
        );
        // And what it heard is where the player is, which is what it walks to.
        let heard = world.get::<Aggro>(enemies[2]).unwrap().at;
        assert_eq!(heard, Vec3::ZERO);
    }

    /// Aggro is not a leash. Once an enemy has noticed the player it keeps
    /// coming however far away he gets, and keeps being told where he is.
    #[test]
    fn an_enemy_that_has_noticed_the_player_never_loses_interest() {
        let (mut world, player, enemies) = field(&[Vec3::new(2., 0., 0.)]);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(aggro(&mut world, enemies[0]), Some(player));
        // Right across the castle, far outside anything it could see.
        let away = Vec3::new(300., 0., 300.);
        world.get_mut::<Transform>(player).unwrap().translation = away;
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(
            aggro(&mut world, enemies[0]),
            Some(player),
            "it gave up because he walked away"
        );
        assert_eq!(
            world.get::<Aggro>(enemies[0]).unwrap().at,
            away,
            "it is still walking to where he used to be"
        );
    }

    /// A line of slimes that have all committed to the player, and a Mario
    /// standing among them about to make a nuisance of itself.
    ///
    /// The Mario is spawned *after* the first pass on purpose: it is the fight
    /// already in progress that the attention model is about, and a Mario that
    /// was there from the start would just be the nearest thing to some of them.
    ///
    /// `alert` is **registered** rather than run with `run_system_once`, and
    /// that is load-bearing rather than tidiness. `run_system_once` builds a
    /// fresh system every call, so its `Local<Noticing>` -- which is where the
    /// tick counter the stagger phases off lives -- resets to zero every time.
    /// Run that way the whole field is frozen on one phase, one enemy in six
    /// ever appraises at all, and the rest sit on their first target forever.
    fn quarrel(
        count: usize,
    ) -> (
        World,
        bevy::ecs::system::SystemId,
        Entity,
        Entity,
        Vec<Entity>,
    ) {
        let spots: Vec<Vec3> = (0..count)
            .map(|which| Vec3::new(2.0 + which as f32 * 0.6, 0.0, 0.0))
            .collect();
        let (mut world, player, enemies) = field(&spots);
        let notice = world.register_system(alert);
        world.run_system(notice).expect("alert could not run");
        for enemy in &enemies {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(player),
                "the line was supposed to start out fighting the player"
            );
        }
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::from_xyz(2.0 + count as f32 * 0.3, 0.0, 0.8),
            ))
            .id();
        (world, notice, player, mario, enemies)
    }

    /// Runs `ticks` of noticing, with the Mario killing something where it
    /// stands every `every` ticks. `every` of zero means it kills nothing and
    /// is only standing there.
    fn brawl(
        world: &mut World,
        notice: bevy::ecs::system::SystemId,
        mario: Entity,
        ticks: usize,
        every: usize,
    ) {
        let at = world.get::<Transform>(mario).unwrap().translation;
        for tick in 0..ticks {
            if every > 0 && tick % every == 0 {
                world.resource_mut::<Threats>().kill(mario, at);
            }
            world.run_system(notice).expect("alert could not run");
        }
    }

    /// The whole point. A Mario killing something in the middle of a fight does
    /// not take that fight over on the tick of the punch -- which is exactly
    /// what the old chain did, because a shout re-pointed whoever heard it
    /// whether or not they were busy.
    #[test]
    fn one_marios_kill_does_not_turn_the_fight_round() {
        let (mut world, notice, player, mario, enemies) = quarrel(4);
        brawl(&mut world, notice, mario, 12, 12);
        for enemy in &enemies {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(player),
                "one kill was enough to take the fight off the player"
            );
        }
    }

    /// Standing there is not enough either, however close it is standing. A
    /// Mario that never does anything is worth less than the fight already in
    /// progress, which is what the margin is for.
    #[test]
    fn a_mario_that_only_stands_about_never_takes_the_fight() {
        let (mut world, notice, player, mario, enemies) = quarrel(4);
        brawl(&mut world, notice, mario, 150, 0);
        for enemy in &enemies {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(player),
                "it stole the fight by loitering"
            );
        }
    }

    /// And the other half: a Mario that keeps killing does get the fight, in a
    /// second or two rather than never. A model that only ever refuses to
    /// switch is not attention, it is a latch with extra steps.
    #[test]
    fn a_mario_that_keeps_killing_takes_the_fight_off_the_player() {
        let (mut world, notice, _, mario, enemies) = quarrel(4);
        brawl(&mut world, notice, mario, 60, 8);
        for enemy in &enemies {
            assert_eq!(
                aggro(&mut world, *enemy),
                Some(mario),
                "it went on fighting the player through a massacre"
            );
        }
    }

    /// And it takes it a few at a time. This is the one that says the fight
    /// looks like a crowd changing its mind rather than a switch being thrown:
    /// with one Mario killing the same things at the same range, the line still
    /// turns on different ticks.
    ///
    /// It is also the test that caught the switch being unstaggered. Attention
    /// arrives synchronously -- one kill is credited to every witness on the
    /// same tick -- so scores that were accumulated gradually still *cross*
    /// together, and eight enemies turned as one on tick 16. See
    /// [`ATTENTION_SPAN`].
    #[test]
    fn a_crowd_does_not_all_turn_on_the_same_tick() {
        let (mut world, notice, _, mario, enemies) = quarrel(8);
        let at = world.get::<Transform>(mario).unwrap().translation;
        let mut turned: Vec<Option<usize>> = vec![None; enemies.len()];
        for tick in 0..90 {
            if tick % 8 == 0 {
                world.resource_mut::<Threats>().kill(mario, at);
            }
            world.run_system(notice).expect("alert could not run");
            for (which, enemy) in enemies.iter().enumerate() {
                if turned[which].is_none() && aggro(&mut world, *enemy) == Some(mario) {
                    turned[which] = Some(tick);
                }
            }
        }
        let when: Vec<usize> = turned
            .iter()
            .map(|turned| turned.expect("one of the line never turned at all"))
            .collect();
        let first = when.iter().min().unwrap();
        let last = when.iter().max().unwrap();
        assert!(
            last > first,
            "the whole line turned on tick {first}, which is the stampede this is here to stop"
        );
    }

    /// Two enemies may not stand in the same place, however hard the thing
    /// they are both chasing pulls them together.
    #[test]
    fn enemies_are_held_out_of_one_another() {
        let together = Vec3::new(5., 0., 5.);
        let (mut world, _, enemies) = field(&[together, together]);
        for _ in 0..60 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let apart = world.get::<Transform>(enemies[0]).unwrap().translation
            - world.get::<Transform>(enemies[1]).unwrap().translation;
        let room = Kind::Slime.body().0 * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        assert!(
            apart.length() > room - 0.01,
            "two enemies settled {} apart, inside the {room} they are owed",
            apart.length()
        );
    }

    /// Nothing is ever told to stand somewhere [`spread`] will not let it stand.
    ///
    /// This is the rule that "units on top of each other" turned out to be. A
    /// chaser walks to a point and the shove holds it off a body, and if the
    /// point is inside the body the two argue about its position for as long as
    /// the fight lasts -- the chaser walked in every tick, pushed out every
    /// tick, drawn inside the thing it is attacking. It is invisible while the
    /// actors are small, because every one of these distances was written
    /// against a goomba 0.6 m across and comfortably clears one. An ant is 5 m
    /// across and clears nothing.
    ///
    /// So each approach distance is checked against the room the shove will
    /// insist on, for every pair of kinds that can chase each other. Measured
    /// against the *widest* actor there is rather than against a particular one,
    /// because the next re-export is allowed to change which that is.
    /// A kind with no disc is spawned without the component that draws one,
    /// and a kind with one still gets it.
    ///
    /// Through `spawn` rather than by asking `shadow_radius` twice, because
    /// what a slime must not have is the *component*: `attach` finds its
    /// casters by querying for it, so anything carrying one gets a disc no
    /// matter what radius it was built from.
    #[test]
    fn only_the_kinds_that_cast_a_shadow_carry_a_caster() {
        let mut app = App::new();
        app.add_plugins(AssetPlugin::default())
            // `spawn` asks the server for a clip, which it will not hand out
            // for a type nothing has registered.
            .init_asset::<AnimationClip>();
        for (kind, wanted) in [(Kind::Slime, false), (Kind::Ant, true)] {
            let world = app.world_mut();
            let enemy = world
                .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                    spawn(&mut commands, &assets, kind, Vec3::ZERO, 0.0)
                })
                .expect("spawn could not run");
            assert_eq!(
                world.get::<crate::shadow::ShadowCaster>(enemy).is_some(),
                wanted,
                "{kind:?} came out of `spawn` with its shadow the wrong way round"
            );
        }
    }

    #[test]
    fn nothing_walks_to_a_spot_it_would_be_shoved_out_of() {
        let widest = KINDS
            .iter()
            .fold(crate::player::PLAYER_RADIUS, |most, kind| {
                most.max(kind.body().0)
            });
        for kind in KINDS {
            let (mine, _) = kind.body();
            for theirs in KINDS.iter().map(|kind| kind.body().0).chain([widest]) {
                // The nearest the shove will let these two settle.
                let held = mine + theirs + PERSONAL_SPACE - SPREAD_SLACK;

                // An enemy of `kind` chasing a body of radius `theirs`.
                let stood = Quirk::new(0.0).stand_off(theirs + mine).length();
                assert!(
                    stood >= held,
                    "an enemy {mine:.2} across aims {stood:.2} from something \
                     {theirs:.2} across, which the shove holds at {held:.2}"
                );
            }
            // And a Mario walking up to one to punch it, which is the pair the
            // report was actually about.
            let mario = crate::player::PLAYER_RADIUS;
            let held = mario + mine + PERSONAL_SPACE - SPREAD_SLACK;
            let walks = mine + mario + crate::squad::STRIKE_RANGE;
            assert!(
                walks >= held,
                "a Mario walks to {walks:.2} from an actor {mine:.2} across, \
                 which the shove holds at {held:.2}"
            );
            // Having walked there, its punch has to reach.
            assert!(
                mine + MARIO_REACH >= walks,
                "a Mario stands at {walks:.2} and punches {:.2}",
                mine + MARIO_REACH
            );
            // And the player's sword has to reach what the shove holds off him.
            let held = mario + mine + PERSONAL_SPACE - SPREAD_SLACK;
            assert!(
                mine + ATTACK_REACH >= held,
                "the player's swing reaches {:.2} and the shove holds an actor \
                 {mine:.2} across at {held:.2}",
                mine + ATTACK_REACH
            );
        }
    }

    /// The spatial hash answers with everything nearby, and with nothing twice.
    ///
    /// Two properties, and the second is the one that needed a test written for
    /// it. Addressing cells directly cannot hand a point back twice; *hashing*
    /// them can, because two of the nine cells a query reads may share a bucket
    /// and reading that bucket twice returns everything in it twice. A shove
    /// that counted one neighbour double would push half again as hard in that
    /// direction, on some ticks, for some pairs -- a bug that would show up as
    /// an occasional twitch in a crowd and would be very hard to find from
    /// there.
    ///
    /// Two thousand points, because the table is two buckets a point and a
    /// nine-cell query at that size collides about once in a hundred: rare
    /// enough that a smaller test would pass on a broken grid, common enough
    /// that two thousand queries meet it a dozen times over.
    #[test]
    fn the_spatial_hash_never_hands_back_the_same_body_twice() {
        let cell = 1.5;
        let points: Vec<Vec3> = (0..2000)
            .map(|index| {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                Vec3::new(angle.sin(), 0.0, angle.cos()) * (index as f32).sqrt() * cell
            })
            .collect();
        let mut grid = Neighbourhood::default();
        grid.fill(points.iter().copied(), cell);
        let mut found = Vec::new();
        let mut seen = vec![false; points.len()];
        for (index, point) in points.iter().enumerate() {
            grid.near(*point, &mut found);
            for &other in &found {
                assert!(
                    !seen[other],
                    "the query at point {index} returned point {other} twice"
                );
                seen[other] = true;
            }
            for &other in &found {
                seen[other] = false;
            }
            // And it is a bucket rather than a filter: everything within a cell
            // of the query must be in the answer, or a pair that is overlapping
            // never finds out. Checked over the first two hundred only, because
            // the check itself is the quadratic scan the grid exists to avoid.
            if index < 200 {
                for (other, theirs) in points.iter().enumerate() {
                    if point.distance(*theirs) < cell * 0.999 {
                        assert!(
                            found.contains(&other),
                            "point {other} is inside a cell of point {index} and was missed"
                        );
                    }
                }
            }
        }
    }

    /// And the cheap tier is held apart too, which it was not.
    ///
    /// [`spread`] used to take the near tier only, so a field of two thousand
    /// was two hundred creatures standing politely apart and eighteen hundred
    /// stacked into each other. This is the same heap as
    /// [`enemies_are_held_out_of_one_another`] with every one of them demoted:
    /// it must come apart the same way, only a quarter as often.
    ///
    /// Registered rather than run one-shot, because [`CROWD_SPREAD_STRIDE`] is
    /// counted off a `Local` that a fresh system every call would reset to zero
    /// -- which would shove the same quarter of the field forever and leave the
    /// other three quarters in the pile.
    #[test]
    fn the_cheap_tier_is_held_apart_as_well_as_the_near_one() {
        let heap: Vec<Vec3> = (0..24)
            .map(|index| {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                Vec3::new(angle.sin(), 0., angle.cos()) * (index as f32 * 0.04)
            })
            .collect();
        let (mut world, _, enemies) = field(&heap);
        for enemy in &enemies {
            world.entity_mut(*enemy).insert(Detail::Crowd);
        }
        let jostle = world.register_system(spread);
        // Four times the ticks the near tier is given, which is the whole of
        // what the stride costs: the same settled crowd, arrived at later.
        for _ in 0..2400 {
            world.run_system(jostle).expect("spread could not run");
        }
        let places: Vec<Vec3> = enemies
            .iter()
            .map(|enemy| world.get::<Transform>(*enemy).expect("gone").translation)
            .collect();
        let room = Kind::Slime.body().0 * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        for (index, one) in places.iter().enumerate() {
            for other in &places[index + 1..] {
                assert!(
                    one.distance(*other) > room - 0.05,
                    "two of the cheap tier ended up {} apart, inside the {room} they are owed",
                    one.distance(*other)
                );
            }
        }
        // And having settled, it holds still. A body shoved once every four
        // ticks is given four ticks' worth of rate to make up for it, and a
        // rate that overshoots is a crowd that shuffles forever -- which is
        // what [`SPREAD_RATE_CAP`] is for.
        for _ in 0..40 {
            world.run_system(jostle).expect("spread could not run");
        }
        let moved = enemies
            .iter()
            .zip(&places)
            .fold(0.0_f32, |most, (enemy, was)| {
                let now = world.get::<Transform>(*enemy).expect("gone").translation;
                most.max(was.distance(now))
            });
        assert!(
            moved < 0.01,
            "a settled cheap tier was still shuffling {moved}"
        );
    }

    /// A press of cheap-tier bodies expanding across the castle never pushes one
    /// of its own through a wall.
    ///
    /// This is the shove's half of what
    /// [`a_crowd_walks_the_castle_without_walking_through_it`] checks for the
    /// step, and it needs its own test because the two resolve against different
    /// things: the near tier's shove is put back by `resolve_walls`, and a
    /// cheap-tier one asks the flow field, which is the only thing it is allowed
    /// to ask. A heap of three hundred at one spot is a press that keeps pushing
    /// outward for as long as it is run, so it reaches the courtyard's walls
    /// under its own steam rather than being aimed at them.
    ///
    /// It has teeth, and only just enough of them to be worth having: with the
    /// [`crate::flow::FlowField::clear`] guard taken out of the shove and the
    /// push simply taken, this reports **13 crossings** out of 180,000
    /// enemy-ticks. A shove is a few centimetres and a wall is metres thick, so
    /// posting one through takes a press leaning on it for a while -- which is
    /// exactly the case the near tier's own wall resolution was added for, and
    /// exactly why the cheap tier needs one too.
    #[test]
    fn a_press_of_the_cheap_tier_never_shoves_one_of_its_own_through_a_wall() {
        let (level, _) = crate::level::load();
        let mut world = World::new();
        world.insert_resource(crate::flow::FlowField::new(&level));
        let courtyard = Vec3::new(-13.28, 3.0, 46.64);
        let enemies: Vec<Entity> = (0..300)
            .map(|index| {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                let at =
                    courtyard + Vec3::new(angle.sin(), 0., angle.cos()) * (index as f32 * 0.004);
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Slime,
                            animation: Handle::default(),
                        },
                        Transform::from_translation(at),
                        Detail::Crowd,
                    ))
                    .id()
            })
            .collect();
        world.insert_resource(level);
        let jostle = world.register_system(spread);
        let knee = Vec3::Y * STEP_UP;
        let mut was: Vec<Vec3> = enemies
            .iter()
            .map(|enemy| world.get::<Transform>(*enemy).expect("gone").translation)
            .collect();
        let mut through = 0;
        for _ in 0..600 {
            world.run_system(jostle).expect("spread could not run");
            for (index, enemy) in enemies.iter().enumerate() {
                let now = world.get::<Transform>(*enemy).expect("gone").translation;
                let moved = Vec3::new(now.x - was[index].x, 0.0, now.z - was[index].z);
                if moved.length_squared() > 1e-8 {
                    let from = was[index] + knee;
                    let level = world.resource::<LevelData>();
                    let hit = level
                        .surface_hit(from, from + moved)
                        .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y);
                    // Only what left a cell, for the reason
                    // [`a_crowd_walks_the_castle_without_walking_through_it`]
                    // gives at length: a grid cannot record a fence that does
                    // not separate two cell centres, and neither tier claims it
                    // can. The shove is asked for exactly what the step is.
                    let field = world.resource::<crate::flow::FlowField>();
                    if hit && !field.same_cell(was[index], now) {
                        through += 1;
                    }
                }
                was[index] = now;
            }
        }
        assert_eq!(through, 0, "the press shoved its own through a wall");
        // And it really did press outward far enough to meet something. Without
        // this the test passes just as well on a crowd that never moved.
        let widest = was
            .iter()
            .fold(0.0_f32, |most, at| most.max(at.distance(courtyard)));
        assert!(
            widest > 8.0,
            "the press only reached {widest:.1} m, which is short of anything to hit"
        );
    }

    /// A crowd untangles and then holds still. The one in the middle of a press
    /// has neighbours leaning on it from every side, and if the shove takes out
    /// the whole overlap every tick, what it does with that is vibrate.
    #[test]
    fn a_packed_crowd_settles_instead_of_jittering() {
        let heap: Vec<Vec3> = (0..20)
            .map(|index| {
                let angle = index as f32 * crate::squad::GOLDEN_ANGLE;
                Vec3::new(angle.sin(), 0., angle.cos()) * (index as f32 * 0.05)
            })
            .collect();
        let (mut world, _, enemies) = field(&heap);
        let places = |world: &mut World| -> Vec<Vec3> {
            enemies
                .iter()
                .map(|enemy| world.get::<Transform>(*enemy).unwrap().translation)
                .collect()
        };
        for _ in 0..600 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let settled = places(&mut world);
        for _ in 0..30 {
            world.run_system_once(spread).expect("spread could not run");
        }
        let after = places(&mut world);
        let moved = settled
            .iter()
            .zip(&after)
            .fold(0.0_f32, |most, (was, now)| most.max(was.distance(*now)));
        assert!(
            moved < 0.01,
            "a settled crowd was still shuffling {moved} a tick"
        );
        let room = Kind::Slime.body().0 * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        for (index, one) in after.iter().enumerate() {
            for other in &after[index + 1..] {
                assert!(
                    one.distance(*other) > room - 0.05,
                    "two of the crowd ended up {} apart",
                    one.distance(*other)
                );
            }
        }
    }

    /// An enemy that has reached the player holds still instead of shuffling
    /// back and forth across its own standoff spot.
    ///
    /// The chase has no arrival to declare -- what it is walking to moves with
    /// what it is chasing -- so [`tangent`] hands back a *direction* and the
    /// step used to be taken at full length however near the goal had got.
    /// Something already stood on its spot walked past it, turned round, walked
    /// past it the other way, and did that every tick for as long as the fight
    /// lasted. Two things gave it away, and the second is the loud one: the
    /// position wobbles by a step, and the facing swings a full half-turn.
    ///
    /// Run without [`spread`], so what is under test is the walk on its own
    /// rather than a crowd settling. One enemy, because every enemy in a ring
    /// round the player is doing the same thing at the same time and one of them
    /// is enough to see it.
    #[test]
    fn an_enemy_that_has_arrived_stands_still_instead_of_shuffling() {
        // `update` iterates in parallel, which needs a pool to iterate on.
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let (mut world, player, enemies) = field(&[Vec3::new(0.0, 0.0, 6.0)]);
        let enemy = enemies[0];
        world
            .entity_mut(enemy)
            .insert((Detail::Full, Quirk::new(1.0), Visibility::Visible));
        world.insert_resource(Time::<Fixed>::default());
        // Given the player directly, so the walk is under test rather than
        // `alert`'s noticing of him.
        world.get_mut::<Aggro>(enemy).unwrap().target = Some(player);
        let step = world.register_system(update);
        // Long enough to be well past the walk in and settled onto the spot.
        for _ in 0..600 {
            world.run_system(step).expect("no step");
        }
        let mut was = *world.get::<Transform>(enemy).unwrap();
        let (mut wobble, mut swing) = (0.0_f32, 0.0_f32);
        for _ in 0..120 {
            world.run_system(step).expect("no step");
            let now = *world.get::<Transform>(enemy).unwrap();
            wobble = wobble.max(was.translation.distance(now.translation));
            swing = swing.max(was.rotation.angle_between(now.rotation));
            was = now;
        }
        // A tick of walking at the chase speed, which is the size of the
        // overshoot and so the size of the shuffle it caused.
        let stride = GameTuning::default().enemy_speed * crate::player::FIXED_DT;
        assert!(
            wobble < stride * 0.5,
            "an arrived enemy moved {wobble:.4} m a tick, against a stride of {stride:.4}"
        );
        assert!(
            swing < std::f32::consts::FRAC_PI_2,
            "an arrived enemy swung {:.0} degrees a tick",
            swing.to_degrees()
        );
    }

    /// And a crawler is shoved along its surface rather than off it: two bugs
    /// jostling on a wall stay on the wall.
    #[test]
    fn crawlers_are_shoved_along_the_surface_they_are_stuck_to() {
        let mut world = lawn_world();
        let wall = Vec3::new(4., 3., 0.);
        let pair: Vec<Entity> = [wall, wall + Vec3::new(0., 0.2, 0.)]
            .iter()
            .map(|at| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Ant,
                            animation: Handle::default(),
                        },
                        Crawler {
                            up: Vec3::NEG_X,
                            heading: Vec3::Y,
                        },
                        Transform::from_translation(*at),
                    ))
                    .id()
            })
            .collect();
        for _ in 0..60 {
            world.run_system_once(spread).expect("spread could not run");
        }
        for bug in pair {
            let at = world.get::<Transform>(bug).unwrap().translation;
            assert!(
                (at.x - wall.x).abs() < 1e-4,
                "a bug was shoved off its wall, to {at:?}"
            );
        }
    }

    /// The amble: a spot in its own patch, a walk to it, and a rest before the
    /// next one. The rest is the point -- a walker that picks a new spot the
    /// moment it arrives never stands still and never finishes a stride.
    #[test]
    fn a_wandering_enemy_walks_its_patch_and_rests_between_spots() {
        let home = Vec3::new(10., 0., -4.);
        let mut wander = Wander::new(home, 1.0);
        let goal_after =
            |wander: &mut Wander, at: Vec3| (0..600).find_map(|_| wander.goal(at, FIXED_DT));
        let first = goal_after(&mut wander, home).expect("it never set off anywhere");
        assert!(
            Vec2::new(first.x - home.x, first.z - home.z).length() <= WANDER_RADIUS + 1e-4,
            "it ambled clean out of its patch, to {first:?}"
        );
        // Stood on the spot, it stops rather than turning straight round.
        assert_eq!(wander.goal(first, FIXED_DT), None, "it never stops");
        assert_eq!(wander.goal(first, FIXED_DT), None, "its rest was one tick");
        let next = goal_after(&mut wander, first).expect("it never set off again");
        assert!(next != first, "it went back to the spot it was already on");
    }

    /// A Mario and a slime, stood within arm's reach of each other, with the
    /// Mario having noticed it.
    fn duel(apart: f32) -> (World, Entity, Entity) {
        duel_with(Kind::Slime, apart)
    }

    /// The same, against whichever kind the test is about. Both sides carry
    /// their real hit points, so what these run is the arithmetic the game
    /// runs rather than `health::strike`'s no-component fallback.
    fn duel_with(kind: Kind, apart: f32) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Threats>();
        // The one die in the game. Every path that kills something rolls it --
        // see `nuclonium::Drops::maybe` -- so a world that means to resolve a fight
        // has to have it, exactly as it has to have the threat queue.
        world.init_resource::<crate::nuclonium::Drops>();
        world.insert_resource(SoundQueue::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Health::new(health::MARIO_HEALTH),
                Transform::default(),
            ))
            .id();
        let foe = world
            .spawn((
                Enemy {
                    kind,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Health::new(kind.health()),
                Transform::from_xyz(apart, 0., 0.),
            ))
            .id();
        world.get_mut::<Aggro>(mario).unwrap().target = Some(foe);
        (world, mario, foe)
    }

    /// How many ticks a Mario's whole punch takes, wind-up included.
    fn a_full_punch() -> usize {
        (MARIO_SWING / FIXED_DT).ceil() as usize + 1
    }

    fn swing(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world
                .run_system_once(ally_combat)
                .expect("ally_combat could not run");
        }
    }

    /// A Mario stood over something on the other side hits it, and what it hits
    /// dies. It takes the length of the punch to do it: the blow lands when the
    /// swing finishes, not when it starts.
    #[test]
    fn a_mario_punches_what_it_has_noticed() {
        let (mut world, mario, slime) = duel(1.0);
        swing(&mut world, 1);
        assert!(
            world.get_entity(slime).is_ok(),
            "the slime died on the wind-up"
        );
        assert_eq!(
            world.get::<Ally>(mario).unwrap().state.motion,
            Motion::Attack
        );
        swing(&mut world, a_full_punch());
        assert!(
            world.get_entity(slime).is_err(),
            "the punch landed on nothing"
        );
    }

    /// And one that wanders out of reach while the punch is in the air is
    /// missed, which is the only thing that makes standing next to a Mario
    /// survivable.
    #[test]
    fn a_mario_misses_what_walks_out_of_the_punch() {
        let (mut world, _, slime) = duel(1.0);
        swing(&mut world, 1);
        world.get_mut::<Transform>(slime).unwrap().translation.x = MARIO_REACH + 3.0;
        swing(&mut world, a_full_punch());
        assert!(
            world.get_entity(slime).is_ok(),
            "the punch followed it across the lawn"
        );
    }

    /// A Mario is worth half the player's blow, so an ant survives the first
    /// punch and not the second.
    ///
    /// This is the whole reason the two tables in [`crate::health`] are the
    /// numbers they are: a lone Mario has to *trade* with an ant rather than
    /// delete it, which is what makes sending one somewhere a decision.
    #[test]
    fn an_ant_takes_two_of_a_marios_punches() {
        let (mut world, _, ant) = duel_with(Kind::Ant, 1.0);
        swing(&mut world, a_full_punch());
        assert!(
            world.get_entity(ant).is_ok(),
            "one punch should not fell an ant"
        );
        assert_eq!(
            world.get::<Health>(ant).unwrap().current,
            health::ANT_HEALTH - health::MARIO_DAMAGE,
            "the punch landed for the wrong number"
        );
        swing(&mut world, a_full_punch());
        assert!(
            world.get_entity(ant).is_err(),
            "the second punch left it standing"
        );
    }

    /// And the other way round: what the ant does back.
    ///
    /// Twenty points against three a touch is seven touches, spaced a whole
    /// invulnerability window apart -- so this is several seconds of standing
    /// against one rather than an instant loss, and a Mario that walks away
    /// keeps what it has left.
    #[test]
    fn an_ant_stood_against_a_mario_wears_it_down() {
        let (mut world, mario, ant) = duel_with(Kind::Ant, 1.0);
        // The ant is the one doing the noticing here.
        world.get_mut::<Aggro>(mario).unwrap().target = None;
        world.entity_mut(ant).insert(Aggro {
            target: Some(mario),
            ..Aggro::default()
        });
        let maul_once = |world: &mut World| {
            world.run_system_once(maul).expect("maul could not run");
        };
        maul_once(&mut world);
        assert_eq!(
            world.get::<Health>(mario).unwrap().current,
            health::MARIO_HEALTH - health::ANT_DAMAGE,
            "the first touch cost the wrong number"
        );
        // Ten more ticks is a third of a second, and the window is a whole one.
        for _ in 0..10 {
            maul_once(&mut world);
        }
        assert_eq!(
            world.get::<Health>(mario).unwrap().current,
            health::MARIO_HEALTH - health::ANT_DAMAGE,
            "hit again inside its own invulnerability window"
        );
        // Long enough for all seven touches, and then some.
        for _ in 0..300 {
            maul_once(&mut world);
        }
        assert!(
            world.get_entity(mario).is_err(),
            "a Mario stood against an ant for ten seconds lived through it"
        );
    }

    /// An enemy chasing the player walks through the Marios on the way without
    /// touching them, because what it hurts is what it is fighting.
    #[test]
    fn an_enemy_does_not_maul_a_mario_it_is_not_fighting() {
        let (mut world, mario, ant) = duel_with(Kind::Ant, 0.5);
        world.get_mut::<Aggro>(mario).unwrap().target = None;
        // Aggro'd on nothing at all, stood right on top of one.
        world.entity_mut(ant).insert(Aggro::default());
        world.run_system_once(maul).expect("maul could not run");
        assert_eq!(
            world.get::<Health>(mario).unwrap().current,
            health::MARIO_HEALTH,
            "hurt by something that had not noticed it"
        );
    }

    /// Out of reach to begin with, a Mario does not swing at the air: walking
    /// to it is the movement step's business.
    #[test]
    fn a_mario_does_not_punch_at_something_across_the_lawn() {
        let (mut world, mario, slime) = duel(MARIO_REACH + 2.0);
        swing(&mut world, 30);
        assert!(world.get_entity(slime).is_ok());
        assert_eq!(world.get::<Ally>(mario).unwrap().swing_left, 0.0);
    }

    /// **An enemy goes for whoever is coming for it, not for whoever is
    /// nearest.**
    ///
    /// The report this is here for is one complaint from both ends: Marios
    /// cannot land a punch on anything that is not already standing on them,
    /// and enemies walk straight through the Mario fighting them after
    /// somebody who has not so much as looked up. Both are the same missing
    /// fact -- that a creature with its fists up is a different thing from one
    /// walking past on an errand, and nothing in a score made of distances can
    /// tell them apart. See [`GameTuning::threat_focus`].
    #[test]
    fn an_enemy_turns_on_the_mario_fighting_it_rather_than_the_one_walking_past() {
        let mut world = World::new();
        world.init_resource::<Threats>();
        world.insert_resource(GameTuning::default());
        // One that has chosen the fight, further off, and one on an errand,
        // nearer. Without the preference the nearer one wins outright, which is
        // what "chasing after units that have a different goal" looks like.
        let mut mario = |at: f32, plan: crate::goap::Goal| {
            let mut ally = Ally::new(Vec3::new(at, 0., 0.), 0.0);
            ally.plan = plan;
            world
                .spawn((
                    ally,
                    Side::Friendly,
                    Aggro::default(),
                    Transform::from_xyz(at, 0., 0.),
                ))
                .id()
        };
        let fetching = mario(
            5.0,
            crate::goap::Goal::Fetch {
                ball: Entity::from_raw_u32(9).unwrap(),
                at: Vec2::ZERO,
                arrive: 1.0,
            },
        );
        let fighting = mario(
            1.0,
            crate::goap::Goal::Fight {
                at: Vec2::new(6., 0.),
                arrive: 1.0,
            },
        );
        let slime = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Aggro::default(),
                Transform::from_xyz(6., 0., 0.),
            ))
            .id();
        // The one that has chosen the fight is coming for this slime, which is
        // what the plan above says and what `alert` would have written down on
        // the tick it decided.
        world.get_mut::<Aggro>(fighting).unwrap().target = Some(slime);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(
            world.get::<Aggro>(slime).unwrap().target,
            Some(fighting),
            "it set off after the Mario that had not noticed it"
        );
        // And it stays turned. The boost is on the fight it is in as well as on
        // what it can see, or the nearer Mario takes it straight back off it.
        for _ in 0..ATTENTION_SPAN * 20 {
            world.run_system_once(alert).expect("alert could not run");
        }
        assert_eq!(
            world.get::<Aggro>(slime).unwrap().target,
            Some(fighting),
            "the Mario walking past pulled it off the one hitting it"
        );
        assert_ne!(fetching, fighting);
    }

    /// Both sides notice each other, off the one rule. The Mario has a slime
    /// to hit and the slime has a Mario to chase, out of a single pass.
    #[test]
    fn a_mario_and_a_slime_notice_each_other() {
        let mut world = World::new();
        world.init_resource::<Threats>();
        world.insert_resource(GameTuning::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::default(),
            ))
            .id();
        let slime = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Aggro::default(),
                Transform::from_xyz(4., 0., 0.),
            ))
            .id();
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(world.get::<Aggro>(mario).unwrap().target, Some(slime));
        assert_eq!(world.get::<Aggro>(slime).unwrap().target, Some(mario));
        // And what a Mario kills stops being a target, so it looks for the next
        // one rather than standing over the spot.
        world.despawn(slime);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(world.get::<Aggro>(mario).unwrap().target, None);
    }

    /// And what it loses it replaces, out of the same pass: a Mario that has
    /// just flattened the slime it was fighting takes the next one within sight
    /// on the tick the first one goes, rather than standing over the spot until
    /// something walks into it.
    ///
    /// The re-acquire runs through `Hunt::seize` rather than through the
    /// margin, and that is the point of asserting it here. An emptied slot is
    /// an *acquire*, not a switch, so nothing has to out-earn the corpse -- the
    /// second slime is taken whole, at whatever range it is at, in one tick.
    #[test]
    fn a_mario_takes_the_next_enemy_when_the_one_it_was_fighting_dies() {
        let mut world = World::new();
        world.init_resource::<Threats>();
        world.insert_resource(GameTuning::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::default(),
            ))
            .id();
        let mut slime = |x: f32| {
            world
                .spawn((
                    Enemy {
                        kind: Kind::Slime,
                        animation: Handle::default(),
                    },
                    Side::Hostile,
                    Aggro::default(),
                    Transform::from_xyz(x, 0., 0.),
                ))
                .id()
        };
        let near = slime(2.);
        let far = slime(6.);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(
            world.get::<Aggro>(mario).unwrap().target,
            Some(near),
            "the nearer of the two is what it notices first"
        );
        world.despawn(near);
        world.run_system_once(alert).expect("alert could not run");
        let aggro = world.get::<Aggro>(mario).unwrap();
        assert_eq!(aggro.target, Some(far), "stood over the corpse");
        // And it is walking to the new one rather than to where the old one
        // fell: `at` is what the movement step reads, and a stale one is a
        // Mario jogging to an empty patch of lawn.
        assert_eq!(aggro.at, Vec3::new(6., 0., 0.));
    }

    /// The cheap tier loses a dead target too, which is the one thing `alert`
    /// still does for it.
    ///
    /// `crowd_step` only ever fills an *empty* slot and cannot ask whether what
    /// is in it still exists, so without this a crowd-tier enemy whose Mario
    /// was killed held a despawned entity for the rest of the level: it never
    /// noticed anything again, and it answered `rouse_crowd` as something that
    /// has seen the player, which pins the flow field's alarm on for the whole
    /// crowd.
    #[test]
    fn a_crowd_tier_enemy_lets_go_of_a_target_that_has_died() {
        let mut world = World::new();
        world.init_resource::<Threats>();
        world.insert_resource(GameTuning::default());
        let mario = world
            .spawn((
                Ally::new(Vec3::ZERO, 0.0),
                Side::Friendly,
                Aggro::default(),
                Transform::default(),
            ))
            .id();
        let slime = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Detail::Crowd,
                Aggro {
                    target: Some(mario),
                    ..Aggro::default()
                },
                Transform::from_xyz(3., 0., 0.),
            ))
            .id();
        // Still there, so nothing is taken off it: aggro is not a leash, and a
        // crowd enemy is not re-pointed by this system.
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(world.get::<Aggro>(slime).unwrap().target, Some(mario));
        world.despawn(mario);
        world.run_system_once(alert).expect("alert could not run");
        assert_eq!(
            world.get::<Aggro>(slime).unwrap().target,
            None,
            "chasing a corpse"
        );
    }

    /// A step at `height`: ground to the west of the origin, a face rising at
    /// x = 0, and more ground on top of it to the east. A kerb and a cliff are
    /// the same shape at two heights, which is the point.
    fn ledge(height: f32) -> LevelData {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let base = vertices.len() as u32;
            vertices.extend([a, b, c, d]);
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        };
        quad(
            Vec3::new(-9., 0., -9.),
            Vec3::new(0., 0., -9.),
            Vec3::new(0., 0., 9.),
            Vec3::new(-9., 0., 9.),
        );
        quad(
            Vec3::new(0., 0., -9.),
            Vec3::new(0., height, -9.),
            Vec3::new(0., height, 9.),
            Vec3::new(0., 0., 9.),
        );
        quad(
            Vec3::new(0., height, -9.),
            Vec3::new(9., height, -9.),
            Vec3::new(9., height, 9.),
            Vec3::new(0., height, 9.),
        );
        LevelData::new(vertices, triangles, Vec::new())
    }

    /// Walks a slime east for a while and reports where it got to.
    fn trudge(level: &LevelData, from: Vec3, ticks: usize) -> Vec3 {
        let mut at = from;
        for _ in 0..ticks {
            at = walk(level, at, Vec3::X * 0.06, Kind::Slime.body().0);
            at = settle(level, at, FIXED_DT);
        }
        at
    }

    /// The bug this was written for. A slime at the bottom of a cliff stays at
    /// the bottom of the cliff: the top is not a step, however near it is in a
    /// straight line, and being handed it by the floor query is not the same as
    /// having climbed it.
    #[test]
    fn a_walker_does_not_arrive_on_top_of_a_cliff() {
        let cliff = ledge(2.5);
        let at = trudge(&cliff, Vec3::new(-3., 0., 0.), 200);
        assert!(
            at.y < 0.01,
            "it got up a two-and-a-half unit cliff, to {at:?}"
        );
        assert!(at.x < 0.01, "it walked into the cliff, to {at:?}");
        // It did set off, rather than being stuck from the first tick.
        assert!(at.x > -3.0 + 0.5, "it never walked anywhere: {at:?}");
    }

    /// And it is not stopped by everything: a kerb it could step onto it steps
    /// onto, and a slope it could walk up it walks up.
    #[test]
    fn a_walker_steps_up_what_it_can() {
        let kerb = ledge(0.35);
        let at = trudge(&kerb, Vec3::new(-3., 0., 0.), 200);
        assert!(
            (at.y - 0.35).abs() < 0.01 && at.x > 1.0,
            "it was stopped by a kerb, ending at {at:?}"
        );
    }

    /// Ground is followed at a walking pace rather than assigned, which is the
    /// other half of the same complaint: the floor query answers a column, a
    /// column's answer jumps, and taking it whole is a walker that morphs
    /// between elevations instead of walking between them.
    #[test]
    fn a_walker_climbs_and_falls_at_a_pace() {
        let kerb = ledge(0.35);
        // Stood at the foot of the step, in the column its top occupies.
        let climbing = settle(&kerb, Vec3::new(0.5, 0., 0.), FIXED_DT);
        assert!(
            (climbing.y - CLIMB_SPEED * FIXED_DT).abs() < 1e-6,
            "it climbed {} in one tick",
            climbing.y
        );
        // Dropped from above it, it comes down faster than it goes up, and it
        // settles exactly rather than shivering around the ground.
        let mut at = Vec3::new(-3., 6., 0.);
        let first = settle(&kerb, at, FIXED_DT);
        assert!((first.y - (6.0 - FALL_SPEED * FIXED_DT)).abs() < 1e-6);
        for _ in 0..300 {
            at = settle(&kerb, at, FIXED_DT);
        }
        assert_eq!(at.y, 0.0);
    }

    /// No two enemies walk quite alike: they keep different paces, make for
    /// different spots around what they are chasing, and weave differently on
    /// the way. A brood that shares one number for all of it marches.
    #[test]
    fn no_two_enemies_walk_quite_alike() {
        let (first, second) = (Quirk::new(1.0), Quirk::new(2.0));
        assert!((first.pace() - second.pace()).abs() > 0.01);
        assert!(first.stand_off(0.0).distance(second.stand_off(0.0)) > 0.1);
        assert!((first.weave(3.0) - second.weave(3.0)).abs() > 0.01);
        // Within bounds, both of them: a quirk is a difference, not a licence.
        for quirk in [&first, &second] {
            assert!((quirk.pace() - 1.0).abs() <= PACE_SPREAD + 1e-6);
            assert!(quirk.stand_off(0.0).length() <= STAND_OFF + STAND_OFF_SPREAD + 1e-6);
            assert!(quirk.weave(7.0).abs() <= WEAVE_WIDTH + 1e-6);
        }
    }

    /// Where the player's feet are when he is coming down on top of one.
    ///
    /// A fraction of the enemy's own height rather than a number, because the
    /// enemy these are measured against has changed once already: 0.8 sat
    /// comfortably inside the goomba's metre and is a clean miss over the
    /// slime's 0.70, so the stomp tests passed for a while and then quietly
    /// stopped testing stomping.
    fn on_its_head() -> f32 {
        Kind::Slime.body().1 * 0.8
    }

    /// A player, and one enemy standing on his toes.
    fn world(player_y: f32, velocity: Vec3) -> (World, Entity) {
        let mut world = World::new();
        world.init_resource::<Threats>();
        // The one die in the game. Every path that kills something rolls it --
        // see `nuclonium::Drops::maybe` -- so a world that means to resolve a fight
        // has to have it, exactly as it has to have the threat queue.
        world.init_resource::<crate::nuclonium::Drops>();
        world.insert_resource(SoundQueue::default());
        let mut controller = Controller::default();
        controller.velocity = velocity;
        world.spawn((
            Player,
            Transform::from_xyz(0.0, player_y, 0.0),
            controller,
            Health::new(health::PLAYER_HEALTH),
        ));
        let enemy = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                // Given hit points here rather than left off, so these tests
                // run the same path the game does: `health::strike` treats a
                // target without them as one that dies to anything, which would
                // make the stomp test pass for the wrong reason.
                Health::new(Kind::Slime.health()),
                Transform::from_xyz(0.5, 0.0, 0.0),
            ))
            .id();
        (world, enemy)
    }

    /// Velocity, health and immunity: everything these tests read.
    fn controller(world: &mut World) -> (Vec3, i32, f32) {
        let mut query = world.query_filtered::<(&Controller, &Health), With<Player>>();
        let (ctrl, health) = query.single(world).unwrap();
        (ctrl.velocity, health.current, ctrl.invulnerable_left)
    }

    /// What the player is left with after one touch from a slime.
    const AFTER_A_SLIME: i32 = health::PLAYER_HEALTH - health::SLIME_DAMAGE;

    /// Walking into one costs its damage and throws the player clear.
    #[test]
    fn touching_an_enemy_hurts_and_knocks_the_player_back() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok(), "the enemy died on contact");
        let (velocity, health, immune) = controller(&mut world);
        assert_eq!(health, AFTER_A_SLIME, "a slime is worth two points");
        assert!(velocity.x < 0.0, "not thrown away from it: {velocity:?}");
        assert!(velocity.y > 0.0);
        assert_eq!(immune, INVULNERABLE_SECONDS);
    }

    /// A swing spends the player's damage against the pool rather than
    /// despawning what it reaches.
    ///
    /// Everything the game actually ships dies to one of his blows -- ten
    /// against an ant's ten is the toughest trade on the field -- so the only
    /// way to see the arithmetic happen at all is against something tougher
    /// than anything placed. That is what this builds: it is a test of the
    /// mechanism, and the numbers it is normally run with are the two tables in
    /// [`crate::health`].
    #[test]
    fn a_swing_spends_damage_rather_than_deleting_what_it_hits() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        // Three of his swings' worth, and mid-swing so the blow lands.
        let tough = health::PLAYER_DAMAGE * 3;
        world.entity_mut(enemy).insert(Health::new(tough));
        world
            .query_filtered::<&mut Controller, With<Player>>()
            .single_mut(&mut world)
            .unwrap()
            .attack_left = 0.2;
        world.run_system_once(combat).expect("combat could not run");
        assert!(
            world.get_entity(enemy).is_ok(),
            "one swing emptied a pool three swings deep"
        );
        assert_eq!(
            world.get::<Health>(enemy).unwrap().current,
            tough - health::PLAYER_DAMAGE,
            "the swing landed for the wrong number"
        );
        // And the last of the three finishes it.
        for _ in 0..2 {
            world
                .query_filtered::<&mut Controller, With<Player>>()
                .single_mut(&mut world)
                .unwrap()
                .attack_left = 0.2;
            world.run_system_once(combat).expect("combat could not run");
        }
        assert!(world.get_entity(enemy).is_err(), "it survived three swings");
    }

    /// The reported bug, at its root. A player thrown into the air by a hit
    /// comes back down on the enemy that hit him, and the descent alone would
    /// stomp it -- so an enemy that touches somebody standing perfectly still
    /// destroys itself, and a warp pipe looks like it spawns nothing.
    #[test]
    fn an_enemy_is_not_stomped_by_the_fall_from_its_own_hit() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        // Now airborne from the knockback, coming down on its head.
        {
            let mut query = world.query_filtered::<&mut Transform, With<Player>>();
            query.single_mut(&mut world).unwrap().translation.y = on_its_head();
            let mut query = world.query_filtered::<&mut Controller, With<Player>>();
            query.single_mut(&mut world).unwrap().velocity = Vec3::new(0.0, -6.0, 0.0);
        }
        for _ in 0..20 {
            world.run_system_once(combat).expect("combat could not run");
        }
        assert!(
            world.get_entity(enemy).is_ok(),
            "the enemy stomped itself on a player who did nothing"
        );
        assert_eq!(
            controller(&mut world).1,
            AFTER_A_SLIME,
            "hit again while immune"
        );
    }

    /// Coming down on one deliberately still defeats it. Without this the
    /// test above would pass just as well if stomping had been removed.
    #[test]
    fn landing_on_an_enemy_defeats_it() {
        let (mut world, enemy) = world(on_its_head(), Vec3::new(0.0, -6.0, 0.0));
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_err(), "the stomp did nothing");
        let (velocity, health, _) = controller(&mut world);
        assert_eq!(velocity.y, BOUNCE_VELOCITY);
        assert_eq!(
            health,
            health::PLAYER_HEALTH,
            "a stomp is not supposed to hurt"
        );
    }

    /// An ant hanging from a ceiling reaches down out of it, and the
    /// player walking underneath is walking into the bug rather than under it.
    /// Measured upright -- from its feet upwards -- it would be a storey away.
    #[test]
    fn an_enemy_hanging_upside_down_reaches_downwards() {
        let (mut world, enemy) = world(0.0, Vec3::ZERO);
        {
            let mut query = world.query_filtered::<&mut Transform, Without<Player>>();
            let mut transform = query.single_mut(&mut world).unwrap();
            transform.translation = Vec3::new(0.5, 2.2, 0.0);
            transform.rotation = orientation(Vec3::NEG_Y, Vec3::X).unwrap();
        }
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok(), "it was somehow stomped");
        assert_eq!(
            controller(&mut world).1,
            AFTER_A_SLIME,
            "hung out of reach of the player"
        );
    }

    /// Standing on a roof directly above one is not touching it.
    #[test]
    fn an_enemy_a_storey_below_is_out_of_reach() {
        let (mut world, enemy) = world(4.0, Vec3::ZERO);
        world.run_system_once(combat).expect("combat could not run");
        assert!(world.get_entity(enemy).is_ok());
        assert_eq!(
            controller(&mut world).1,
            health::PLAYER_HEALTH,
            "hurt from a storey up"
        );
    }

    /// An ant that has been under the world is stood back up by the
    /// crowd tier, and that is the only thing in the game that will do it.
    ///
    /// The state being fixed is a real one and reachable in a couple of
    /// seconds' play: walk a bug up a wall and onto a ceiling and its `up` is
    /// now pointing down, which is correct while it is hanging there. Move it
    /// without telling it -- the crowd tier planting it on the ground is the
    /// easy way, a knockback is another -- and it is under a floor rather than
    /// on a ceiling, with no way to tell the difference and no way back. It
    /// clings to the underside of the lawn from then on.
    #[test]
    fn a_bug_left_upside_down_is_stood_back_up_by_the_crowd() {
        let level = room();
        // Walk it up the wall and onto the ceiling, so the upside-down `up`
        // is one the game itself produced rather than one written by hand.
        let mut position = Vec3::new(-3., 0., 0.);
        let mut crawler = Crawler::default();
        for _ in 0..200 {
            position = crawl_towards(
                &level,
                position,
                &mut crawler,
                Vec3::new(10., 4., 0.),
                3.0,
                FIXED_DT,
                0.0,
            );
        }
        assert!(
            crawler.up.y < -0.99,
            "it never made it onto the ceiling: up {:?}",
            crawler.up
        );

        // Now put it where the crowd tier would: on the floor, upside down.
        // `update` walks the field in parallel, so it wants a pool to walk it in.
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut world = World::new();
        let (level, _) = crate::level::load();
        world.insert_resource(GameTuning::default());
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(Time::<Fixed>::default());
        let ground = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(ground)));
        let bug = world
            .spawn((
                Enemy {
                    kind: Kind::Ant,
                    animation: Handle::default(),
                },
                Transform::from_translation(ground + Vec3::X * 2.0),
                Visibility::Visible,
                Side::Hostile,
                Aggro::default(),
                Quirk::new(0.0),
                Wander::new(ground, 0.0),
                crawler,
                Detail::Crowd,
            ))
            .id();
        // Several ticks, because righting itself is a roll like any other and
        // [`ROLL_RATE`] gives it about six of them to turn all the way over.
        let step = world.register_system(update);
        let mut ticks = 0;
        while world.get::<Crawler>(bug).expect("no crawler").up != Vec3::Y {
            world.run_system(step).expect("no run");
            ticks += 1;
            assert!(ticks < 30, "it is still upside down after {ticks} ticks");
        }
        // And it did roll rather than snap: a flip that took one tick would be
        // the instant swap this is here to prevent.
        assert!(ticks > 2, "it snapped upright in {ticks} tick(s)");
    }

    /// A field of crowd-tier slimes turned loose on the castle never walks
    /// through anything.
    ///
    /// The unit tests in [`crate::flow`] check the rule; this checks that the
    /// tier actually asks it, over the real collision mesh, with the weave and
    /// the wander and the promotions and everything else that moves an enemy
    /// somewhere the flow field did not send it. Every step of every enemy is
    /// cast against the level at knee height -- the near tier's own test for
    /// what it may walk through -- and none of them may hit a wall.
    ///
    /// Before the survey learned about walls this reported hundreds of
    /// crossings over a thirty-second walk.
    #[test]
    fn a_crowd_walks_the_castle_without_walking_through_it() {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let (level, _) = crate::level::load();
        let mut world = World::new();
        world.insert_resource(GameTuning {
            // Everything is crowd, which is the tier under test.
            sim_budget: 0.0,
            ..GameTuning::default()
        });
        world.insert_resource(crate::flow::FlowField::new(&level));
        let spots = crowd_spots(96, &level);
        world.insert_resource(level);
        world.insert_resource(Time::<Fixed>::default());
        world.spawn((Player, Transform::from_xyz(-13.28, 3.0, 46.64)));
        let enemies: Vec<Entity> = spots
            .iter()
            .enumerate()
            .map(|(index, at)| {
                world
                    .spawn((
                        Enemy {
                            kind: Kind::Slime,
                            animation: Handle::default(),
                        },
                        Transform::from_translation(*at),
                        Visibility::Visible,
                        Side::Hostile,
                        Aggro::default(),
                        Quirk::new(index as f32 * crate::squad::GOLDEN_ANGLE),
                        Wander::new(*at, index as f32),
                        Detail::Crowd,
                    ))
                    .id()
            })
            .collect();
        // Registered rather than run one-shot, so the tick counter the strides
        // are taken off actually counts.
        let sweep = world.register_system(crate::flow::rebuild);
        let step = world.register_system(update);
        world.insert_resource(Time::<()>::default());

        let mut was: Vec<Vec3> = spots.clone();
        let knee = Vec3::Y * STEP_UP;
        let mut through = Vec::new();
        for _ in 0..900 {
            world.run_system(sweep).expect("no sweep");
            world.run_system(step).expect("no step");
            for (index, enemy) in enemies.iter().enumerate() {
                let now = world.get::<Transform>(*enemy).expect("gone").translation;
                let level = world.resource::<LevelData>();
                // The horizontal part only, cast from knee height: exactly the
                // probe [`walk`] makes, so the two tiers are held to one
                // standard. The vertical part is [`settle`] falling, and a
                // segment drawn down a cliff face hits the cliff whether or not
                // anything walked through it.
                let moved = Vec3::new(now.x - was[index].x, 0.0, now.z - was[index].z);
                if moved.length_squared() > 1e-8 {
                    let from = was[index] + knee;
                    let crossed = level
                        .surface_hit(from, from + moved)
                        .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y);
                    if crossed {
                        let field = world.resource::<crate::flow::FlowField>();
                        through.push((was[index], now, field.same_cell(was[index], now)));
                    }
                }
                was[index] = now;
            }
        }
        // It has to have gone somewhere, or a field that never moves passes.
        let travelled: f32 = enemies
            .iter()
            .zip(&spots)
            .map(|(enemy, from)| {
                world
                    .get::<Transform>(*enemy)
                    .expect("gone")
                    .translation
                    .distance(*from)
            })
            .sum();
        // Four metres a head rather than the five it was. The number moved when
        // the tier started probing a body's width ahead of its feet instead of
        // walking the grid as a point: a crowd that stops at fences and at the
        // foot of the castle covers slightly less ground than one that walks
        // through both, and on this field that is about five percent of it.
        // What the assertion is for is unchanged -- a field that never moves
        // must not pass -- and 96 enemies covering 460 m is not that.
        assert!(
            travelled > 96.0 * 4.0,
            "the crowd barely moved: {travelled:.0} m between all of them"
        );
        // Nothing crossed a wall on its way out of a cell, which is the whole of
        // what a grid of this size can promise. What is left is enemies moving
        // *within* one cell that has a fence running through it: the survey
        // records what stands between two cell centres and has no way to record
        // anything finer, so an enemy can clip a fence corner by less than the
        // 1.7 m the grid is drawn at. It is also the tier that is never within
        // twenty-five metres of the camera -- anything nearer is walking on
        // [`walk`], which casts for itself.
        let escaped: Vec<_> = through
            .iter()
            .filter(|(_, _, same_cell)| !same_cell)
            .collect();
        assert!(
            escaped.is_empty(),
            "{} steps crossed a wall into another cell, e.g. {:?}",
            escaped.len(),
            &escaped[..escaped.len().min(3)]
        );

        // And nobody walked up the castle. The measure is bluntly physical --
        // how much height an enemy gained in thirty seconds -- because that is
        // what the report was: slimes going up and down elevations that are
        // impossibly steep. With only `walkable` checked, the worst of a field
        // of ninety-six climbed **30.3 m**, straight up the castle wall at
        // [`CLIMB_SPEED`], with four more past 9 m. Refusing the cliff edges
        // leaves 8 m, over ramps and lawn that genuinely rise that far.
        let worst = enemies
            .iter()
            .zip(&spots)
            .map(|(enemy, from)| {
                world.get::<Transform>(*enemy).expect("gone").translation.y - from.y
            })
            .fold(f32::MIN, f32::max);
        assert!(
            worst < 10.0,
            "something climbed {worst:.1} m in thirty seconds"
        );
    }

    /// The Marios were held out of nothing at all: `move_allies` walks each one
    /// to its slot and asks nothing about what is standing there. A squad
    /// following the player was a heap of Marios in one place.
    #[test]
    fn the_marios_are_held_out_of_each_other_and_out_of_the_player() {
        let mut world = lawn_world();
        let at = Vec3::new(0.0, 0.0, 0.0);
        world.spawn((Player, Transform::from_translation(at)));
        // All four in the same spot, on top of him.
        let marios: Vec<Entity> = (0..4)
            .map(|_| {
                world
                    .spawn((
                        crate::squad::Ally::new(at, 0.0),
                        Transform::from_translation(at),
                    ))
                    .id()
            })
            .collect();
        for _ in 0..120 {
            world.run_system_once(spread).expect("no run");
        }
        let room = crate::player::PLAYER_RADIUS * 2.0 + PERSONAL_SPACE - SPREAD_SLACK;
        let places: Vec<Vec3> = marios
            .iter()
            .map(|mario| world.get::<Transform>(*mario).expect("gone").translation)
            .collect();
        for (index, mine) in places.iter().enumerate() {
            assert!(
                mine.distance(at) >= room * 0.99,
                "a Mario is standing inside the player, {:.2} m away",
                mine.distance(at)
            );
            for theirs in &places[index + 1..] {
                assert!(
                    mine.distance(*theirs) >= room * 0.99,
                    "two Marios {:.2} m apart, which is inside each other",
                    mine.distance(*theirs)
                );
            }
        }
        // And the player was not shoved out of the way by the crowd he is
        // standing in: he is driven by the controller, not by the press.
        let mut players = world.query_filtered::<&Transform, With<Player>>();
        assert_eq!(players.single(&world).expect("no player").translation, at);
    }

    /// An enemy that walks out past `enemy_draw` and back wears exactly one
    /// skeleton, not two.
    ///
    /// Removing a `WorldAssetRoot` does not despawn the scene it spawned --
    /// its `on_remove` hook only unregisters the instance -- so [`shed_scenes`]
    /// has to take the children itself. When it did not, every round trip past
    /// the swap distance stacked another model on the enemy, and the field read
    /// as being doubled up.
    ///
    /// The child stands in for a spawned scene rather than being one: what is
    /// under test is that shedding empties the enemy out, and a real glTF here
    /// would only add a loader to the test.
    #[test]
    fn shedding_a_model_takes_its_children_with_it() {
        let mut app = App::new();
        app.add_plugins(AssetPlugin::default());
        let world = app.world_mut();
        let enemy = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Visibility::Hidden,
                WorldAssetRoot(Handle::default()),
            ))
            .id();
        world.spawn(ChildOf(enemy));
        world
            .run_system_once(shed_scenes)
            .expect("shed_scenes could not run");
        assert!(
            world.get::<WorldAssetRoot>(enemy).is_none(),
            "the far enemy kept its model"
        );
        assert!(
            world
                .get::<Children>(enemy)
                .is_none_or(|kids| kids.is_empty()),
            "the far enemy kept the skeleton the model spawned, so the next one \
             it is given will be its second"
        );
    }

    /// A lawn with a wall across the middle of it and one gate in the wall.
    ///
    /// The wall is a plain vertical quad, which a drop from the sky goes
    /// straight past -- so both sides survey as walkable ground at nought and
    /// what stops anything crossing is the fence check, a horizontal cast
    /// between neighbouring cell centres. That is the same thing the castle's
    /// railings are to the grid, which is the case this stands in for.
    fn a_walled_lawn() -> LevelData {
        let points = vec![
            Vec3::new(-30., 0., -30.),
            Vec3::new(30., 0., -30.),
            Vec3::new(30., 0., 30.),
            Vec3::new(-30., 0., 30.),
            // The left half of the wall, from the western edge to the gate.
            Vec3::new(-30., 0., 0.),
            Vec3::new(-3., 0., 0.),
            Vec3::new(-3., 5., 0.),
            Vec3::new(-30., 5., 0.),
            // And the right half, from the gate to the eastern edge.
            Vec3::new(3., 0., 0.),
            Vec3::new(30., 0., 0.),
            Vec3::new(30., 5., 0.),
            Vec3::new(3., 5., 0.),
        ];
        LevelData::new(
            points,
            vec![
                [0, 1, 2],
                [0, 2, 3],
                [4, 5, 6],
                [4, 6, 7],
                [8, 9, 10],
                [8, 10, 11],
            ],
            Vec::new(),
        )
    }

    /// One near-tier slime, chasing something on the far side of that wall.
    ///
    /// `routed` is the whole experiment: with it, [`navigate`] and
    /// [`crate::path::plan`] run in the tick and the slime is told the way;
    /// without it, nothing else about the world differs and it does what every
    /// enemy in this game did before -- walks straight at what it noticed.
    /// Answers the furthest it ever got across the wall.
    fn chase_across_the_wall(routed: bool) -> f32 {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut world = World::new();
        let level = a_walled_lawn();
        world.insert_resource(GameTuning {
            // So the walk is a few hundred ticks rather than a few thousand.
            // Nothing being tested here is about how fast it gets there.
            enemy_speed: 6.0,
            ..GameTuning::default()
        });
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(Time::<Fixed>::default());
        world.init_resource::<crate::path::Pathing>();
        let start = Vec3::new(-14.0, 0.0, -8.0);
        let quarry = Vec3::new(-14.0, 0.0, 8.0);
        // The player stands next to the slime rather than being what it is
        // after, because the walk step measures how much simulation to give a
        // body against how far off *he* is. What it is chasing is `Aggro::at`,
        // which is across the wall -- and a chase is never given up on, so
        // nothing has to keep refreshing it.
        let luna = world
            .spawn((Player, Transform::from_translation(start + Vec3::X * 2.0)))
            .id();
        let slime = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Transform::from_translation(start),
                Visibility::Visible,
                Side::Hostile,
                Aggro {
                    target: Some(luna),
                    at: quarry,
                    ..Aggro::default()
                },
                Quirk::new(0.0),
                Wander::new(start, 0.0),
                Detail::Full,
            ))
            .id();
        let tell = world.register_system(navigate);
        let route = world.register_system(crate::path::plan);
        let walk = world.register_system(update);
        let mut furthest = f32::NEG_INFINITY;
        for _ in 0..600 {
            if routed {
                world.run_system(tell).expect("navigate could not run");
                world.run_system(route).expect("path::plan could not run");
            }
            world.run_system(walk).expect("update could not run");
            furthest = furthest.max(world.get::<Transform>(slime).unwrap().translation.z);
        }
        furthest
    }

    /// **What "enemies need the A* too" is asking for.** The far crowd has had
    /// global navigation all along -- it walks down the flow field's own
    /// gradient, which is a flood over the whole grid -- and the near tier, the
    /// enemies actually on screen, had a stride and a half of local steering
    /// and nothing else. So the ones the player can watch were the ones that
    /// pressed into the wall in front of them for the rest of the session, and
    /// the flood cannot help them: it is flooded from the player, and this
    /// slime is after something else.
    #[test]
    fn a_chasing_enemy_takes_the_gate_rather_than_the_wall_in_front_of_it() {
        let told = chase_across_the_wall(true);
        assert!(
            told > 1.0,
            "it never got through the gate: furthest z {told}"
        );
        // And the same slime, in the same world, with nothing to go on but
        // where the thing it is chasing is. It is a hundred ticks of walking
        // from the gate and never finds it.
        let alone = chase_across_the_wall(false);
        assert!(
            alone < 0.0,
            "it crossed a wall nobody told it the way round: furthest z {alone}"
        );
    }

    /// **Ten enemies going the same way are one search between them, and that
    /// is what makes routing the near tier affordable at all.** A brood comes
    /// out of one pipe at one Mario: same place, same destination, and by any
    /// reasonable reading the same walk. [`crate::path::plan`] buckets on a
    /// block of cells at each end and hands the bucket one route, so the cost
    /// of a crowd chasing something is the cost of one body chasing it.
    #[test]
    fn a_brood_going_the_same_way_is_one_search_between_them() {
        let mut world = World::new();
        let level = a_walled_lawn();
        world.insert_resource(GameTuning::default());
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.init_resource::<crate::path::Pathing>();
        let start = Vec3::new(-14.0, 0.0, -8.0);
        let quarry = Vec3::new(-14.0, 0.0, 8.0);
        let luna = world
            .spawn((Player, Transform::from_translation(start)))
            .id();
        for index in 0..10 {
            let offset = crate::squad::slot(index, 0.25);
            world.spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Transform::from_translation(start + Vec3::new(offset.x, 0.0, offset.y)),
                Side::Hostile,
                Aggro {
                    target: Some(luna),
                    at: quarry,
                    ..Aggro::default()
                },
                // The same phase for all ten, so they want the same spot as
                // well as coming from the same one. A brood with ten different
                // standoff angles is ten destinations two metres apart, which
                // is a fair test of the bucketing and a poor one of the claim.
                Quirk::new(0.0),
                Wander::new(start, 0.0),
                Detail::Full,
            ));
        }
        world
            .run_system_once(navigate)
            .expect("navigate could not run");
        world
            .run_system_once(crate::path::plan)
            .expect("path::plan could not run");
        let stats = world.resource::<crate::path::Pathing>();
        assert_eq!(stats.queued, 10, "not every slime asked the way: {stats:?}");
        assert_eq!(stats.direct, 0, "the wall was not in the way: {stats:?}");
        // **Two rather than one, and the slack is honest rather than
        // defensive.** The bucket is a block of cells and a huddle can sit
        // across the line between two of them, so a brood is one journey or it
        // is two depending on where the grid happens to fall -- which is a
        // property of a fixed grid and not something worth aligning a test to.
        // What is being claimed is that the cost of a crowd going one way is a
        // small constant, and the number that would fail it is ten.
        assert!(
            stats.groups <= 2,
            "one journey came out as several: {stats:?}"
        );
        assert!(
            stats.searched <= 2,
            "ten bodies cost a search each: {stats:?}"
        );
    }

    /// The far crowd is not offered a route, and that is the other half of the
    /// affordability argument.
    ///
    /// It is not going without: it has the flood, which answers "which way is
    /// the player" for every cell of the grid at one array read and comes round
    /// the moat and through the gate on its own. What it must not do is join
    /// the search queue -- a thousand bodies the size of a thumbnail would
    /// spend the whole of `path_budget` and leave none of it for the enemies
    /// the player can actually see.
    #[test]
    fn the_far_crowd_asks_for_no_routes_at_all() {
        let mut world = World::new();
        let level = a_walled_lawn();
        world.insert_resource(GameTuning::default());
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.init_resource::<crate::path::Pathing>();
        let start = Vec3::new(-14.0, 0.0, -8.0);
        let luna = world
            .spawn((Player, Transform::from_translation(start)))
            .id();
        let chasing = |detail: Detail| {
            (
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Transform::from_translation(start),
                Side::Hostile,
                Aggro {
                    target: Some(luna),
                    at: Vec3::new(-14.0, 0.0, 8.0),
                    ..Aggro::default()
                },
                Quirk::new(0.0),
                Wander::new(start, 0.0),
                detail,
            )
        };
        let far = world.spawn(chasing(Detail::Crowd)).id();
        world
            .run_system_once(navigate)
            .expect("navigate could not run");
        world
            .run_system_once(crate::path::plan)
            .expect("path::plan could not run");
        assert_eq!(
            world.resource::<crate::path::Pathing>().queued,
            0,
            "the far crowd is in the search queue"
        );
        // And the tier really is what decided that: the same slime, promoted,
        // asks the way like anything else. Without this the test passes just as
        // well against an enemy that never routes at all.
        *world.get_mut::<Detail>(far).unwrap() = Detail::Full;
        world
            .run_system_once(navigate)
            .expect("navigate could not run");
        world
            .run_system_once(crate::path::plan)
            .expect("path::plan could not run");
        assert_eq!(
            world.resource::<crate::path::Pathing>().queued,
            1,
            "a near-tier enemy went unrouted"
        );
    }

    /// The rule the climb turns on, and the three things it must not fire for.
    ///
    /// Written against [`climbing`] rather than against a walk, because this is
    /// arithmetic over two vectors and the cases that matter are the ones a
    /// staged walk would never happen to produce. It is also what
    /// [`draw_crawlers`] colours by, so these are the four pictures the overlay
    /// can show.
    #[test]
    fn a_bug_climbs_only_what_is_between_it_and_where_it_is_going() {
        // On the lawn, with the Mario a stride away and a hair below it. The
        // sign of that hair is noise and must decide nothing.
        assert!(!climbing(Vec3::Y, Vec3::new(9.0, -0.2, 0.0)));
        assert!(!climbing(Vec3::Y, Vec3::new(9.0, 0.2, 0.0)));
        // On the near face of a wall, with the Mario on the far side of it and
        // on the ground. This is the case the whole thing exists for: what the
        // wall can hold of "over there and below me" is the below.
        assert!(climbing(Vec3::NEG_Z, Vec3::new(0.0, -2.5, 9.0)));
        // The far face of that same wall, on the way down. The goal is in
        // front of the surface now, so the rule lets go on its own rather than
        // needing to be told the climb is over.
        assert!(!climbing(Vec3::Z, Vec3::new(0.0, -4.0, 7.0)));
        // A steep hillside with the Mario at the foot of it. Its normal leans
        // out over its own foot, which is what keeps a bug walking down a hill
        // rather than climbing it -- and is why the steepness test alone would
        // not do.
        let slope = Vec3::new(0.0, 0.34, 0.94);
        assert!(!climbing(slope, Vec3::new(0.0, -3.0, 3.0)));
        // And a ceiling, which is not steep at all by this measure: a bug under
        // one walks along it towards what it is after, exactly as it did.
        assert!(!climbing(Vec3::NEG_Y, Vec3::new(9.0, -4.0, 0.0)));
    }

    /// The same lawn, with a solid slab across the whole width of it and no way
    /// round at either end. Five metres tall, two thick, with a top to walk on.
    ///
    /// A wall rather than the fence [`a_walled_lawn`] uses, because what is
    /// being watched here is a body going *over* one, and a zero-thickness quad
    /// has no over: no top face to reach, and both of its sides are the same
    /// triangle.
    fn a_lawn_cut_in_two_by_a_wall(gate: Option<(f32, f32)>) -> LevelData {
        let mut points = vec![
            Vec3::new(-30., 0., -30.),
            Vec3::new(30., 0., -30.),
            Vec3::new(30., 0., 30.),
            Vec3::new(-30., 0., 30.),
        ];
        let mut faces: Vec<[u32; 3]> = vec![[0, 1, 2], [0, 2, 3]];
        // One span of wall, or two with a hole between them.
        let spans = match gate {
            None => vec![(-30.0_f32, 30.0_f32)],
            Some((from, to)) => vec![(-30.0, from), (to, 30.0)],
        };
        for (from, to) in spans {
            let base = points.len() as u32;
            points.extend([
                // The near face, at z = -1, anticlockwise from its bottom left.
                Vec3::new(from, 0., -1.),
                Vec3::new(to, 0., -1.),
                Vec3::new(to, 5., -1.),
                Vec3::new(from, 5., -1.),
                // And the far face, at z = 1, the same way round.
                Vec3::new(from, 0., 1.),
                Vec3::new(to, 0., 1.),
                Vec3::new(to, 5., 1.),
                Vec3::new(from, 5., 1.),
            ]);
            // Winding is not worth getting right: `surface_hit` turns every
            // normal it answers with to face whatever asked, so a triangle here
            // is a solid surface from both sides however it is wound.
            let mut quad = |a: u32, b: u32, c: u32, d: u32| {
                faces.push([base + a, base + b, base + c]);
                faces.push([base + a, base + c, base + d]);
            };
            quad(0, 1, 2, 3);
            quad(4, 5, 6, 7);
            // And the top between them, which is what makes this a wall with an
            // over rather than a fence.
            quad(3, 2, 6, 7);
        }
        LevelData::new(points, faces, Vec::new())
    }

    /// One enemy, chasing something on the far side of that wall for twenty
    /// seconds. Answers how far across it ever got and how high it ever climbed.
    ///
    /// `crawling` is the only difference between the two runs, and it is the
    /// difference between the two kinds: a [`Crawler`] is what [`spawn`] gives
    /// an ant and withholds from a slime.
    fn chase_over_the_wall(crawling: bool, gate: Option<(f32, f32)>) -> (f32, f32) {
        bevy::tasks::ComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        let mut world = World::new();
        let level = a_lawn_cut_in_two_by_a_wall(gate);
        world.insert_resource(GameTuning {
            // So the climb is a couple of hundred ticks rather than a couple of
            // thousand. Nothing here is about how fast it gets over.
            enemy_speed: 6.0,
            ..GameTuning::default()
        });
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.insert_resource(Time::<Fixed>::default());
        world.init_resource::<crate::path::Pathing>();
        // Well away from wherever the gate is, when there is one.
        let start = Vec3::new(-20.0, 0.0, -8.0);
        let quarry = Vec3::new(-20.0, 0.0, 8.0);
        let luna = world
            .spawn((Player, Transform::from_translation(start + Vec3::X * 2.0)))
            .id();
        let kind = match crawling {
            true => Kind::Ant,
            false => Kind::Slime,
        };
        let body = world
            .spawn((
                Enemy {
                    kind,
                    animation: Handle::default(),
                },
                Transform::from_translation(start),
                Visibility::Visible,
                Side::Hostile,
                Aggro {
                    target: Some(luna),
                    at: quarry,
                    ..Aggro::default()
                },
                Quirk::new(0.0),
                Wander::new(start, 0.0),
                Detail::Full,
            ))
            .id();
        if crawling {
            world.entity_mut(body).insert(Crawler::default());
        }
        let tell = world.register_system(navigate);
        let route = world.register_system(crate::path::plan);
        let walk = world.register_system(update);
        let (mut across, mut high) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for _ in 0..600 {
            world.run_system(tell).expect("navigate could not run");
            world.run_system(route).expect("path::plan could not run");
            world.run_system(walk).expect("update could not run");
            let at = world.get::<Transform>(body).unwrap().translation;
            across = across.max(at.z);
            high = high.max(at.y);
        }
        (across, high)
    }

    /// **"Ants should be able to path along walls and ceilings."** A wall right
    /// across the lawn, and the Mario on the other side of it.
    ///
    /// Two things had to be true and neither was. The ant must not be handed a
    /// route, because [`crate::flow::FlowField`] is a floor and every answer it
    /// has is about going round -- see the note on [`navigate`]. And once it is
    /// on the wall it must keep climbing, which is the rule [`CLIMB_STEEP`]
    /// draws: before that, the way to the goal projected into a vertical face
    /// pointed *downwards*, so an ant climbed a metre, turned round inside a
    /// second, walked back down, and bobbed at the foot of the wall for ever.
    ///
    /// The slime is the control and the point of the geometry. There is no gate
    /// and no way round at either end, so nothing that keeps its feet on the
    /// floor can be on the far side however good its pathfinding is -- which
    /// makes "the ant got there" a claim about climbing rather than about the
    /// wall having a hole in it somewhere.
    #[test]
    fn an_ant_goes_over_a_wall_that_stops_a_slime_dead() {
        let (across, high) = chase_over_the_wall(true, None);
        assert!(
            high > 4.0,
            "the ant never got up the wall: highest y {high}"
        );
        assert!(
            across > 2.0,
            "the ant never got over the wall: furthest z {across}"
        );
        let (walked, climbed) = chase_over_the_wall(false, None);
        assert!(
            climbed < 1.0,
            "the slime went up a wall: highest y {climbed}"
        );
        assert!(
            walked < 0.0,
            "the slime got past a wall with no way round it: furthest z {walked}"
        );
    }

    /// The other half, and the one the pathfinding put back. Now the wall has a
    /// gate in it, twenty metres off to one side.
    ///
    /// A route exists, and it is a perfectly good one -- along the wall, through
    /// the gate, back along the far side. It is also not what an ant does, and
    /// handing it one is how a bug that used to walk over the courtyard wall
    /// started taking the long way round like everything else. So a [`Crawler`]
    /// is not routed at all: see the note on [`navigate`], which is where the
    /// rule lives and why.
    ///
    /// The claim is about the climb rather than the arrival, because both
    /// behaviours arrive. What tells them apart is whether the ant ever left the
    /// ground, and an ant that walked to the gate never does.
    #[test]
    fn an_ant_climbs_the_wall_in_front_of_it_rather_than_walking_to_the_gate() {
        let (across, high) = chase_over_the_wall(true, Some((14.0, 18.0)));
        assert!(
            high > 4.0,
            "the ant took the long way round to the gate: highest y {high}"
        );
        assert!(
            across > 2.0,
            "the ant never got to the far side at all: furthest z {across}"
        );
        // And the slime does use the gate, which is what makes the gate a real
        // way round rather than a hole this test happens not to find.
        let (walked, climbed) = chase_over_the_wall(false, Some((14.0, 18.0)));
        assert!(
            walked > 2.0,
            "the slime never found the gate: furthest z {walked}"
        );
        assert!(
            climbed < 1.0,
            "the slime went up a wall: highest y {climbed}"
        );
    }

    #[test]
    #[ignore]
    fn bench_navigate_and_plan() {
        use std::time::Instant;
        let mut world = World::new();
        let level = a_walled_lawn();
        world.insert_resource(GameTuning::default());
        world.insert_resource(crate::flow::FlowField::new(&level));
        world.insert_resource(level);
        world.init_resource::<crate::path::Pathing>();
        let luna = world
            .spawn((Player, Transform::from_translation(Vec3::ZERO)))
            .id();
        let total: usize = std::env::var("BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);
        let full: usize = std::env::var("BENCH_FULL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        for index in 0..total {
            let angle = index as f32 * 0.137;
            let at = Vec3::new(angle.cos() * 30.0, 0.0, angle.sin() * 30.0);
            let near = index < full;
            world.spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Transform::from_translation(at),
                Side::Hostile,
                Aggro {
                    target: near.then_some(luna),
                    at: Vec3::ZERO,
                    ..Aggro::default()
                },
                Quirk::new(index as f32),
                Wander::new(at, 0.0),
                if near { Detail::Full } else { Detail::Crowd },
            ));
        }
        let navigate_id = world.register_system(navigate);
        let plan_id = world.register_system(crate::path::plan);
        // Warm up: first tick plans, later ones ride the refresh.
        for _ in 0..5 {
            world.run_system(navigate_id).unwrap();
            world.run_system(plan_id).unwrap();
        }
        let ticks = 200;
        let mut nav = 0u128;
        let mut plan = 0u128;
        for _ in 0..ticks {
            let t = Instant::now();
            world.run_system(navigate_id).unwrap();
            nav += t.elapsed().as_nanos();
            let t = Instant::now();
            world.run_system(plan_id).unwrap();
            plan += t.elapsed().as_nanos();
        }
        let stats = format!("{:?}", world.resource::<crate::path::Pathing>());
        println!(
            "{total} enemies ({full} full): navigate {:.1} us/tick, plan {:.1} us/tick -- {stats}",
            nav as f64 / ticks as f64 / 1000.0,
            plan as f64 / ticks as f64 / 1000.0,
        );
    }
}
