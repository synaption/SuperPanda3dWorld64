//! Portals: a gate you plant, and the other gate that is the same gate.
//!
//! Two of them, blue and orange, put down the way a pylon is -- aim with the
//! crosshair, hold the action button, let go -- and each one stands on the lawn
//! as a free-standing hoop you can walk through. What makes them worth the
//! module is that they are not a doorway drawn on the ground: they are a
//! **place**, and four separate systems in this game have to agree that they
//! are:
//!
//!   * **You can see through one.** A second camera stands behind the far gate,
//!     at the eye's own position carried through the pair, and draws the world
//!     into an image the near gate's opening shows -- sampled by screen
//!     position, so the view inside the frame swings with your head the way a
//!     window does rather than sitting on the hoop like a poster. See
//!     [`aim_cameras`] and `portal.wgsl`.
//!   * **You can walk through one.** [`transit`] carries a body, its bearing
//!     and its speed through to the far side on the tick it crosses the plane.
//!   * **A crowd knows it is there.** [`crate::flow::FlowField::set_warp`]
//!     hangs one extra edge on the navigation grid, joining the cell in front
//!     of one gate to the cell in front of the other, and every question the
//!     grid answers is asked over that edge as well: the sweep that tells two
//!     thousand enemies which way you are, the A* a Mario runs to reach a ball,
//!     and the taut route that comes out of it. A gate is a shortcut across the
//!     castle and the crowd takes it.
//!   * **Power crosses one.** A pylon beam is light, and light goes through a
//!     gate: [`crate::pylon::links`] joins two masts that cannot see each other
//!     but can each see a mouth, and the supply packet flies the dog-leg.
//!
//! **None of those four is optional.** A gate you can see through but which the
//! crowd walks around is scenery with a good shader on it; one the crowd uses
//! but which the beams ignore is a rule the player has to learn twice. The pair
//! is one fact -- [`Portals`] -- and every system reads it from there.
//!
//! # What a gate is, geometrically
//!
//! A [`Mouth`] is a point, and a turn whose local **+Z points out of its face**,
//! +Y up it and +X across it. Everything else here is that convention applied
//! twice: the ellipse is the unit rectangle in the mouth's own XY plane, the
//! frame is a ring in the same plane, and the map from one side of the pair to
//! the other is [`Mouth::through`] -- the exit's frame, a half turn about its
//! up axis, and the entry's frame undone. The half turn is the part worth
//! saying out loud: without it a body walking *into* the front of one gate
//! comes out walking into the *back* of the other, which is a gate that spits
//! you backwards.
//!
//! A planted gate is always upright and its face is a yaw, which is what makes
//! it a thing you walk through rather than a thing you look at: the opening
//! stands on the ground it was planted on, so its lower half is at chest height
//! on everything in this game. [`Mouth::standing`] is the constructor the
//! placement uses; [`Mouth::built`] is the general one underneath it, and the
//! arithmetic never assumed upright.
//!
//! # A gate has a front
//!
//! One side of the hoop is a window and the other is an empty ring you can see
//! the lawn through, and only the window side is a door. That is not a
//! shortcut: a gate that worked from behind would need a second camera per gate
//! -- four full passes over the world per frame rather than two -- to have
//! anything to show from that side, and the rule "what you can see through is
//! what you can walk through" is the one a player can learn by looking.
//!
//! It costs nothing in practice, because the pair is still two-way: each gate
//! is entered from its own face and left from the other's, so blue takes you to
//! orange and orange takes you back. A gate is turned to face whoever planted
//! it, so the side you are standing on when you let go of the button is the
//! side that works.

use bevy::{
    asset::embedded_asset,
    camera::{
        visibility::RenderLayers, ImageRenderTarget, PerspectiveProjection, Projection,
        RenderTarget,
    },
    core_pipeline::tonemapping::Tonemapping,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{AsBindGroup, ShaderType, TextureFormat},
    shader::ShaderRef,
};

use crate::{
    aim,
    console::ConsoleState,
    display::SceneTarget,
    flow::FlowField,
    input::InputState,
    level::LevelData,
    player::{Controller, Player},
    squad,
    stellarator::{self, Stellarator},
};

const SHADER: &str = "embedded://space_crusaders/portal.wgsl";

// -- the shape of a portal --------------------------------------------------

/// Half the opening's width and height, in metres.
///
/// **Three metres across and four and a half tall, which is a city gate rather
/// than a door, and the size is the camera's rather than the player's.** A body
/// needs about a metre; this game is played over the shoulder from a boom
/// several metres long, and an opening a body fits through is one the *camera*
/// is left outside of -- so the moment you step through, the view has to jump
/// from one side to the other because there is no way for it to follow you.
/// Wide enough for the boom to come through with you and the walk-through is
/// continuous: see [`crate::camera::update`], which shortens the boom to fit
/// what is left, and [`carry_camera`], which flies it through.
///
/// The head is a semicircle of [`HALF_WIDTH`], so [`HALF_HEIGHT`] is also what
/// says how much straight jamb there is under it -- see [`Mouth::depth`]. At
/// these two it is three quarters of a metre, which is a proper arch and not a
/// horseshoe.
pub const HALF_WIDTH: f32 = 6.0;
pub const HALF_HEIGHT: f32 = 9.0;

/// How much ground a gate's footing covers, which is what two of them -- or a
/// gate and a mast -- may not share.
///
/// The opening's own half-width. A gate is a hoop rather than a tower: what it
/// takes up on the lawn is exactly how wide it is, and a rule that claimed more
/// would refuse a pair planted either side of a doorway, which is the first
/// thing anybody tries.
pub const RADIUS: f32 = HALF_WIDTH;

/// How deep a bubble is against how wide, as a fraction.
///
/// **One is a true ball, and a true ball is the wrong shape for a threshold.**
/// A gate's opening is its waist -- the disc a body crosses, see
/// [`Mouth::depth`] -- and at a radius this size a ball puts six metres of its
/// own inside between the skin and that disc. What a player walks into is then
/// a long soft nothing: the skin passes over them and is culled from the
/// inside, so the bubble vanishes, and the crossing happens somewhere in the
/// middle of where it used to be.
///
/// Squashed, it is a closed surface that reads as a bubble from the front,
/// shows its picture from every side, and is over in a stride. Wind it back to
/// one for a ball if the look is worth the depth.
const ORB_DEPTH: f32 = 0.3;

/// How thick the frame is drawn, in metres: the radius of the bar the arch is
/// bent out of.
///
/// Thin enough to be a frame rather than a monument, thick enough to survive
/// the internal render resolution the display settings can drop the world to --
/// [`crate::pylon`]'s beams are 0.16 for the same reason and this is the same
/// judgement, scaled up with the arch itself.
const FRAME_THICKNESS: f32 = 0.45;

/// What a gate is built as.
///
/// **Two looks, one set of rules.** An orb is not a second kind of portal with
/// its own arithmetic -- it is the same threshold, the same map to the far
/// side, the same edge on the navigation grid and the same camera flight,
/// drawn as a bubble instead of a doorway. Everything below that asks about a
/// gate's shape asks two questions and no more: how wide is the opening at this
/// height, and how far off the ground is the middle of it.
///
/// Keeping it that narrow is deliberate. A shape that changed how a body was
/// carried, or where the crowd walked, would be a second feature wearing the
/// first one's name -- and the pair of them would disagree the first time
/// anybody planted one of each.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A doorway: two jambs and a semicircular head, standing on the ground.
    #[default]
    Arch,
    /// A bubble: the front half of a sphere, resting on the ground.
    Orb,
}

impl Shape {
    /// How high above its footing the middle of the opening sits, in metres.
    ///
    /// The one number the two shapes disagree about, and the reason they
    /// disagree is that both stand *on the ground*: an arch's middle is half
    /// its height up and a bubble's is its own radius up.
    pub fn rise(self) -> f32 {
        match self {
            Shape::Arch => HALF_HEIGHT,
            Shape::Orb => HALF_WIDTH,
        }
    }

    /// What the shader is told, in [`PortalUniform::shape`]'s last slot.
    fn key(self) -> f32 {
        match self {
            Shape::Arch => 0.0,
            Shape::Orb => 1.0,
        }
    }
}

/// Which end of the pair.
///
/// Blue is the primary trigger and orange the secondary, the way the game this
/// is named after has it, and the two are otherwise identical -- there is no
/// entrance and no exit, only two mouths.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Side {
    /// The first gate planted, and the one the picker's hint names.
    #[default]
    Blue,
    Orange,
}

impl Side {
    pub const BOTH: [Side; 2] = [Side::Blue, Side::Orange];

    /// Its place in every pair of arrays in this module.
    pub fn index(self) -> usize {
        match self {
            Side::Blue => 0,
            Side::Orange => 1,
        }
    }

    /// The other end of the pair.
    pub fn other(self) -> Self {
        match self {
            Side::Blue => Side::Orange,
            Side::Orange => Side::Blue,
        }
    }

    /// The ring's colour, and the tint the far side's picture is seen through.
    fn tint(self) -> Vec3 {
        match self {
            // Above one on purpose: these go through the camera's bloom, which
            // is what puts the halo on the air around a gate. See
            // `portal.wgsl`.
            Side::Blue => Vec3::new(0.35, 1.5, 3.4),
            Side::Orange => Vec3::new(3.6, 1.35, 0.25),
        }
    }
}

/// One gate: where it is, and which way its face looks.
///
/// The rotation is the whole convention of this module. Local **+Z is out of
/// the face**, pointing at whoever is looking at it; +Y is up the opening and
/// +X across it. Every other piece of geometry here is
/// stated in those terms, so there is one place to be wrong about handedness
/// rather than six.
#[derive(Clone, Copy, Debug)]
pub struct Mouth {
    /// The middle of the opening, [`Shape::rise`] above the ground it stands on.
    pub at: Vec3,
    /// The frame described above.
    pub rotation: Quat,
    /// What it is built as. See [`Shape`].
    pub shape: Shape,
    /// The ground in front of it, and whether a body could walk in.
    ///
    /// Worked out once, when the gate is planted, because it is a query into
    /// the level's collision and the answer cannot change: nothing in this game
    /// moves the ground. See [`Self::survey_approach`].
    approach: Option<Vec3>,
}

impl Mouth {
    /// A gate planted on the ground at `foot`, facing `yaw`.
    ///
    /// The constructor the placement uses, and the reason [`Shape::rise`]
    /// exists: whatever a gate is built as it stands *on the ground it was
    /// planted on*, so the middle of the opening is however far up that shape
    /// puts it -- and the bottom of it is at a walker's feet either way.
    ///
    /// `yaw` is the heading its face looks along, in the same measure
    /// [`crate::aim::heading`] hands back and [`crate::pylon::spawn`] takes --
    /// so "turned to face whoever planted it" is one subtraction at the call
    /// site rather than a quaternion.
    pub fn standing(level: &LevelData, foot: Vec3, yaw: f32, shape: Shape) -> Option<Self> {
        let (sin, cos) = yaw.sin_cos();
        let mut mouth = Self::built(
            foot + Vec3::Y * shape.rise(),
            Vec3::new(sin, 0.0, cos),
            shape,
        )?;
        mouth.approach = mouth.survey_approach(level);
        Some(mouth)
    }

    /// The general constructor: a point, the way it faces, and what it is built
    /// as.
    ///
    /// [`Self::standing`] is this with the two things only a level can answer
    /// folded in -- where the ground is, and whether anything can walk in from
    /// the front. The geometry of a pair is the same whether or not a castle is
    /// loaded, which is what lets it be tested.
    ///
    /// The `up` of the frame is world up flattened onto the face, so a planted
    /// gate -- whose face is horizontal -- stands exactly upright. The
    /// flattening is kept rather than replaced by a plain `Vec3::Y`, because
    /// nothing in the arithmetic below assumes upright and a level with its own
    /// idea of down is a thing this game already has; laid flat, where there is
    /// no such thing as up the face, world north stands in.
    pub fn built(at: Vec3, normal: Vec3, shape: Shape) -> Option<Self> {
        let normal = normal.try_normalize()?;
        let along = if normal.y.abs() > 0.95 {
            Vec3::NEG_Z
        } else {
            Vec3::Y
        };
        let up = (along - normal * along.dot(normal)).try_normalize()?;
        let right = up.cross(normal);
        Some(Self {
            at,
            rotation: Quat::from_mat3(&Mat3::from_cols(right, up, normal)),
            shape,
            approach: None,
        })
    }

    /// Out of its face.
    pub fn normal(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }

    /// Up the opening.
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// Across the opening.
    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// Where the gate stands.
    pub fn foot(&self) -> Vec3 {
        self.at - self.up() * self.shape.rise()
    }

    /// Where the hoop, the opening and the far camera's clip plane all sit.
    pub fn transform(&self) -> Transform {
        Transform {
            translation: self.at,
            rotation: self.rotation,
            scale: Vec3::ONE,
        }
    }

    fn matrix(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.at)
    }

    /// The spot on the ground a body walks to in order to go through, or
    /// `None` for a gate nothing can walk into.
    ///
    /// Two things have to be true and each rules out a gate that would
    /// otherwise route a crowd into thin air. There has to be **ground in front
    /// of it**, a stride out, which is what refuses one planted on the very lip
    /// of a drop; and that ground has to be **level with its own footing**,
    /// within a step, or the gate is a window above a cliff rather than a door.
    ///
    /// The face only. A gate is a door on one side and a hoop on the other --
    /// see the module note -- so the side the crowd is routed to is the side
    /// that works.
    fn survey_approach(&self, level: &LevelData) -> Option<Vec3> {
        // A stride out from the opening rather than in it: the point of this is
        // where a body *stands*, and a route that ends inside the gate is a
        // route the survey cannot vouch for.
        let out = self.at + self.normal() * APPROACH_REACH;
        let (ground, _) = level.ground_at(out + Vec3::Y * PROBE_RISE)?;
        let foot = self.foot();
        ((ground - foot.y).abs() <= crate::enemy::STEP_UP)
            .then_some(Vec3::new(out.x, ground, out.z))
    }

    /// Where a body walks to on its way through, or `None` for a gate nothing
    /// can walk into. See [`Self::survey_approach`].
    pub fn walkable(&self) -> Option<Vec3> {
        self.approach
    }

    /// This end as the navigation grid wants it: the spot in front of the gate,
    /// and the point through it that a route ends its last leg at.
    ///
    /// The second is a stride *past* the opening, and at the height of the
    /// ground it is being walked in from rather than at the middle of the
    /// opening. Both matter. Past it, because a leg that stops in front of the
    /// gate is a body that walks up to one and admires it; at ground height,
    /// because the follower steers in the horizontal plane and a leg a metre
    /// and a third up is a leg it is never quite square to.
    pub fn warp(&self) -> Option<crate::flow::Warp> {
        let stand = self.walkable()?;
        let mouth = self.at - self.normal() * THROUGH_REACH;
        Some(crate::flow::Warp {
            stand,
            mouth: Vec3::new(mouth.x, stand.y, mouth.z),
        })
    }

    /// The map that carries a point, a bearing or a whole camera from the
    /// entry's side of the pair to the exit's.
    ///
    /// The exit's frame, a half turn about its up axis, and the entry's frame
    /// undone. The half turn is what makes it a *doorway*: a body walking into
    /// the front of the entry is travelling along the entry's -Z, and after the
    /// turn that is the exit's +Z, which is out of the exit and into the room.
    /// Without it the same body arrives travelling backwards out of the exit's
    /// own hoop.
    pub fn through(entry: &Mouth, exit: &Mouth) -> Mat4 {
        exit.matrix() * Mat4::from_rotation_y(std::f32::consts::PI) * entry.matrix().inverse()
    }

    /// Where the segment from `was` to `now` crosses the opening, as a
    /// fraction along it, or `None` if it does not.
    ///
    /// Front to back only, and that half is what stops a body oscillating: a
    /// portal is entered from the side the picture is on, and something that
    /// has just been *delivered* out of the far mouth is travelling away from
    /// it and cannot immediately be swallowed again.
    pub fn crossing(&self, was: Vec3, now: Vec3) -> Option<f32> {
        let at = self.plane_cross(was, now)?;
        self.inside(was.lerp(now, at)).then_some(at)
    }

    /// Where the segment crosses the gate's *plane*, inside the opening or not.
    ///
    /// Separate from [`Self::crossing`] because two callers want the two
    /// halves. Going through the doorway is what [`transit`] is about; passing
    /// behind the plane *beside* the doorway is what the camera boom must not
    /// do, and telling those apart needs the crossing before the arch is
    /// consulted. See [`Portals::clearance`].
    ///
    /// Front to back only, and that half is what stops a body oscillating: a
    /// gate is entered from the side its face is on, and something that has
    /// just been *delivered* out of the far mouth is travelling away from it
    /// and cannot immediately be swallowed again.
    fn plane_cross(&self, was: Vec3, now: Vec3) -> Option<f32> {
        let normal = self.normal();
        let before = (was - self.at).dot(normal);
        let after = (now - self.at).dot(normal);
        if before <= 0.0 || after > 0.0 {
            return None;
        }
        let span = before - after;
        (span > 1e-6).then(|| before / span)
    }

    /// How far inside the opening a point is, in half-widths, negative outside
    /// it.
    ///
    /// **The arch, and the one place its shape is written down in Rust.** The
    /// same function is in `portal.wgsl`, which cuts the picture out with it,
    /// and the two have to agree or a body walks through air the shader has
    /// drawn as frame -- or, worse, is stopped by nothing at a place the player
    /// can see straight through.
    ///
    /// Two straight jambs rising to a semicircular head of [`HALF_WIDTH`], so
    /// the springing line sits that far below the top and everything above it
    /// is the circle. Measured in half-widths on *both* axes rather than in
    /// each axis's own half-extent, which is what keeps the head round instead
    /// of stretched to whatever proportion the opening happens to have.
    ///
    /// There is deliberately no bottom edge: a doorway's jambs run into the
    /// ground, and a rule that counted the sill would be a bar across it.
    pub fn depth(&self, point: Vec3) -> f32 {
        let local = point - self.at;
        let across = local.dot(self.right()) / HALF_WIDTH;
        let up = local.dot(self.up()) / HALF_WIDTH;
        match self.shape {
            // Two straight jambs to a semicircular head.
            Shape::Arch => {
                let springing = HALF_HEIGHT / HALF_WIDTH - 1.0;
                match up <= springing {
                    true => 1.0 - across.abs(),
                    false => 1.0 - Vec2::new(across, up - springing).length(),
                }
            }
            // A disc: the bubble's own waist, which is its silhouette seen from
            // in front and the widest it ever is. What the mesh does above and
            // below that plane is the shader's business and nothing walks
            // through it -- see `portal.wgsl`.
            Shape::Orb => 1.0 - Vec2::new(across, up).length(),
        }
    }

    /// Whether a point in the opening's plane is inside the arch.
    pub fn inside(&self, point: Vec3) -> bool {
        self.depth(point) > 0.0
    }
}

/// How far outside the opening a gate still counts as being in the camera's
/// way, in half-widths.
///
/// A frame's own reach and a stride more: near enough that the camera really is
/// squeezing past a piece of standing geometry, and no further. In half-widths
/// rather than metres, so it is a *fraction* of the opening -- which is what
/// keeps it a stride at any size the arch is built at. See
/// [`Portals::clearance`] for what goes wrong without an outer limit at all.
const GATE_MARGIN: f32 = 0.3;

/// The gap left between the camera and a gate's plane it has been pulled up
/// short of, in metres.
///
/// [`crate::camera::WALL_GAP`]'s value and its reason: the near plane must not
/// end up inside the thing the boom stopped for.
const CAMERA_GAP: f32 = 0.3;

/// How far past the surface the leg that carries a body through is put, in
/// metres.
///
/// Far enough that a body steering at it has to cross the plane to make any
/// progress towards it at all, and no further: this point is on the *back* side
/// of the gate, which is open lawn a body would happily walk onto, and a leg
/// further out is a leg it would walk right past the gate to reach if the
/// transit ever failed to happen.
const THROUGH_REACH: f32 = 0.5;

/// How high above a candidate spot the ground is probed from, in metres.
///
/// Above anything a gate could be standing on the near side of and below the
/// castle's own roofs, so a gate at the foot of a step finds the step rather
/// than the battlement over it. The same idea [`crate::pipe`] probes with.
const PROBE_RISE: f32 = 4.0;

/// How far in front of a mouth a body stands to walk into it, in metres.
///
/// Rather more than the body's own radius, and clear of the gate's own
/// footprint: this is a spot on the navigation grid, and one the survey would
/// call unwalkable is a spot the route refuses to end at.
const APPROACH_REACH: f32 = 1.1;

// -- the pair ---------------------------------------------------------------

/// The two openings, and a counter that says when they last changed.
///
/// A resource rather than components, for [`crate::pylon::Network`]'s reason:
/// every question asked of a portal is a question about the *pair* -- where
/// does this one come out, is there anything to see through it, does the grid
/// have an edge for it -- and answering those from a query would mean finding
/// both ends before any of them could be asked.
#[derive(Resource, Default, Debug)]
pub struct Portals {
    mouths: [Option<Mouth>; 2],
    /// Which end the next planting replaces.
    ///
    /// **One button plants both ends, so something has to say which**, and the
    /// alternative -- two buttons, or a picker mode each -- spends a control
    /// the game has not got on a decision the player never wants to make. A
    /// pair is built the way a pair is used: put one down, walk somewhere, put
    /// the other down, and from then on each new gate replaces the older of the
    /// two. That is also the only rule under which the pair is never
    /// half-broken by a plant: whatever you do, both ends exist as soon as two
    /// have been planted, and the one that moves is the one you are furthest
    /// from having just used.
    next: Side,
    /// Bumped whenever either end moves or is taken away. The systems that
    /// rebuild something expensive off the pair -- the navigation edge, the
    /// beam network -- watch this and do nothing at all while it stands still.
    pub revision: u64,
}

impl Portals {
    /// One end, if it is open.
    pub fn mouth(&self, side: Side) -> Option<&Mouth> {
        self.mouths[side.index()].as_ref()
    }

    /// Puts one end down, or takes it away.
    pub fn set(&mut self, side: Side, mouth: Option<Mouth>) {
        self.mouths[side.index()] = mouth;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Plants the next gate, and says which end it turned out to be.
    ///
    /// The alternation lives here rather than in [`place`] because the console
    /// plants gates too, and a rule about which end is next that two callers
    /// each had their own copy of is a rule that would disagree the first time
    /// somebody used both. See [`Self::next`].
    pub fn plant(&mut self, mouth: Mouth) -> Side {
        // **A pair is one shape.** The map from one end to the other is the
        // entry's frame undone and the exit's applied, and the two shapes put
        // their middles at different heights off the ground -- so a body
        // walking into an arch and out of a bubble arrives at the bubble's
        // centre, which is its own radius up in the air. Rather than teach the
        // map about mismatched ends, planting a different kind starts a fresh
        // pair: the odd one out goes, and this is the first of the two again.
        let matched = self
            .mouth(self.next.other())
            .is_none_or(|other| other.shape == mouth.shape);
        if !matched {
            self.set(self.next.other(), None);
            self.next = Side::Blue;
        }
        let side = self.next;
        self.next = side.other();
        self.set(side, Some(mouth));
        side
    }

    /// Both ends, in the order `(the one you go in, the one you come out)` for
    /// a body entering `side`.
    pub fn pair(&self, side: Side) -> Option<(&Mouth, &Mouth)> {
        Some((self.mouth(side)?, self.mouth(side.other())?))
    }

    /// Whether there is a way through at all, which is the question every
    /// system downstream of this one asks first.
    pub fn open(&self) -> bool {
        self.mouths.iter().all(Option::is_some)
    }

    /// The pair as the navigation grid wants it, in blue-then-orange order, or
    /// `None` unless *both* ends can be walked into.
    ///
    /// Both, because a one-way edge on the navigation grid is a route the crowd
    /// walks into and cannot come back out of -- and because the pair is one
    /// fact. See [`Mouth::survey_approach`] for what makes an end walkable.
    pub fn walkway(&self) -> Option<(crate::flow::Warp, crate::flow::Warp)> {
        let (blue, orange) = self.pair(Side::Blue)?;
        Some((blue.warp()?, orange.warp()?))
    }

    /// How much of a boom hung off `focus` the camera may use before it ends up
    /// behind a gate without having gone through the doorway, as a fraction.
    ///
    /// **A gate is a wall in the one place it is not a doorway: right beside
    /// the opening.** The boom is already pulled in by the castle's own walls
    /// ([`crate::camera::update`]), and a gate wants the same treatment for a
    /// different cause. Passing through the doorway is what the boom is
    /// *supposed* to do -- see [`carry_camera`] -- and passing behind the
    /// arch's plane a hand's breadth to one side of a jamb is the same camera
    /// position reached the wrong way, with nowhere on the far side that
    /// corresponds to it. Left alone, a player who veers sideways while the
    /// camera is still behind the gate has the view snap from one end of the
    /// pair to the other. Pulled in, the camera hangs closer to him for the
    /// stride it takes to get clear.
    ///
    /// Two things are deliberately *not* obstructions. A boom through the
    /// **opening** goes at full length, which is the whole point of the arch
    /// being as big as it is: the ordinary case of walking through one is the
    /// boom coming through it with you. And a boom crossing the gate's plane
    /// **well clear of it** goes at full length too, because a gate is a
    /// free-standing frame on a lawn rather than a wall -- its plane is
    /// infinite and the thing standing in it is three metres across. Without
    /// that second rule a gate stops the camera everywhere in the level that
    /// happens to share a plane with it, which is a stripe across the whole map.
    ///
    /// One is "nothing in the way", exactly as the wall test's is.
    pub fn clearance(&self, focus: Vec3, boom: Vec3) -> f32 {
        let length = boom.length();
        if length <= f32::EPSILON {
            return 1.0;
        }
        let camera = focus + boom;
        Side::BOTH
            .iter()
            .filter_map(|side| self.mouth(*side))
            .filter_map(|gate| {
                let at = gate.plane_cross(focus, camera)?;
                // Negative outside the opening, and only the near miss counts.
                let depth = gate.depth(focus.lerp(camera, at));
                (depth <= 0.0 && depth > -GATE_MARGIN).then(|| (at - CAMERA_GAP / length).max(0.0))
            })
            .fold(1.0_f32, f32::min)
    }

    /// The gate a boom goes through, and the map to the far side of the pair.
    ///
    /// `None` when the boom reaches no gate's doorway, which is almost every
    /// frame of the game.
    pub fn boom_through(&self, focus: Vec3, camera: Vec3) -> Option<Mat4> {
        Side::BOTH.iter().find_map(|&side| {
            let (entry, exit) = self.pair(side)?;
            entry.crossing(focus, camera)?;
            Some(Mouth::through(entry, exit))
        })
    }

    /// The two points a beam is threaded through, in blue-then-orange order.
    ///
    /// The mouth centres. Light through a portal genuinely enters and leaves
    /// somewhere on the disc rather than at the middle of it, and the middle is
    /// the right approximation for the same reason a pylon's beam leaves from
    /// one point rather than from the whole emitter head: the openings are two
    /// metres across and the beams are a hundred and fifty long.
    pub fn optics(&self) -> Option<(Vec3, Vec3)> {
        let (blue, orange) = self.pair(Side::Blue)?;
        Some((blue.at, orange.at))
    }
}

// -- planting them ------------------------------------------------------------

/// The live placement: how long the button has been down, where it resolves to,
/// and what the ring is saying about that spot.
///
/// [`crate::pylon::Plant`]'s shape and for its reason: the state belongs to the
/// button rather than to any one entity, and the system that draws the preview
/// and the system that plants want the same answer.
#[derive(Resource, Default)]
pub struct Planting {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    /// Nothing already standing is in the way.
    pub clear: bool,
}

impl Planting {
    /// The ring appears once the press has outlasted a tap, so a tap does not
    /// flash one on its way to planting a gate.
    pub fn showing(&self) -> bool {
        self.held_for.is_some_and(|held| held >= squad::TAP_SECONDS)
    }
}

/// Whether a gate may be planted here: is there room for its footing.
///
/// Deliberately [`crate::pylon::fits`]'s own rule rather than a second one that
/// looks like it. A gate, a mast and a machine are all things standing on the
/// lawn with a footprint, and a player who has learnt what a red ring means
/// under one should not have to learn it again under the next.
pub fn fits(at: Vec3, placed: &[(Vec3, f32)]) -> bool {
    stellarator::fits(at, RADIUS, placed)
}

/// Everything the preview and the gates both need kept off each other's
/// `Transform`.
///
/// Every exclusion is load-bearing, for [`crate::pylon`]'s reason: Bevy proves
/// two queries disjoint from their filters alone, so a mutable `Transform` that
/// does not name the other `Transform` queries in its own system is a schedule
/// that refuses to build -- which, in a windowed build, is a game that opens
/// and shuts without a word.
type SiteQuery<'w, 's> = Query<
    'w,
    's,
    &'static mut Transform,
    (
        With<PortalSite>,
        Without<Player>,
        Without<Camera3d>,
        Without<PortalFrame>,
        Without<PortalSurface>,
        Without<crate::pylon::Pylon>,
        Without<Stellarator>,
    ),
>;

/// The plant button: held opens a site, released puts a gate on it.
///
/// Runs at the render rate rather than on the fixed step, for
/// [`crate::pylon::place`]'s reason -- the preview is drawn every frame -- and
/// it takes the released edge the same latched way, so a press is neither lost
/// nor counted twice across the fixed-step boundary.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn place(
    time: Res<Time>,
    assets: Res<PortalArt>,
    mut input: ResMut<InputState>,
    level: Res<LevelData>,
    mut planting: ResMut<Planting>,
    mut portals: ResMut<Portals>,
    camera: Query<
        &Transform,
        (
            With<Camera3d>,
            Without<Player>,
            Without<PortalView>,
            Without<PortalSite>,
        ),
    >,
    player: Query<&Transform, With<Player>>,
    masts: Query<(&Transform, &crate::pylon::Pylon)>,
    machines: Query<(&Transform, &Stellarator)>,
    mut site: SiteQuery,
    mut visibility: Query<&mut Visibility, With<PortalSite>>,
    mut ring: Query<&mut MeshMaterial3d<StandardMaterial>, With<PortalSiteRing>>,
) {
    let (Ok(camera), Ok(leader)) = (camera.single(), player.single()) else {
        return;
    };
    // Two buttons and one placement, because the two shapes are the same
    // gesture at the same site and only differ in what stands there. Which one
    // is held decides what gets planted; the arch wins a tie, which cannot
    // happen through the picker and can through the console's own flags.
    let orb = input.orb || input.orb_released;
    let shape = match input.portal || input.portal_released {
        true => Shape::Arch,
        false => Shape::Orb,
    };
    let released =
        InputState::take(&mut input.portal_released) | InputState::take(&mut input.orb_released);
    if input.portal || orb || released {
        // Refreshed on the press as well as on the hold, so a tap too short to
        // have opened a site still plants somewhere.
        planting.aim = squad::aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
            // A gate is put down within sight of the person putting it down,
            // exactly as a mast is. See [`crate::squad::PLACE_REACH`].
            squad::PLACE_REACH,
        );
        planting.held_for = Some(planting.held_for.unwrap_or(0.0) + time.delta_secs());
        // Everything already standing, including the *other* gate: a pair
        // planted through itself is a pair whose two ends are one place, which
        // the navigation grid refuses outright and which nothing else in the
        // game has any answer for.
        let taken: Vec<_> = masts
            .iter()
            .map(|(transform, mast)| (transform.translation, mast.radius))
            .chain(
                machines
                    .iter()
                    .map(|(transform, machine)| (transform.translation, machine.radius)),
            )
            .chain(
                Side::BOTH
                    .iter()
                    .filter(|side| **side != portals.next)
                    .filter_map(|side| portals.mouth(*side))
                    .map(|mouth| (mouth.foot(), RADIUS)),
            )
            .collect();
        planting.clear = fits(planting.aim, &taken);
    }
    if released {
        planting.held_for = None;
        if planting.clear {
            // Turned to face whoever planted it, which is the whole of the
            // "a gate has a front" rule being usable: the side you are standing
            // on when you let go is the side that is a door. See the module
            // note.
            let away = leader.translation - planting.aim;
            if let Some(mouth) = Mouth::standing(&level, planting.aim, away.x.atan2(away.z), shape)
            {
                portals.plant(mouth);
            }
        }
    }
    let showing = planting.showing();
    if let Ok(mut transform) = site.single_mut() {
        if showing {
            transform.translation = planting.aim;
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
        // Two answers rather than the mast's three: a gate joins nothing, so
        // there is no third state between "legal" and "wired in" to draw. The
        // colours are the mast's own, which is the point -- one mark, learnt
        // once.
        let wanted = match planting.clear {
            true => &assets.site_clear,
            false => &assets.site_blocked,
        };
        if material.0.id() != wanted.id() {
            material.0 = wanted.clone();
        }
    }
}

// -- walking through them -----------------------------------------------------

/// Where a body was at the end of the last tick, so a step can be tested
/// against the plane rather than a position.
///
/// **A position test cannot work here and it is worth saying why.** A body
/// moves up to a third of a metre in a tick and the opening is a plane with no
/// thickness; asking "is it inside the portal" needs a slab thick enough to
/// catch that, and a slab that thick catches a body walking *past* the mouth
/// past the side of it. Asking instead whether the step crossed the plane
/// inside the ellipse is exact, catches a body moving at any speed, and cannot
/// fire on one that never went through.
#[derive(Component, Default, Debug)]
pub struct Transit {
    was: Option<Vec3>,
    /// Ticks before this body may be swallowed again.
    ///
    /// It comes out of the far mouth travelling away from it, so nothing about
    /// the geometry wants it back -- but a body shoved into the exit by the
    /// crowd behind it on the very next tick would go straight back, and a pair
    /// of portals facing each other across a courtyard would hold a stream of
    /// enemies in a loop. A tick or two of grace is all it takes and it costs
    /// nothing anywhere else.
    grace: u8,
    /// The map the last transit made, until somebody has acted on it.
    ///
    /// **Only the camera wants this, and it wants it because a camera is not a
    /// body.** Everything else about a transit is finished by the time
    /// [`transit`] returns -- the position, the bearing and the speed are all
    /// written straight onto the body. An orbiting camera has none of those: it
    /// has a yaw and a smoothed focus, both in its own frame, which nothing in
    /// this module can reach from inside a query that is already holding every
    /// walking thing in the world mutably. So the map is left here and
    /// [`turn_camera`] takes it on the next drawn frame, which is the same
    /// frame the player first sees the far side on.
    carried: Option<Mat4>,
}

/// How high up a body the crossing is measured, in metres.
///
/// Not its feet, which are on the floor and would only ever cross a portal in
/// one -- and the floor is not cut, see the module note. Waist height on Luna
/// and on a Mario, chest height on a slime, and comfortably inside the opening
/// for every one of them.
const TRANSIT_LIFT: f32 = 0.6;

/// How many ticks a body that has just come through is left alone for.
const TRANSIT_GRACE: u8 = 2;

/// Carries whatever crossed an opening this tick out of the other one.
///
/// Runs late in the tick, after everything that moves a body, because what it
/// tests is the step the tick actually took. Position, bearing and speed all go
/// through: the position because that is what a portal is, and the other two
/// because a body that arrives facing the way it set off is a body that walks
/// straight back through the gate it just came out of.
#[allow(clippy::type_complexity)]
pub fn transit(
    portals: Res<Portals>,
    mut bodies: Query<(
        &mut Transform,
        &mut Transit,
        Option<&mut Controller>,
        Option<&mut crate::squad::Ally>,
        Option<&mut crate::path::Route>,
    )>,
) {
    for (mut transform, mut memory, controller, ally, route) in &mut bodies {
        let now = transform.translation + Vec3::Y * TRANSIT_LIFT;
        let was = memory.was.replace(now);
        memory.grace = memory.grace.saturating_sub(1);
        let Some(was) = was else {
            continue;
        };
        if memory.grace > 0 || !portals.open() {
            continue;
        }
        let Some((entry, exit)) = Side::BOTH.iter().find_map(|&side| {
            let (entry, exit) = portals.pair(side)?;
            entry.crossing(was, now)?;
            Some((entry, exit))
        }) else {
            continue;
        };
        let carry = Mouth::through(entry, exit);
        let turn = Quat::from_mat4(&carry);
        // The body itself. Taken on the *test* point and put back down by the
        // same lift, so a body whose origin is at its feet lands on its feet.
        transform.translation = carry.transform_point3(now) - Vec3::Y * TRANSIT_LIFT;
        transform.rotation = turn * transform.rotation;
        memory.was = Some(transform.translation + Vec3::Y * TRANSIT_LIFT);
        memory.grace = TRANSIT_GRACE;
        memory.carried = Some(carry);
        // And its speed, turned rather than kept or dropped. Kept, a body
        // running east out of a portal facing north keeps running east through
        // the gate; dropped, every transit is a dead stop and a jump through a
        // gate lands at the exit's feet.
        if let Some(mut controller) = controller {
            controller.velocity = turn * controller.velocity;
        }
        if let Some(mut ally) = ally {
            ally.velocity = turn * ally.velocity;
        }
        // The route it was walking is a list of corners on the side it has just
        // left, and the first of them is the mouth it went through. Nothing
        // sensible can be salvaged from that, and a body that keeps it walks
        // back at the exit it just came out of. Dropped, it is replanned from
        // where it now is on the next tick the budget reaches it -- which is
        // the same handful of ticks a body waits for a route anywhere.
        if let Some(mut route) = route {
            route.replan();
        }
    }
}

/// Turns the camera with the player when he goes through.
///
/// Its own system rather than a branch inside [`transit`], for the reason
/// [`Transit::turned`] gives: the camera is an orbit rather than a body, and
/// what has to be carried through is the yaw of that orbit. A player who walks
/// through a portal into a corridor at right angles to the one he left and
/// keeps the old yaw is a player looking the wrong way down a valley, hunting
/// for where he came out with the mouse before he can carry on.
///
/// The turn is applied to a *bearing* rather than to the camera's transform,
/// which is what keeps it right on a level whose up is not world up:
/// [`crate::camera::FollowCamera::yaw`] is an angle inside
/// [`crate::camera::FollowCamera::view`], and the difference between two
/// headings is the same number in any frame those two headings share an up
/// with.
pub fn turn_camera(
    mut player: Query<&mut Transit, With<Player>>,
    mut camera: Query<&mut crate::camera::FollowCamera>,
) {
    let (Ok(mut transit), Ok(mut follow)) = (player.single_mut(), camera.single_mut()) else {
        return;
    };
    // Taken rather than read: a turn acted on twice is a camera that swings
    // round again on the frame after the transit, and every frame after that.
    let Some(carry) = transit.carried.take() else {
        return;
    };
    let turn = Quat::from_mat4(&carry);
    let before = Vec3::new(follow.yaw.sin(), 0.0, follow.yaw.cos());
    let after = turn * before;
    follow.yaw = aim::wrap(follow.yaw + aim::heading(after) - aim::heading(before));
    // **And the focus starts again at the player.** The point the boom hangs off
    // chases him at [`crate::console::GameTuning::cam_smooth`] a frame, which
    // is what filters his steps and bumps out of the view, and a transit is not
    // a step: eased across one, the boom sets off after him at walking pace and
    // drags the picture along behind.
    //
    // Carrying it through the pair rather than dropping it was the obvious
    // thing and was wrong, in a way that is worth writing down because it is
    // the *flash* somebody will otherwise chase for an afternoon. The focus
    // lags him, so at the instant he crosses it is still a little way in
    // **front** of the entry -- and a map that carries the front of one gate
    // carries it to the *back* of the other. So the focus lands behind the exit
    // and eases forward over the next few frames, and for every one of those
    // frames [`Portals::boom_through`] finds no way through -- a boom that
    // starts behind a gate does not go through its doorway -- so the camera is
    // not flown, and sits out in the open behind the exit looking at the world
    // from the far side. One frame of that is a flash. `None` costs the
    // smoothing for a single frame and cannot be on the wrong side of anything.
    follow.focus = None;
}

/// Gives every body that could go through a portal its memory of where it was.
///
/// By what it *is* rather than by a list kept somewhere: anything with a
/// [`Controller`], an [`crate::squad::Ally`] or an [`crate::enemy::Enemy`] on
/// it walks the world and can walk into a gate, and a body spawned by any of
/// the four systems that spawn those gets this without that system knowing
/// portals exist.
#[allow(clippy::type_complexity)]
pub fn claim(
    mut commands: Commands,
    bodies: Query<
        Entity,
        (
            Without<Transit>,
            Or<(
                With<Controller>,
                With<crate::squad::Ally>,
                With<crate::enemy::Enemy>,
            )>,
        ),
    >,
) {
    for body in &bodies {
        commands.entity(body).try_insert(Transit::default());
    }
}

/// A second copy of the player's body, standing where he already is on the
/// other side.
///
/// **This is the whole of "half way in and out".** A transit is instant: on the
/// tick the middle of him crosses, all of him is at the far gate. What makes it
/// read as walking through a doorway rather than as blinking is that for the
/// second either side of that tick there is a body at *both* gates, and each
/// one is cut off at its own threshold -- so the front half emerging over there
/// and the back half still over here are the two halves of one body.
///
/// Neither half is cut by a shader. Both come for free out of geometry that had
/// to be there anyway:
///
///   * **The near half.** Whatever of the real body has gone past the gate's
///     plane is *behind the opening*, and the opening is an opaque quad that
///     writes depth. So it is simply occluded -- and what is drawn over it is
///     the picture through the gate, which is where the far half is.
///   * **The far half.** The ghost stands at the exit, and the camera that
///     draws what the near gate shows has its near plane laid on the exit's own
///     plane ([`clip_plane`]). Everything of the ghost behind that plane is
///     outside the frustum. The oblique frustum that stops a gate showing the
///     ground behind it is the same one that cuts the ghost in half.
///
/// So a ghost is an ordinary [`crate::player::PlayerVisual`] with this marker
/// on it, and the marker's whole job is to keep the two systems that drive the
/// real visuals off it: [`crate::player::sync_visual`], which would put it back
/// on the player, and the character swap, which would show it. The animation is
/// not excluded and must not be -- `animation::update` drives every player on
/// the active character, so the ghost is in the same pose on the same frame
/// without anything being copied.
#[derive(Component, Clone, Copy)]
pub struct Ghost;

/// How near a gate's plane the player has to be for there to be two of him, in
/// metres.
///
/// A body's own half-depth, near enough. Any less and the halves separate
/// before they have finished being one body; any more and a ghost is standing
/// at the far gate while the player is still a stride away from this one, which
/// is a second player rather than the same one.
const STRADDLE_DEPTH: f32 = 0.75;

/// Puts the ghost where the player already is on the far side, while he is in
/// the doorway.
///
/// Measured on the *rendered* pose rather than the simulated one, and that is
/// its correctness rather than a nicety: the real body is drawn interpolated
/// between two ticks, so a ghost placed from the tick would separate from it by
/// up to a whole step of walking -- which is exactly the seam this exists to
/// hide.
pub fn ghost(
    portals: Res<Portals>,
    pose: Res<crate::player::RenderPose>,
    state: Res<crate::GameState>,
    mut ghosts: Query<(&crate::ActiveCharacter, &mut Transform, &mut Visibility), With<Ghost>>,
) {
    // Which gate he is in the doorway of, and which way that carries him. Both
    // ways round are tried, because the half-second *after* a transit is the
    // half-second before one seen from the far side: he is standing in the
    // exit's own doorway, and the ghost belongs back at the gate he came from.
    let straddling = Side::BOTH.iter().find_map(|&side| {
        let (entry, exit) = portals.pair(side)?;
        let at = pose.translation + Vec3::Y * TRANSIT_LIFT;
        let across = (at - entry.at).dot(entry.normal());
        (across.abs() <= STRADDLE_DEPTH && entry.inside(at)).then(|| Mouth::through(entry, exit))
    });
    for (character, mut transform, mut visible) in &mut ghosts {
        let showing = straddling.filter(|_| *character == state.active);
        match showing {
            Some(carry) => {
                let (_, rotation, translation) = (carry
                    * Transform {
                        translation: pose.translation,
                        rotation: pose.rotation,
                        scale: Vec3::ONE,
                    }
                    .to_matrix())
                .to_scale_rotation_translation();
                transform.translation = translation;
                transform.rotation = rotation;
                *visible = Visibility::Visible;
            }
            None => *visible = Visibility::Hidden,
        }
    }
}

// -- telling the rest of the game ---------------------------------------------

/// Hangs the pair's edge on the navigation grid, and takes it off again.
///
/// Cheap to check and expensive to do -- a new edge means the next sweep is
/// over a different graph, and every route cached against the old one is stale
/// -- so the check is the whole system on all but the handful of ticks where
/// somebody fired the gun.
pub fn wire_field(
    portals: Res<Portals>,
    mut field: ResMut<FlowField>,
    mut wired: Local<Option<u64>>,
) {
    if *wired == Some(portals.revision) {
        return;
    }
    *wired = Some(portals.revision);
    field.set_warp(portals.walkway());
}

/// A gate planted on the ground under a spot, facing a point.
///
/// The console's way in, and its only job is to be *reproducible* -- the same
/// numbers have to give the same gate on the same level every run, which rules
/// out anything that depends on how the player happens to be standing at the
/// time. So the ground is found by dropping onto it rather than by asking where
/// anybody is, and the facing is the one thing a gate cannot do without: it is
/// turned to look at `toward`, which the caller passes as the player's own
/// position, exactly as [`place`] does.
///
/// `None` where there is no ground under the spot, or none in front of the gate
/// once it is standing there. Both are the ordinary answer for three numbers
/// typed into a console.
fn drop_onto(level: &LevelData, spot: Vec3, toward: Vec3, shape: Shape) -> Option<Mouth> {
    let (ground, _) = level.ground_at(spot + Vec3::Y * PROBE_RISE)?;
    let foot = Vec3::new(spot.x, ground, spot.z);
    let away = toward - foot;
    Mouth::standing(level, foot, away.x.atan2(away.z), shape)
}

/// `portal`, `portal clear`, `portal x,y,z x,y,z`: opens a pair or shuts one.
///
/// The console's half of the gun, and it exists for the reason every other
/// command in this game does -- a state that takes two accurate shots to reach
/// is a state a screenshot cannot reproduce, and one that takes none to leave
/// is worth having.
pub fn command(
    mut console: ResMut<ConsoleState>,
    level: Res<LevelData>,
    mut portals: ResMut<Portals>,
    player: Query<&Transform, With<Player>>,
) {
    for request in console.take_requests() {
        match request {
            crate::console::Request::ClearPortals => {
                for side in Side::BOTH {
                    portals.set(side, None);
                }
            }
            crate::console::Request::PlacePortals(blue, orange, orb) => {
                let shape = match orb {
                    true => Shape::Orb,
                    false => Shape::Arch,
                };
                let Ok(leader) = player.single() else {
                    continue;
                };
                // Set by side rather than through [`Portals::plant`], because
                // the command names which end each spot is and a caller who
                // wrote `portal a b` and got them the other way round would
                // have to know about the alternation to understand why.
                for (side, spot) in [(Side::Blue, blue), (Side::Orange, orange)] {
                    // Left where it was where the spot has no ground under it,
                    // exactly as a plant onto a blocked site is. See [`place`].
                    if let Some(mouth) =
                        drop_onto(&level, Vec3::from_array(spot), leader.translation, shape)
                    {
                        portals.set(side, Some(mouth));
                    }
                }
            }
            other => console.defer(other),
        }
    }
}

// -- seeing through them ------------------------------------------------------

/// The two layers a gate's surface is drawn on, and who draws which.
///
/// **A camera cannot sample the texture it is writing.** The image the blue
/// gate shows is drawn by the blue portal camera, and if that pass drew a
/// surface reading the same image the result is not a recursion, it is a
/// texture bound as a render attachment and as a resource at once -- which is a
/// validation error rather than a picture. That single rule is why an infinity
/// mirror is not free, and why there are two of everything below.
///
/// Each gate has **two** surfaces standing in the same place, showing the same
/// two-frame ping-pong of images from opposite ends:
///
///   * The **near** one is on [`NEAR_LAYER`], which only the world's own camera
///     draws, and it shows the image the portal passes finished writing *this
///     frame*. That is what the player looks at, and it is not stale.
///   * The **far** one is on [`FAR_LAYER`], which only the portal cameras draw,
///     and it shows the image from the frame before. Nothing is writing that
///     one while they run, so they may draw it freely -- and what they draw is
///     the next level down.
///
/// So looking into blue shows this frame's pass; inside it stands blue again
/// from the frame before; inside that, the frame before that, as far down as
/// there are pixels. Each level is one frame staler than the one around it, no
/// level costs a pass, and the outermost -- the one anybody is actually looking
/// at -- is current.
///
/// A gate's *frame* is on neither: it is ordinary geometry on layer nought, and
/// every camera draws it. The impostor baker and anything else drawing the
/// world for somebody who is not the player stays on nought alone and so never
/// sees a portal surface at all, which is right: an impostor sheet with a
/// window onto somewhere else baked into it would show that somewhere else for
/// the rest of the session.
const NEAR_LAYER: usize = 1;
const FAR_LAYER: usize = 2;

/// What the camera drawing the world for the player belongs to.
pub fn world_layers() -> RenderLayers {
    RenderLayers::from_layers(&[0, NEAR_LAYER])
}

/// How far *in front* of the exit the far camera's near plane is put, in metres.
///
/// A hair more than the hoop is thick, and both halves of that matter. Clipped
/// exactly at the gate's own plane, the exit's hoop is half kept and half cut,
/// and the two halves disagree about rounding -- which is a shimmering fringe
/// around the inside of every opening. Clipped a hair in front of it, the hoop
/// is gone.
///
/// **And it should be gone.** The far camera stands behind the exit and looks
/// out through it, so the exit's own hoop is a ring a few metres ahead
/// subtending very nearly the angle the near gate's opening does on screen: kept,
/// it lands exactly on the near gate's rim and recolours it with the far gate's
/// colour. What a player is meant to see inside a blue ring is the lawn on the
/// other side, not a second orange ring drawn on top of the first.
///
/// The slab of world it costs is twelve centimetres immediately in front of the
/// exit, which nothing can be standing in: a body that close has already gone
/// through.
const PLANE_BIAS: f32 = FRAME_THICKNESS + 0.03;

/// The four values the portal shader reads. See `portal.wgsl`.
#[derive(Clone, Copy, ShaderType)]
struct PortalUniform {
    rim: Vec4,
    shape: Vec4,
}

/// The surface of one opening: a picture of somewhere else, and a ring.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct PortalMaterial {
    #[uniform(0)]
    uniform: PortalUniform,
    /// What the far camera drew. `None` is a portal with nothing behind it,
    /// which the shader draws as the ring alone.
    #[texture(1)]
    #[sampler(2)]
    view: Option<Handle<Image>>,
}

impl Material for PortalMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn enable_shadows() -> bool {
        false
    }

    /// The depth prepass would write the quad's *own* depth, and the quad's own
    /// depth is the hoop it is strung in rather than the depth of the ground on
    /// the far side. Anything reading that -- and the prepass exists to be read
    /// -- would then be told that the world stops at the gate, which is exactly
    /// what a portal is for denying.
    fn enable_prepass() -> bool {
        false
    }
}

/// The camera that draws what is on the far side of one opening.
///
/// Carries the side of the pair whose *surface* it feeds, not the side it
/// stands behind: the blue opening shows the world from behind the orange one,
/// and naming these after the picture rather than after the position is what
/// keeps every array in this module in one order.
#[derive(Component, Clone, Copy)]
pub struct PortalView(pub Side);

/// One of the two surfaces a gate is drawn on.
///
/// Which of the two it is, is its [`RenderLayers`] rather than a field here:
/// the whole difference between them is who is allowed to draw them, and that
/// *is* a render layer. See [`NEAR_LAYER`].
#[derive(Component, Clone, Copy)]
pub struct PortalSurface(pub Side);

/// The hoop around one opening.
///
/// A gate is a free-standing thing on a lawn, and an opening with no edge is a
/// hole in the air: seen from the side it would vanish entirely, and seen from
/// behind there would be nothing there at all. The frame is what makes it an
/// object -- and it is the only part of a gate that is visible from its back.
#[derive(Component, Clone, Copy)]
pub struct PortalFrame(pub Side);

/// The disc on the ground under the crosshair while the button is held.
#[derive(Component)]
pub struct PortalSite;

/// The ring drawn on it, whose colour is the answer.
#[derive(Component)]
pub struct PortalSiteRing;

/// The mesh, the two materials and the two images, built once.
#[derive(Resource)]
pub struct PortalArt {
    /// Two images per side, written and read on alternate frames. See
    /// [`NEAR_LAYER`], and [`flip_views`], which does the alternating.
    views: [[Handle<Image>; 2]; 2],
    /// The material each side's near and far surface wears, in that order.
    materials: [[Handle<PortalMaterial>; 2]; 2],
    /// The surface each shape is drawn on, and the frame that stands round it.
    ///
    /// Both built once and shared by both ends: a gate changing shape is a
    /// handle swapped on two entities, which is what makes the choice free at
    /// the moment somebody makes it. There is no matching pair of frames,
    /// because a bubble has none -- its own limb is its edge, and a ring round
    /// it would be an arch nobody asked for.
    surfaces: [Handle<Mesh>; 2],
    /// The ring the placement preview draws on the ground: legal, and blocked.
    ///
    /// [`crate::pylon::GridArt`]'s own two colours rather than a second pair
    /// that looks like them, because it is the same mark saying the same thing.
    site_clear: Handle<StandardMaterial>,
    site_blocked: Handle<StandardMaterial>,
}

/// The plane the far camera must clip its near side against, in its own view
/// space, or `None` where there is nothing to clip.
///
/// **Without this a portal shows whatever is standing behind the far gate.**
/// The camera drawing the far side stands *behind* the exit -- that is what
/// carrying the eye through the pair means -- so between it and the opening is
/// every metre of ground, every tree and every Mario on the exit's own back
/// side, and all of it lands inside the near gate's ring. On a lawn the worst
/// of it is the ground itself, which runs continuously under the gate and fills
/// the bottom of the frame.
///
/// Pulling the near plane forward cannot fix it: a near plane is parallel to
/// the screen and the gate is not, so any distance that clears the gate at one
/// edge of the opening eats the world at the other. What is wanted is a near
/// plane that *is* the gate, and that is
/// [`PerspectiveProjection::near_clip_plane`] -- Bevy's own oblique frustum,
/// which does the whole of Lengyel's construction including the rescaling that
/// keeps the depth buffer's precision. This function's only job is to hand it
/// the right plane.
///
/// The exit's own plane, pushed [`PLANE_BIAS`] forward past its hoop, with its
/// normal pointing out into the room the exit opens onto -- so the half-space
/// the camera keeps is the half a player standing in front of the exit can see.
///
/// A plane is carried into another space by the *transpose of the inverse* of
/// the map that carries points, and the map that carries points from view space
/// to the world is the camera's own transform, so this is one transpose and no
/// inverse at all.
///
/// `None` when the camera has ended up in front of the plane rather than behind
/// it, which happens for the frame or two a player spends stepping through: the
/// construction assumes the camera is on the near side of what it is clipping,
/// and a frame with that sign flipped is a picture drawn inside out. A plain
/// frame for those frames is at worst a glimpse of the far gate's own back.
fn clip_plane(exit: &Mouth, camera: &Transform) -> Option<Vec4> {
    let normal = exit.normal();
    let point = exit.at + normal * PLANE_BIAS;
    let world = normal.extend(-normal.dot(point));
    let view = camera.to_matrix().transpose() * world;
    // `view.w` is the plane's value at the view origin, which is the signed
    // distance from the camera to it.
    (view.w < -1e-3).then_some(view)
}

/// The plane that means "no oblique clipping", in the form Bevy recognises.
///
/// It short-circuits on the *normal* rather than on the whole vector, so this
/// has to be a plane pointing straight down the view axis; the distance in it
/// is never read. Written out rather than left to
/// `PerspectiveProjection::default`, because the default carries that type's
/// near plane and not this camera's.
fn no_clip_plane(near: f32) -> Vec4 {
    Vec4::new(0.0, 0.0, -1.0, -near)
}

/// The lens the far camera looks through: the world camera's own, with its near
/// plane laid onto the opening.
///
/// The field of view is copied rather than kept in step by hand, because the
/// picture inside the frame and the world around it have to be at one zoom or
/// the opening reads as a screen. The aspect ratio is not copied -- Bevy
/// maintains that against each camera's own render target, and the two targets
/// are the same size by [`resize`].
fn lens_for(
    world: &PerspectiveProjection,
    exit: &Mouth,
    camera: &Transform,
) -> PerspectiveProjection {
    let mut lens = PerspectiveProjection {
        fov: world.fov,
        near: world.near,
        far: world.far,
        ..world.clone()
    };
    let Some(plane) = clip_plane(exit, camera) else {
        lens.near_clip_plane = no_clip_plane(lens.near);
        return lens;
    };
    // **Bevy declines to tilt a plane that is already square to the view axis,
    // and it decides that on the normal alone.** Such a plane is the shape a
    // near plane already has, so there is nothing to tilt -- but the *distance*
    // in it would then be quietly dropped, and the frame would clip at the
    // world camera's five centimetres rather than at the gate. The one
    // orientation that hits it is a gate looked at exactly head-on, which is
    // also the easiest one in the game to line up: plant one and walk straight
    // at it -- a gate is turned to face whoever planted it, so that is the
    // default view of every gate in the game. So for that case the distance goes into `near` instead, which
    // for this orientation and no other is the same frame.
    if plane.x == 0.0 && plane.y == 0.0 && plane.z == -1.0 {
        lens.near = (-plane.w).max(world.near);
        lens.near_clip_plane = no_clip_plane(lens.near);
    } else {
        lens.near_clip_plane = plane;
    }
    lens
}

/// Builds the two images, the two cameras and the two quads, once.
///
/// All four exist for the whole session whether or not a portal has ever been
/// fired, and that is the point: a camera spawned mid-frame has no propagated
/// transform until the next one, so a gate planted on the lawn would show a
/// picture from wherever the origin is for a frame before settling. Everything
/// here is switched off rather than absent -- [`Camera::is_active`] false and
/// [`Visibility::Hidden`] -- which costs a hidden entity and a texture and
/// makes the first frame of a portal the right one.
pub fn setup(
    mut commands: Commands,
    // All three optional, and the material collection is the one that matters:
    // it is [`PortalPlugin`]'s, and the headless harness runs this schedule
    // with every render plugin stubbed out. What a portal *is* in that world is
    // a pair of positions the pathing and the pylon network read out of
    // [`Portals`], and none of the four entities below has anything to do with
    // either. Refusing to start rather than spawning half of them is what keeps
    // "the cameras exist for the whole session" true wherever they exist at all.
    images: Option<ResMut<Assets<Image>>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<PortalMaterial>>>,
    surfaces: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let (Some(mut images), Some(mut meshes), Some(mut materials), Some(mut surfaces)) =
        (images, meshes, materials, surfaces)
    else {
        return;
    };
    // The opening is the ellipse inscribed in this, cut out in the shader --
    // see `portal.wgsl` for why the mesh is not the shape.
    let quad = meshes.add(Rectangle::new(HALF_WIDTH * 2.0, HALF_HEIGHT * 2.0));
    // The frame, built at its real size in the gate's own plane, so nothing
    // downstream has to scale or turn it. See [`arch_mesh`].
    let arch = meshes.add(arch_mesh());
    // And the bubble: a whole closed surface, squashed along the gate's own
    // axis. See [`ORB_DEPTH`].
    let bubble = meshes.add(
        Sphere::new(HALF_WIDTH)
            .mesh()
            .uv(32, 18)
            .scaled_by(Vec3::new(1.0, 1.0, ORB_DEPTH)),
    );
    let glow = |colour: Vec3| StandardMaterial {
        base_color: Color::linear_rgb(colour.x, colour.y, colour.z),
        emissive: LinearRgba::rgb(colour.x, colour.y, colour.z),
        // Unlit, because nothing in this world is lit the standard material's
        // way: every surface the level draws goes through `n64::N64Material`,
        // which carries its own sun. A gate lit by the lights this scene has
        // not got would be a black hoop.
        unlit: true,
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
    // The mast's own two, so the mark under a gate and the mark under a pylon
    // are the same mark. See [`PortalArt::site_clear`].
    let site_clear = surfaces.add(flat(Color::srgba(0.40, 0.95, 1.00, 0.80)));
    let site_blocked = surfaces.add(flat(Color::srgba(1.00, 0.35, 0.30, 0.80)));
    let site = commands
        .spawn((
            PortalSite,
            Transform::default(),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
        ))
        .id();
    commands.entity(site).with_child((
        PortalSiteRing,
        bevy::light::NotShadowCaster,
        // The whistle's annulus, shared with the squad, the machine and the
        // mast, so that every ring this game asks a player to read is one mark.
        Mesh3d(meshes.add(squad::ring_mesh())),
        MeshMaterial3d(site_clear.clone()),
        // Just clear of the ground, the same five centimetres every other ring
        // in this game is lifted by, so it is not half-buried in the slope it
        // is drawn on.
        Transform::from_xyz(0.0, 0.05, 0.0).with_scale(Vec3::new(RADIUS, 1.0, RADIUS)),
    ));
    // One render target per side per frame-parity. See [`NEAR_LAYER`], and
    // [`flip_views`], which does the alternating.
    let target = |images: &mut Assets<Image>| {
        // Sized at anything that is not zero: [`resize`] puts it on the scene
        // target's own size on the first frame, and has to, because the scene
        // target is itself resized off the window.
        let mut image = Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None);
        // The world is drawn at an internal resolution and shown nearest
        // neighbour, and a portal is a piece of the world: a smooth window in
        // a blocky one would read as a different game showing through.
        image.sampler = ImageSampler::nearest();
        images.add(image)
    };
    let mut views: Vec<[Handle<Image>; 2]> = Vec::new();
    let mut openings: Vec<[Handle<PortalMaterial>; 2]> = Vec::new();
    for side in Side::BOTH {
        let pair = [target(&mut images), target(&mut images)];
        let uniform = PortalUniform {
            // The rim is lit from the moment the material exists; what makes an
            // unopened portal invisible is the surface being hidden, which is
            // one thing rather than two saying the same thing.
            rim: side.tint().extend(RIM_GLOW),
            // `z` is the opening's height in half-widths, which is the one
            // number the arch's shape needs: everything above `z - 1` is the
            // semicircular head. See `arch_depth` in `portal.wgsl`, and
            // [`Mouth::depth`], which is the same function on this side of the
            // bind group.
            shape: Vec4::new(RIM_WIDTH, 0.0, HALF_HEIGHT / HALF_WIDTH, 0.0),
        };
        // Which image each of the two wears is [`flip_views`]'s to say, and it
        // says it again every frame; these are somewhere to start.
        let skins = [
            materials.add(PortalMaterial {
                uniform,
                view: Some(pair[0].clone()),
            }),
            materials.add(PortalMaterial {
                uniform,
                view: Some(pair[1].clone()),
            }),
        ];
        views.push(pair.clone());
        openings.push(skins.clone());
        commands.spawn((
            PortalView(side),
            Camera3d::default(),
            RenderTarget::Image(ImageRenderTarget {
                handle: pair[0].clone(),
                // The image is already sized in physical pixels, exactly as
                // the scene target it is kept in step with is.
                scale_factor: 1.0,
            }),
            Camera {
                // Ahead of the world camera at nought, so the picture the world
                // pass samples is this frame's rather than last frame's. The
                // two portal passes are ordered against each other as well,
                // which is what makes the outermost level of the recursion the
                // *current* frame rather than the last one.
                order: -2 + side.index() as isize,
                is_active: false,
                ..default()
            },
            // The world, and the *stale* copy of both gates' surfaces -- which
            // is what this pass may draw without reading what it is writing,
            // and is where the recursion comes from. See [`NEAR_LAYER`].
            RenderLayers::from_layers(&[0, FAR_LAYER]),
            // Replaced wholesale every frame by [`aim_cameras`], which copies
            // the world camera's lens and tilts the near plane onto the far
            // opening. What matters here is only that it is a perspective
            // frame, because that is the arm that system matches on.
            Projection::from(PerspectiveProjection::default()),
            Msaa::Off,
            Tonemapping::None,
            // The medium the camera is in rides along with it, exactly as it
            // does on the world's own -- a portal opening onto the far side of
            // the moat should look like the far side of the moat.
            crate::water::air_fog(),
            // **No `Bloom`, deliberately, and it is the one way a portal is
            // not a window.** The world camera has one, so an emissive thing
            // seen directly halos and the same thing seen through an opening
            // does not: this pass writes an eight-bit target, and the energy
            // above one that the halo is made of is gone by the time the world
            // pass samples it. What it buys is a whole post-process chain per
            // opening per frame on the weakest thing this runs on, for a
            // difference that shows on the orbs and on nothing else. Give this
            // camera the world's `Bloom` and `Hdr` and the difference goes
            // away, at that price.
            Transform::default(),
        ));
        // Two of them standing in the same place, showing the same gate one
        // frame apart. See [`NEAR_LAYER`].
        for near in [true, false] {
            commands.spawn((
                PortalSurface(side),
                Mesh3d(quad.clone()),
                MeshMaterial3d(skins[usize::from(!near)].clone()),
                bevy::light::NotShadowCaster,
                RenderLayers::layer(match near {
                    true => NEAR_LAYER,
                    false => FAR_LAYER,
                }),
                Transform::default(),
                Visibility::Hidden,
            ));
        }
        // The frame is its own entity rather than a child of the surfaces, and
        // that is not tidiness: those live on layers chosen by which camera may
        // draw them (see [`NEAR_LAYER`]), and a frame parented to one would
        // inherit its layer and vanish from half the views it has to be an
        // object in. On the default layer it is drawn by every camera, exactly
        // like the masts and the trees.
        commands.spawn((
            PortalFrame(side),
            Mesh3d(arch.clone()),
            MeshMaterial3d(surfaces.add(glow(side.tint()))),
            bevy::light::NotShadowCaster,
            Transform::default(),
            Visibility::Hidden,
        ));
    }
    commands.insert_resource(PortalArt {
        views: [views[0].clone(), views[1].clone()],
        materials: [openings[0].clone(), openings[1].clone()],
        surfaces: [quad, bubble],
        site_clear,
        site_blocked,
    });
}

/// How many segments the head is drawn with, and how many sides the bar has.
///
/// Twenty-four round the semicircle is a smooth arch at the size these are and
/// two hundred triangles for the whole frame, which is a rounding error against
/// one Mario. Six sides on the bar rather than more: it is a bar a hand's width
/// across seen from several metres, and the silhouette is the semicircle rather
/// than the cross-section.
const ARCH_SEGMENTS: usize = 24;
const BAR_SIDES: usize = 6;

/// The frame: a bar bent into two jambs and a semicircular head, standing on
/// its own two feet.
///
/// **Built here rather than exported from Blender**, which is the opposite of
/// what this project does for actors and is right for this one thing: the arch
/// is [`Mouth::depth`]'s shape, and that function is also in the shader that
/// cuts the opening out. Three copies of one shape in three places, two of them
/// files somebody could re-export, is a frame that stops fitting the hole the
/// first time anybody changes [`HALF_WIDTH`]. Generated from the same two
/// constants, it cannot.
///
/// Laid in the gate's own plane -- the bar's centre line runs along the
/// opening's edge, so half of it overhangs the hole and half stands outside it
/// -- and swept with a circular cross-section of [`FRAME_THICKNESS`].
fn arch_mesh() -> Mesh {
    // The centre line, from one foot up over the head and down to the other,
    // as points paired with the direction pointing out of the opening at each.
    let springing = HALF_HEIGHT - HALF_WIDTH;
    let mut spine: Vec<(Vec2, Vec2)> = Vec::new();
    // Up the left jamb. Two points is enough for a straight run, and the feet
    // are at the ground rather than at the opening's own bottom edge because a
    // gate stands on the ground it was planted on.
    spine.push((Vec2::new(-HALF_WIDTH, -HALF_HEIGHT), Vec2::NEG_X));
    spine.push((Vec2::new(-HALF_WIDTH, springing), Vec2::NEG_X));
    // Over the head, from the left springing round to the right.
    for step in 1..ARCH_SEGMENTS {
        let angle = std::f32::consts::PI * (1.0 - step as f32 / ARCH_SEGMENTS as f32);
        let (sin, cos) = angle.sin_cos();
        spine.push((
            Vec2::new(cos * HALF_WIDTH, springing + sin * HALF_WIDTH),
            Vec2::new(cos, sin),
        ));
    }
    // And down the right one.
    spine.push((Vec2::new(HALF_WIDTH, springing), Vec2::X));
    spine.push((Vec2::new(HALF_WIDTH, -HALF_HEIGHT), Vec2::X));

    let mut positions = Vec::with_capacity(spine.len() * BAR_SIDES);
    let mut normals = Vec::with_capacity(spine.len() * BAR_SIDES);
    let mut uvs = Vec::with_capacity(spine.len() * BAR_SIDES);
    let mut indices = Vec::with_capacity((spine.len() - 1) * BAR_SIDES * 6);
    for (along, (centre, out)) in spine.iter().enumerate() {
        for side in 0..BAR_SIDES {
            let angle = side as f32 / BAR_SIDES as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            // The cross-section is spanned by the outward direction in the
            // gate's own plane and the gate's normal, which is what makes the
            // bar round rather than a ribbon.
            let normal = Vec3::new(out.x * cos, out.y * cos, sin);
            positions
                .push((Vec3::new(centre.x, centre.y, 0.0) + normal * FRAME_THICKNESS).to_array());
            normals.push(normal.to_array());
            uvs.push([
                along as f32 / (spine.len() - 1) as f32,
                side as f32 / BAR_SIDES as f32,
            ]);
        }
    }
    for along in 0..spine.len() - 1 {
        for side in 0..BAR_SIDES {
            let here = (along * BAR_SIDES + side) as u32;
            let next = (along * BAR_SIDES + (side + 1) % BAR_SIDES) as u32;
            let (over, over_next) = (here + BAR_SIDES as u32, next + BAR_SIDES as u32);
            indices.extend_from_slice(&[here, over, next, next, over, over_next]);
        }
    }
    Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(bevy::mesh::Indices::U32(indices))
}

/// How far in from the edge of the opening the rim burns, in half-widths.
const RIM_WIDTH: f32 = 0.09;

/// Keeps the two portal images the size of the one the world is drawn into.
///
/// Taken off the scene target rather than off the window and the display
/// setting, which is the same answer arrived at once instead of twice: the
/// portal shader samples by screen position, so a portal image of a different
/// shape from the world image is a picture stretched across the opening. See
/// [`crate::display::resize`], which is what this follows.
pub fn resize(
    // Both optional, and neither is defensiveness: the headless harness runs
    // this whole schedule with the render plugins stubbed out, so there is no
    // scene target to follow and no art to follow it with. A portal in that
    // world is a pair of positions the pathing and the network read, which is
    // exactly what those tests are about.
    target: Option<Res<SceneTarget>>,
    art: Option<Res<PortalArt>>,
    images: Option<ResMut<Assets<Image>>>,
) {
    let (Some(target), Some(art), Some(mut images)) = (target, art, images) else {
        return;
    };
    let Some(wanted) = images
        .get(&target.0)
        .map(|image| image.texture_descriptor.size)
    else {
        return;
    };
    for view in art.views.iter().flatten() {
        let Some(mut image) = images.get_mut(view) else {
            continue;
        };
        if image.texture_descriptor.size == wanted {
            continue;
        }
        image.resize(wanted);
    }
}

/// Swaps which of a side's two images is being written and which are being
/// read, once a frame.
///
/// **The one moving part of the infinity mirror.** A camera cannot sample the
/// texture it is writing, so each gate keeps two and alternates: this frame's
/// pass writes one while every surface reads the other, and next frame they
/// change places. Nothing is ever both at once, which is what lets a portal
/// pass draw a portal.
///
/// The near surface is then pointed at the image the passes have *just
/// finished*, and the far one at the image from the frame before -- see
/// [`NEAR_LAYER`] for why that split is what keeps the outermost level current
/// while the ones inside it fall behind by a frame apiece.
///
/// Runs before [`show`], so the surfaces are dressed before anything decides
/// whether to show them.
pub fn flip_views(
    art: Option<Res<PortalArt>>,
    materials: Option<ResMut<Assets<PortalMaterial>>>,
    mut cameras: Query<(&PortalView, &mut RenderTarget)>,
    mut written: Local<usize>,
) {
    let (Some(art), Some(mut materials)) = (art, materials) else {
        return;
    };
    // Flipped at the top, so `written` names the image this frame's passes are
    // about to fill rather than the one the last frame left behind.
    *written = 1 - *written;
    let fresh = *written;
    let stale = 1 - fresh;
    for (view, mut target) in &mut cameras {
        let wanted = &art.views[view.0.index()][fresh];
        // Compared before it is written: a `RenderTarget` marked changed is a
        // view Bevy may rebuild, and this runs every drawn frame.
        if target.as_image() == Some(wanted) {
            continue;
        }
        *target = RenderTarget::Image(ImageRenderTarget {
            handle: wanted.clone(),
            scale_factor: 1.0,
        });
    }
    for side in Side::BOTH {
        // The near surface reads what is being written this frame and the far
        // one what was written last, and both are `[near, far]` in that order
        // -- which is the order they were built in and the order the surfaces
        // pick their material by.
        for (which, slot) in [(0usize, fresh), (1, stale)] {
            let wanted = &art.views[side.index()][slot];
            let handle = &art.materials[side.index()][which];
            if materials
                .get(handle)
                .is_some_and(|material| material.view.as_ref() != Some(wanted))
            {
                if let Some(mut material) = materials.get_mut(handle) {
                    material.view = Some(wanted.clone());
                }
            }
        }
    }
}

/// Puts the two quads where the two openings are, and switches them on.
pub fn show(
    portals: Res<Portals>,
    art: Option<Res<PortalArt>>,
    // Optional for [`resize`]'s reason: the material collection is
    // [`PortalPlugin`]'s, and the headless harness runs this schedule without
    // any of the render plugins in it.
    materials: Option<ResMut<Assets<PortalMaterial>>>,
    mut surfaces: Query<
        (&PortalSurface, &mut Transform, &mut Visibility, &mut Mesh3d),
        Without<PortalFrame>,
    >,
    mut frames: Query<(&PortalFrame, &mut Transform, &mut Visibility), Without<PortalSurface>>,
) {
    let (Some(art), Some(mut materials)) = (art, materials) else {
        return;
    };
    // The frames first, because there is nothing conditional about them: a gate
    // that is standing has its frame whether or not it has a far side to show.
    // A bubble has none at all -- its own limb is its edge -- so what stands
    // there is hidden rather than empty.
    for (frame, mut transform, mut visible) in &mut frames {
        match portals.mouth(frame.0) {
            Some(mouth) if mouth.shape == Shape::Arch => {
                *transform = mouth.transform();
                *visible = Visibility::Visible;
            }
            _ => *visible = Visibility::Hidden,
        }
    }
    let paired = portals.open();
    for (surface, mut transform, mut visible, mut mesh) in &mut surfaces {
        let side = surface.0;
        match portals.mouth(side) {
            Some(mouth) => {
                *transform = mouth.transform();
                // Only once there is a far side. A lone gate is a frame you can
                // see the lawn through, which is the honest picture of one: it
                // is not a door yet.
                *visible = match paired {
                    true => Visibility::Visible,
                    false => Visibility::Hidden,
                };
                // The shape it is built as, compared before it is written for
                // the reason the uniform below is: a `Mesh3d` written every
                // frame is a change tick every frame, and every system watching
                // this entity woken for it.
                let wanted = &art.surfaces[usize::from(mouth.shape == Shape::Orb)];
                if mesh.0.id() != wanted.id() {
                    mesh.0 = wanted.clone();
                }
            }
            None => *visible = Visibility::Hidden,
        }
        // The picture is shown only once there is a far side to have taken
        // one; an opening on its own is the ring alone. See `portal.wgsl`.
        //
        // Compared before it is written, and not as tidiness: `get_mut` on an
        // asset marks it changed whether or not the value moved, and a material
        // marked changed is a uniform re-uploaded to the GPU. This runs every
        // drawn frame and the answer changes twice a session.
        let shown = f32::from(u8::from(paired));
        let built = portals.mouth(side).map_or(0.0, |mouth| mouth.shape.key());
        // Both of a side's materials, because both surfaces are the same gate
        // and a level of the recursion that disagreed about its own shape would
        // be the one place the illusion comes apart.
        for handle in &art.materials[side.index()] {
            if !materials.get(handle).is_some_and(|material| {
                material.uniform.shape.y != shown || material.uniform.shape.w != built
            }) {
                continue;
            }
            // Written through `get_mut` rather than by replacing the handle, so
            // the material the surface is already holding is the one that
            // changes.
            if let Some(mut material) = materials.get_mut(handle) {
                material.uniform.shape.y = shown;
                material.uniform.shape.w = built;
            }
        }
    }
}

/// How brightly the ring burns. Above one, so the camera's bloom spills it onto
/// the hoop and the ground under it -- which, with the hoop itself edge-on, is
/// the only cue that the picture inside the frame is not simply more lawn.
const RIM_GLOW: f32 = 1.6;

/// Stands each portal camera behind the far opening, at the player's own eye
/// carried through the pair, and tilts its near plane onto that opening.
///
/// **Everything about how a portal looks is these fifteen lines.** The picture
/// is right when the far camera's eye is the near camera's eye reflected
/// through the pair, when its field of view is the same, and when its near
/// plane is the far gate; get any of the three wrong and the window reads as a
/// screen showing a video of somewhere else.
///
/// In `PostUpdate` and before Bevy propagates transforms, because the transform
/// this writes has to be propagated in the same frame it is written -- and
/// after [`crate::camera::update`], which is what moves the eye it is derived
/// from. A frame of lag here is a gate whose contents swim inside their own
/// hoop when the player turns.
#[allow(clippy::type_complexity)]
pub fn aim_cameras(
    portals: Res<Portals>,
    eye: Query<(&Transform, &Projection), (With<crate::camera::FollowCamera>, Without<PortalView>)>,
    mut cameras: Query<(&PortalView, &mut Transform, &mut Projection, &mut Camera)>,
) {
    let Ok((eye, lens)) = eye.single() else {
        return;
    };
    for (view, mut transform, mut projection, mut camera) in &mut cameras {
        let side = view.0;
        // The pair, in the order this camera needs it: the opening it feeds is
        // the one being *looked through*, so it is the entry, and the picture
        // is taken from behind the other one.
        let Some((entry, exit)) = portals.pair(side) else {
            camera.is_active = false;
            continue;
        };
        // **A live portal camera is a whole second pass over the world**, and
        // on the machine this game is meant to run on that is the most
        // expensive thing in the feature by a wide margin. So it is switched
        // off for every opening the player is not, right now, in a position to
        // see anything through.
        //
        // Three cheap tests and no ray casts. **Behind it:** the quad is
        // back-face culled, so an opening seen from its own back is already
        // invisible. **Behind him:** a portal past the eye's own plane is off
        // the screen, with a stile's width of slack so one at the very edge is
        // not switched off half way through being looked at. **Too far:** past
        // the distance the fog closes over, an opening is a few pixels of ring
        // and the picture inside it could be anything.
        let toward = entry.at - eye.translation;
        let facing = toward.dot(entry.normal()) < 0.0;
        let ahead = toward.dot(Vec3::from(eye.forward())) > -HALF_WIDTH;
        let near = toward.length_squared() < crate::water::SIGHT * crate::water::SIGHT;
        camera.is_active = facing && ahead && near;
        if !camera.is_active {
            continue;
        }
        let carried = Mouth::through(entry, exit) * eye.to_matrix();
        let (_, rotation, translation) = carried.to_scale_rotation_translation();
        transform.translation = translation;
        transform.rotation = rotation;
        let (Projection::Perspective(mine), Projection::Perspective(theirs)) =
            (projection.as_mut(), lens)
        else {
            continue;
        };
        let wanted = lens_for(theirs, exit, &transform);
        mine.fov = wanted.fov;
        mine.near = wanted.near;
        mine.far = wanted.far;
        mine.near_clip_plane = wanted.near_clip_plane;
    }
}

/// Flies the camera through the gate the boom goes through, and points it back
/// the way it came.
///
/// **This is what makes walking through one continuous rather than a cut**, and
/// it is worth being precise about why, because the position it writes is a
/// jump and the *picture* is not.
///
/// Walk at a gate. The boom is behind you, pointing away from it, so nothing
/// happens. You cross, and now you are standing in front of the far gate with
/// the boom pointing back at it -- through its doorway. So the camera is flown
/// through to the near gate's side and lands exactly where it already was, a
/// frame ago, behind where you were standing. It does not move at all. What
/// changes is what it can see: you are no longer in front of it, you are
/// through the gate it is looking at, and the gate's opening is showing you.
///
/// Then you walk on, the boom stops reaching the doorway, and the camera stops
/// being flown -- which *is* a jump, from one side of the pair to the other.
/// It is also invisible, because it happens at the instant the camera is in the
/// gate's own plane: at that moment its whole frustum is the opening, and the
/// view through the opening is by construction the view from the other side.
/// The camera passes through the doorway. That the arch has to be wide enough
/// for the frustum to be inside it when that happens is why [`HALF_WIDTH`] is a
/// camera's measurement rather than a body's.
///
/// # Which transform is which
///
/// The camera entity's `Transform` is the *rendered* eye, and it is what
/// everything downstream of here reads: the billboards turn to it, the
/// impostors pick an angle by it, the health bars project through it, and the
/// portal cameras themselves are derived from it -- all correctly, because all
/// of them are asking where the picture is taken from.
///
/// Everything that *aims* has to ask the other question, and gets the answer
/// from [`crate::camera::FollowCamera::eye`]: the crosshair belongs beside the
/// player rather than on the far side of the map, and a shot laid from the
/// flown camera would be a shot at the back of a gate. [`release_camera`] puts
/// the logical eye back on the entity at the top of each frame, so the fixed
/// step -- which reads the camera for the movement basis -- never sees a flown
/// one either.
pub fn carry_camera(
    portals: Res<Portals>,
    mut camera: Query<(&mut Transform, &crate::camera::FollowCamera), Without<PortalView>>,
) {
    let Ok((mut transform, follow)) = camera.single_mut() else {
        return;
    };
    let Some(focus) = follow.focus else {
        return;
    };
    let Some(carry) = portals.boom_through(focus, transform.translation) else {
        return;
    };
    let (_, rotation, translation) =
        (carry * transform.to_matrix()).to_scale_rotation_translation();
    transform.translation = translation;
    transform.rotation = rotation;
}

/// Puts the logical eye back on the camera entity before the tick reads it.
///
/// In `First`, which is before both the fixed step and `camera::update`. See
/// [`carry_camera`] for what it is undoing and why the undoing is not optional:
/// [`crate::player::movement`] takes the movement basis off this transform, and
/// a player whose forward key ran toward a gate on the far side of the castle
/// for the second after every transit would be a player who cannot walk away
/// from one.
pub fn release_camera(
    mut camera: Query<(&mut Transform, &crate::camera::FollowCamera), Without<PortalView>>,
) {
    let Ok((mut transform, follow)) = camera.single_mut() else {
        return;
    };
    // Nothing to put back until the camera has been placed once. On the very
    // first frame `eye` is still the identity, and restoring *that* would drop
    // the camera at the origin for the one tick before `camera::update` runs --
    // which is the tick the fixed step takes its movement basis from. The focus
    // is the flag for it because it is `None` for exactly as long.
    if follow.focus.is_none() {
        return;
    }
    *transform = follow.eye;
}

/// Registers the portal surface material and embeds its shader.
pub struct PortalPlugin;

impl Plugin for PortalPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "portal.wgsl");
        app.add_plugins(MaterialPlugin::<PortalMaterial>::default());
    }
}

/// The per-frame half, in the order one frame does it.
///
/// `turn_camera` first and before [`crate::camera::update`] in the same frame,
/// so a player who went through on the tick just gone is framed looking out of
/// the exit rather than swinging round to it a frame later.
pub fn systems() -> bevy::ecs::schedule::ScheduleConfigs<bevy::ecs::system::ScheduleSystem> {
    (turn_camera, place, ghost, flip_views, show, resize).chain()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::CameraProjection;

    /// A gate stated as a point and a facing, which is what the geometry is
    /// about: none of it cares where the ground is.
    fn mouth(at: Vec3, normal: Vec3) -> Mouth {
        Mouth::built(at, normal, Shape::Arch).expect("a normal that is a direction")
    }

    /// The frame is a right-handed one with +Z out of the gate's face.
    ///
    /// Every other piece of geometry in the module is stated in those terms, so
    /// getting the handedness wrong here is getting all of it wrong at once --
    /// and the symptom is a portal whose picture is mirrored, which is subtle
    /// enough to live in a build for a while.
    #[test]
    fn the_frame_points_out_of_the_face() {
        let facing = mouth(Vec3::new(3.0, 1.5, 0.0), Vec3::X);
        assert!(facing.normal().distance(Vec3::X) < 1e-5);
        assert!(facing.up().distance(Vec3::Y) < 1e-5);
        // Right-handed: across cross up is out.
        assert!(
            facing.right().cross(facing.up()).distance(facing.normal()) < 1e-5,
            "{:?}",
            facing.right()
        );
        // Laid flat there is no up the face, so world north stands in -- and
        // the frame is still right-handed. Nothing plants a gate that way, but
        // the arithmetic below it never assumed upright and this is what says
        // so.
        let floor = mouth(Vec3::ZERO, Vec3::Y);
        assert!(floor.normal().distance(Vec3::Y) < 1e-5);
        assert!(floor.right().cross(floor.up()).distance(floor.normal()) < 1e-5);
    }

    /// A body walking into the front of one mouth walks out of the front of
    /// the other.
    ///
    /// **The half turn in [`Mouth::through`] is the whole of this.** Without
    /// it the map is the exit's frame applied to the entry's, and what that
    /// produces is a body arriving at the exit travelling backwards through the
    /// exit's own hoop -- which is not a gate, it is a trap.
    #[test]
    fn walking_into_one_mouth_is_walking_out_of_the_other() {
        // Two gates facing each other down a corridor, forty metres apart.
        let west = mouth(Vec3::new(-20.0, 1.5, 0.0), Vec3::X);
        let east = mouth(Vec3::new(20.0, 1.5, 0.0), Vec3::NEG_X);
        let carry = Mouth::through(&west, &east);

        // The middle of one is the middle of the other.
        assert!(carry.transform_point3(west.at).distance(east.at) < 1e-4);

        // And a body walking *into* the west gate -- which is travelling along
        // that gate's own -Z, west, since its face looks east down the corridor
        // -- comes out travelling along the east gate's +Z, which is also west,
        // and which is out of its face rather than backwards through its own
        // hoop. Two gates facing each other down a corridor do not turn
        // anybody, and that is the right answer for them.
        let turn = Quat::from_mat4(&carry);
        let arriving = turn * -west.normal();
        assert!(
            arriving.distance(east.normal()) < 1e-4,
            "came out travelling {arriving:?} rather than {:?}",
            east.normal()
        );
        // Never backwards, which is the failure the half turn exists for.
        assert!(arriving.dot(east.normal()) > 0.0);
    }

    /// Two gates at right angles are still a pair, and the turn is a turn.
    #[test]
    fn a_pair_at_right_angles_turns_the_body_with_it() {
        let north = mouth(Vec3::new(0.0, 1.5, -10.0), Vec3::Z);
        let east = mouth(Vec3::new(10.0, 1.5, 0.0), Vec3::NEG_X);
        let turn = Quat::from_mat4(&Mouth::through(&north, &east));
        // Into the north gate is travelling along -Z; out of the east one is
        // travelling along -X.
        let arriving = turn * Vec3::NEG_Z;
        assert!(arriving.distance(Vec3::NEG_X) < 1e-4, "{arriving:?}");
    }

    /// A step counts as going through only when it crosses the opening.
    #[test]
    fn a_crossing_is_a_step_through_the_opening_and_nothing_else() {
        let gate = mouth(Vec3::new(0.0, 1.2, 0.0), Vec3::Z);
        let front = |z: f32| Vec3::new(0.0, 1.2, z);

        // Straight through the middle.
        assert!(gate.crossing(front(0.4), front(-0.4)).is_some());
        // The other way, which is a body that has just come *out*. Refused, and
        // that refusal is what stops a pair swallowing what it has delivered.
        assert!(gate.crossing(front(-0.4), front(0.4)).is_none());
        // Up to the gate and no further.
        assert!(gate.crossing(front(0.8), front(0.05)).is_none());
        // Through the plane, but past the edge of the opening: beside a gate
        // is beside it, not through it.
        let beside = Vec3::new(HALF_WIDTH + 0.4, 1.2, 0.0);
        assert!(gate
            .crossing(beside + Vec3::Z * 0.4, beside - Vec3::Z * 0.4)
            .is_none());
        // And above it.
        let over = Vec3::new(0.0, 1.2 + HALF_HEIGHT + 0.4, 0.0);
        assert!(gate
            .crossing(over + Vec3::Z * 0.4, over - Vec3::Z * 0.4)
            .is_none());
        // A stride so long it clears the gate in one tick is still caught,
        // which is the whole reason this is a segment test and not a position
        // test. See [`Transit`].
        assert!(gate.crossing(front(6.0), front(-6.0)).is_some());
    }

    /// The tilted frame clips at the portal's plane rather than at a depth.
    ///
    /// Stated on the matrix rather than on a picture: a point in front of the
    /// plane lands inside the near clip and a point behind it lands outside,
    /// whatever their distance from the camera. That is the property the
    /// picture rests on -- see [`clip_plane`] -- and it is the one a screenshot
    /// cannot tell you about: the failure is a gate full of the ground behind
    /// it, and a gate on a lawn is full of ground either way.
    #[test]
    fn the_far_camera_clips_at_the_opening_rather_than_at_a_distance() {
        // A camera at the origin looking down -Z, and an exit whose surface is
        // ten metres away, square to the view.
        let camera = Transform::IDENTITY;
        let exit = mouth(Vec3::new(0.0, 0.0, -10.0), Vec3::NEG_Z);
        let world = PerspectiveProjection {
            fov: 60_f32.to_radians(),
            near: 0.05,
            far: 1000.0,
            aspect_ratio: 1.0,
            ..default()
        };
        // Reverse-Z: the near plane is where the depth reaches one, and
        // anything nearer than it is over one and so clipped.
        let depth = |lens: &PerspectiveProjection, at: Vec3| {
            let clip = lens.get_clip_from_view() * at.extend(1.0);
            clip.z / clip.w
        };

        // Square on, which is the case Bevy's own oblique frame short-circuits
        // and this module has to carry the distance for itself. See `lens_for`.
        let square = lens_for(&world, &exit, &camera);
        assert!(depth(&square, Vec3::new(0.0, 0.0, -1.0)) > 1.0);
        assert!(depth(&square, Vec3::new(0.0, 0.0, -11.0)) < 1.0);

        // And at an angle, which is every other portal there is: an opening on
        // a gate turned away from the camera. The point beyond its plane on one
        // side is kept and the point in front of that plane on the other side is
        // clipped -- and no plain near plane, at any distance, can do both.
        let slanted = mouth(
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(1.0, 0.0, -1.0).normalize(),
        );
        let lens = lens_for(&world, &slanted, &camera);
        // Out through the opening, which is the room the far side of the pair
        // looks onto: kept.
        let beyond = slanted.at + slanted.normal() * 2.0;
        assert!(depth(&lens, beyond) < 1.0, "{}", depth(&lens, beyond));
        // And a point on the camera's side of the same plane, further along it
        // -- which is **further from the camera** than the kept point is. That
        // is the whole case: no near plane parallel to the screen, at any
        // distance, can keep the first of these and clip the second, because
        // the second is the further away of the two.
        let behind = slanted.at + slanted.right() * 20.0 - slanted.normal() * 2.0;
        assert!(
            behind.length() > beyond.length(),
            "{behind:?} is not further off than {beyond:?}"
        );
        assert!(depth(&lens, behind) > 1.0, "{}", depth(&lens, behind));
        // The ordering still runs the right way, which is what makes the depth
        // buffer usable at all: further away is a smaller number.
        let along = |t: f32| slanted.at + slanted.normal() * t;
        assert!(depth(&lens, along(40.0)) < depth(&lens, along(2.0)));
        assert!(depth(&lens, along(400.0)) < depth(&lens, along(40.0)));

        // A camera that has ended up in *front* of the opening gets no plane
        // at all rather than a frame with the sign flipped.
        let ahead = Transform::from_xyz(0.0, 0.0, -20.0);
        assert!(clip_plane(&exit, &ahead).is_none());
        let plain = lens_for(&world, &exit, &ahead);
        assert_eq!(plain.near, world.near);
        assert_eq!(plain.near_clip_plane, no_clip_plane(world.near));
    }

    /// The opening is a doorway: straight jambs, a round head, no sill.
    ///
    /// The shape is written twice -- here and in `portal.wgsl`, which cuts the
    /// picture out with it -- so these are the properties both copies have to
    /// have. Get them apart and a body walks through air the shader drew as
    /// frame, or is stopped by nothing where the player can see straight
    /// through.
    #[test]
    fn the_opening_is_an_arch_rather_than_a_hole() {
        let gate = mouth(Vec3::new(0.0, HALF_HEIGHT, 0.0), Vec3::Z);
        let at =
            |across: f32, up: f32| gate.at + gate.right() * across + gate.up() * (up - HALF_HEIGHT);
        // The middle is as far inside as anything gets.
        assert!(gate.inside(at(0.0, HALF_HEIGHT)));
        // The jambs are straight: at any height under the springing line the
        // opening is the full width and not a pinch narrower.
        let springing = HALF_HEIGHT - HALF_WIDTH;
        for up in [0.05, 0.5, 1.0, springing + HALF_HEIGHT - 0.01] {
            let up = up.min(springing + HALF_HEIGHT);
            assert!(gate.inside(at(HALF_WIDTH * 0.98, up)), "{up}");
            assert!(!gate.inside(at(HALF_WIDTH * 1.02, up)), "{up}");
        }
        // **No sill.** A doorway's jambs run into the ground, so the very
        // bottom of the opening is as wide as the middle of it -- which is what
        // lets a body walk in rather than step over.
        assert!(gate.inside(at(0.0, 0.001)));
        assert!(gate.inside(at(HALF_WIDTH * 0.9, 0.001)));
        // The head is a semicircle of the half-width, so the very top is a
        // point and the corners beside it are outside.
        let top = HALF_HEIGHT * 2.0;
        assert!(gate.inside(at(0.0, top - 0.02)));
        assert!(!gate.inside(at(0.0, top + 0.02)));
        assert!(!gate.inside(at(HALF_WIDTH * 0.9, top - 0.02)));
        // And it is *round* rather than stretched: forty-five degrees round the
        // head from the springing is the half-width over root two, in metres,
        // on both axes at once.
        let corner = HALF_WIDTH * std::f32::consts::FRAC_1_SQRT_2;
        let head = HALF_HEIGHT + springing;
        assert!(gate.inside(at(corner * 0.95, head + corner * 0.95)));
        assert!(!gate.inside(at(corner * 1.05, head + corner * 1.05)));
    }

    /// A bubble is the same gate wearing a different skin.
    ///
    /// The point of the variant is that it *is* a variant: one threshold, one
    /// map to the far side, one edge on the navigation grid. What differs is
    /// the shape of the opening and how far off the ground the middle of it
    /// sits, and this is the list of everything that follows from those two.
    #[test]
    fn a_bubble_is_the_same_gate_in_a_different_shape() {
        let orb = Mouth::built(Vec3::new(0.0, HALF_WIDTH, 0.0), Vec3::Z, Shape::Orb)
            .expect("a direction");
        let at = |across: f32, up: f32| orb.at + orb.right() * across + orb.up() * up;

        // It rests on the ground: the middle is its own radius up, where an
        // arch's is half its height up.
        assert!(orb.foot().distance(Vec3::ZERO) < 1e-4, "{:?}", orb.foot());
        assert_eq!(Shape::Orb.rise(), HALF_WIDTH);
        assert_eq!(Shape::Arch.rise(), HALF_HEIGHT);

        // The opening is a disc rather than a doorway, so it narrows towards
        // the top and bottom where an arch's jambs stay parallel.
        assert!(orb.inside(at(0.0, 0.0)));
        assert!(orb.inside(at(HALF_WIDTH * 0.98, 0.0)));
        assert!(!orb.inside(at(HALF_WIDTH * 1.02, 0.0)));
        assert!(!orb.inside(at(HALF_WIDTH * 0.9, HALF_WIDTH * 0.9)));
        let arch = mouth(Vec3::new(0.0, HALF_HEIGHT, 0.0), Vec3::Z);
        assert!(
            arch.inside(
                arch.at + arch.right() * (HALF_WIDTH * 0.9) - arch.up() * (HALF_HEIGHT * 0.9)
            ),
            "an arch's jambs are not parallel"
        );

        // And everything else is the arch's, unchanged: a step through the skin
        // is carried, and it is carried by the same map.
        let far = Mouth::built(Vec3::new(50.0, HALF_WIDTH, 0.0), Vec3::X, Shape::Orb).unwrap();
        assert!(orb
            .crossing(at(0.0, 0.0) + orb.normal(), at(0.0, 0.0) - orb.normal())
            .is_some());
        assert!(orb
            .crossing(
                at(HALF_WIDTH * 1.4, 0.0) + orb.normal(),
                at(HALF_WIDTH * 1.4, 0.0) - orb.normal()
            )
            .is_none());
        let turn = Quat::from_mat4(&Mouth::through(&orb, &far));
        assert!((turn * -orb.normal()).distance(far.normal()) < 1e-4);
    }

    /// A pair is one shape, and planting the other kind starts a fresh one.
    ///
    /// The two shapes put the middle of the opening at different heights off
    /// the ground, so a body walking into an arch and out of a bubble would
    /// arrive at the bubble's centre -- its own radius up in the air. Rather
    /// than teach the map about mismatched ends, the odd one out goes.
    #[test]
    fn planting_the_other_shape_starts_the_pair_again() {
        let mut portals = Portals::default();
        let gate = |x: f32, shape: Shape| {
            Mouth::built(Vec3::new(x, shape.rise(), 0.0), Vec3::Z, shape).expect("a direction")
        };
        portals.plant(gate(0.0, Shape::Arch));
        portals.plant(gate(40.0, Shape::Arch));
        assert!(portals.open());

        // A bubble while two arches stand: the arch that was not about to be
        // replaced goes, and this is the first of a new pair.
        assert_eq!(portals.plant(gate(80.0, Shape::Orb)), Side::Blue);
        assert!(!portals.open(), "a mismatched pair was left standing");
        assert_eq!(portals.mouth(Side::Blue).unwrap().shape, Shape::Orb);
        // And the second bubble completes it, as the second of anything does.
        assert_eq!(portals.plant(gate(120.0, Shape::Orb)), Side::Orange);
        assert!(portals.open());
        for side in Side::BOTH {
            assert_eq!(portals.mouth(side).unwrap().shape, Shape::Orb);
        }
    }

    /// The camera is big enough to come through with the player.
    ///
    /// Not a shape test but a *size* one, and it is the reason the arch is a
    /// city gate rather than a door: the walk-through only reads as continuous
    /// if the boom passes through the opening too, and a boom hangs several
    /// metres up and behind. See [`HALF_WIDTH`].
    #[test]
    fn a_boom_behind_the_player_comes_through_the_doorway() {
        let gate = mouth(Vec3::new(0.0, HALF_HEIGHT, 0.0), Vec3::Z);
        let far = mouth(Vec3::new(100.0, HALF_HEIGHT, 0.0), Vec3::Z);
        let mut portals = Portals::default();
        portals.set(Side::Blue, Some(gate));
        portals.set(Side::Orange, Some(far));
        // The player a hand's breadth out of the doorway, having just come
        // through, with the camera on a nine-metre boom behind and a little
        // above him -- the game's own default framing. Behind him is back
        // through the gate.
        let focus = gate.at + gate.normal() * 0.05 - gate.up() * (HALF_HEIGHT - 1.4);
        let boom = -gate.normal() * 9.0 + gate.up() * 0.76;
        assert_eq!(
            portals.clearance(focus, boom),
            1.0,
            "the boom was cut short by a doorway it goes through"
        );
        assert!(
            portals.boom_through(focus, focus + boom).is_some(),
            "the camera is behind the gate and was not flown through it"
        );
        // Stepped a body's width to one side of the jamb, the same boom passes
        // behind the gate rather than through it -- and is pulled up short
        // rather than left looking at the back of one.
        let beside = focus + gate.right() * (HALF_WIDTH + 1.0);
        let cut = portals.clearance(beside, boom);
        assert!(cut < 1.0, "the boom slid behind the gate: {cut}");
        assert!(
            portals.boom_through(beside, beside + boom).is_none(),
            "a boom that missed the doorway was flown through it anyway"
        );
    }

    /// Flying the camera through leaves it exactly where it already was.
    ///
    /// **The whole of why a transit is not a cut.** A step through the doorway
    /// moves the player from one gate to the other; the boom then reaches back
    /// through the gate he came out of, and the map that carries it is the
    /// inverse of the one that carried him. So the camera does not move on the
    /// frame the player does -- what moves is which side of the pair the player
    /// is on, and the opening is what shows him.
    #[test]
    fn the_camera_does_not_move_on_the_frame_the_player_does() {
        let near = mouth(Vec3::new(0.0, HALF_HEIGHT, 0.0), Vec3::Z);
        let far = mouth(Vec3::new(80.0, HALF_HEIGHT, 40.0), Vec3::X);
        let mut portals = Portals::default();
        portals.set(Side::Blue, Some(near));
        portals.set(Side::Orange, Some(far));

        // The frame he steps through: the middle of him is a hand's breadth
        // *past* the near gate's plane, and the camera is on a boom behind him
        // -- which is to say still out in front of the gate.
        let focus = near.at - near.normal() * 0.05 - near.up() * (HALF_HEIGHT - 1.4);
        let camera = focus + near.normal() * 8.0 + near.up() * 0.7;
        // The boom runs from behind the plane to in front of it, which is not a
        // way through a doorway: nothing is flown yet.
        assert!(portals.boom_through(focus, camera).is_none());

        // He is carried. So is the focus, by the same map, and so -- because
        // the boom is rebuilt from the focus and a yaw that was carried with it
        // -- is where the camera wants to be.
        let carry = Mouth::through(&near, &far);
        let (moved, wanted) = (
            carry.transform_point3(focus),
            carry.transform_point3(camera),
        );
        // He is now in front of the far gate and the boom reaches back through
        // it. What it is flown to is the place it was already standing.
        let flown = portals
            .boom_through(moved, wanted)
            .expect("the boom does not reach back through the gate he came out of");
        let landed = flown.transform_point3(wanted);
        assert!(
            landed.distance(camera) < 1e-3,
            "the camera jumped {} m on the frame he stepped through",
            landed.distance(camera)
        );
    }

    /// There are two of him while he is in the doorway, and one otherwise.
    ///
    /// The ghost is what makes a transit read as walking through rather than as
    /// blinking, and everything about it that can be got wrong is here: whether
    /// it is showing at all, where it stands, and which of the two characters
    /// it is. What it *looks* like -- half a body either side of a threshold --
    /// is geometry rather than code, and is the opaque opening and the far
    /// camera's tilted near plane doing their own jobs. See [`Ghost`].
    #[test]
    fn there_are_two_of_him_while_he_is_in_the_doorway() {
        use bevy::ecs::system::RunSystemOnce;

        let near = mouth(Vec3::new(0.0, HALF_HEIGHT, 0.0), Vec3::Z);
        let far = mouth(Vec3::new(60.0, HALF_HEIGHT, 20.0), Vec3::X);
        let mut world = World::new();
        let mut portals = Portals::default();
        portals.set(Side::Blue, Some(near));
        portals.set(Side::Orange, Some(far));
        world.insert_resource(portals);
        world.insert_resource(crate::GameState {
            active: crate::ActiveCharacter::Luna,
            aiming: false,
            debug: false,
        });
        world.insert_resource(crate::player::RenderPose {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        });
        let ghosts: Vec<Entity> = crate::ActiveCharacter::ALL
            .iter()
            .map(|character| {
                world
                    .spawn((Ghost, *character, Transform::default(), Visibility::Hidden))
                    .id()
            })
            .collect();
        let run = |world: &mut World, at: Vec3| {
            world
                .resource_mut::<crate::player::RenderPose>()
                .translation = at;
            world
                .run_system_once(ghost)
                .expect("the ghost would not run");
        };
        let seen = |world: &World, which: usize| {
            (
                *world.get::<Visibility>(ghosts[which]).unwrap(),
                world.get::<Transform>(ghosts[which]).unwrap().translation,
            )
        };

        // Out on the lawn there is one of him.
        run(
            &mut world,
            near.at + near.normal() * 20.0 - Vec3::Y * HALF_HEIGHT,
        );
        assert_eq!(seen(&world, 0).0, Visibility::Hidden);

        // In the doorway there are two, and the second one is standing where
        // the first already is on the other side.
        let standing = near.at - Vec3::Y * (HALF_HEIGHT - TRANSIT_LIFT) - near.normal() * 0.1;
        run(&mut world, standing - Vec3::Y * TRANSIT_LIFT);
        let (visible, at) = seen(&world, 0);
        assert_eq!(
            visible,
            Visibility::Visible,
            "he walked into a doorway alone"
        );
        let wanted =
            Mouth::through(&near, &far).transform_point3(standing - Vec3::Y * TRANSIT_LIFT);
        assert!(at.distance(wanted) < 1e-3, "{at:?} vs {wanted:?}");
        // And the other character has no ghost: there is one player.
        assert_eq!(seen(&world, 1).0, Visibility::Hidden);

        // Beside the gate rather than in it, there is one of him again -- the
        // jamb is not a doorway.
        run(
            &mut world,
            standing + near.right() * (HALF_WIDTH + 1.0) - Vec3::Y * TRANSIT_LIFT,
        );
        assert_eq!(seen(&world, 0).0, Visibility::Hidden);

        // And a stride clear of the plane, likewise: a ghost that lingered
        // would be a second player following him about.
        run(
            &mut world,
            standing + near.normal() * (STRADDLE_DEPTH + 0.5) - Vec3::Y * TRANSIT_LIFT,
        );
        assert_eq!(seen(&world, 0).0, Visibility::Hidden);
    }

    /// A gate is only walkable where a body could actually walk into it.
    ///
    /// The rule earns its keep at the edges of the map rather than in the
    /// middle of the lawn: a gate planted on the lip of a drop is a gate the
    /// crowd would be routed at and then walked off. What comes back from
    /// [`Mouth::warp`] is what the navigation grid hangs its edge on, so a gate
    /// with no answer here is a gate the pathing does not know about at all --
    /// while still being one you can see through and walk through yourself.
    #[test]
    fn a_gate_is_walkable_only_where_there_is_ground_to_walk_in_from() {
        let (level, _) = crate::level::load();
        // Somewhere on the castle lawn with ground under it.
        let ground = Vec3::new(-13.28, 2.6, 46.64);
        let (height, _) = level
            .ground_at(ground + Vec3::Y * 40.0)
            .expect("no lawn to stand on");
        let foot = Vec3::new(ground.x, height, ground.z);
        let standing =
            Mouth::standing(&level, foot, 0.0, Shape::Arch).expect("a gate on open lawn");
        // Upright, standing on its own footing, with the middle of the opening
        // a chest's height up -- which is what makes it a door.
        assert!(
            standing.foot().distance(foot) < 1e-4,
            "{:?}",
            standing.foot()
        );
        assert!(standing.up().distance(Vec3::Y) < 1e-4);
        assert!((standing.at.y - height - HALF_HEIGHT).abs() < 1e-4);
        let stand = standing
            .walkable()
            .expect("a gate on open lawn has nowhere to walk in from");
        assert!((stand.y - height).abs() < 1.0, "{stand:?}");
        // The spot is in front of its face, which is the side that is a door.
        assert!((stand - standing.at).dot(standing.normal()) > 0.0);

        // Out over the sea there is no ground at all, so there is nothing to
        // walk in from -- and the gate still exists, because being unreachable
        // by a crowd is not the same as not being there.
        let (low, high) = level.bounds();
        let outside = Vec3::new(high.x + 60.0, 0.0, low.y - 60.0);
        let stranded =
            Mouth::standing(&level, outside, 0.0, Shape::Arch).expect("a gate is still a gate");
        assert!(
            stranded.walkable().is_none(),
            "a gate over nothing was wired into the pathing"
        );
        assert!(stranded.warp().is_none());
    }

    /// A pair the crowd can use needs *both* ends walkable.
    ///
    /// A one-way edge on the navigation grid is a route the crowd walks into
    /// and cannot come back out of, so [`Portals::walkway`] refuses the pair
    /// rather than wiring half of it.
    #[test]
    fn a_pair_is_wired_into_the_pathing_only_when_both_ends_are() {
        let (level, _) = crate::level::load();
        let ground = Vec3::new(-13.28, 2.6, 46.64);
        let (height, _) = level.ground_at(ground + Vec3::Y * 40.0).expect("no lawn");
        let here = Mouth::standing(
            &level,
            Vec3::new(ground.x, height, ground.z),
            0.0,
            Shape::Arch,
        )
        .unwrap();
        let (low, high) = level.bounds();
        let nowhere = Mouth::standing(
            &level,
            Vec3::new(high.x + 60.0, 0.0, low.y - 60.0),
            0.0,
            Shape::Arch,
        )
        .unwrap();

        let mut portals = Portals::default();
        portals.set(Side::Blue, Some(here));
        portals.set(Side::Orange, Some(nowhere));
        assert!(portals.open(), "both ends are standing");
        assert!(
            portals.walkway().is_none(),
            "half a walkable pair was hung on the grid"
        );
        // The beams do not care: light does not need somewhere to stand.
        assert!(portals.optics().is_some());

        portals.set(Side::Orange, Some(here));
        assert!(portals.walkway().is_some());
    }
}
