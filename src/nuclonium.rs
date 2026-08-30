//! Nuclonium: what a dead enemy leaves behind, and the chain that carries it
//! home.
//!
//! **Nuclonium is the substance; a ball is only what it looks like today.**
//! One is drawn as a small glowing green sphere because that reads at a
//! distance and costs two entities, and everything in this module is written
//! about *a unit of nuclonium* rather than about a sphere -- which is what will
//! let the ore patches `next.md` wants (a seam in the ground, mined by a Mario
//! standing over it rather than dropped by a corpse) hand their output to the
//! same three legs below without any of them changing.
//!
//! Those three legs:
//!
//!   1. **A kill drops one, one time in twenty.** Enemies die in four places --
//!      the sword, the stomp, a Mario's fist and a bullet -- and all four call
//!      [`Drops::maybe`], which is the only thing in the game that decides
//!      whether a ball appears. See it for why five percent is a *Weyl
//!      sequence* rather than a random number: this port has no RNG in it
//!      anywhere, and a session that replays identically is worth more than an
//!      independent coin flip.
//!   2. **A Mario fetches it and carries it to a pylon.** Whether it does is
//!      not decided here: fetching is one of the jobs [`crate::goap`] scores a
//!      Mario's options against, beside fighting, obeying an order and standing
//!      about -- and it is the job that gives way to a fight, because a squad
//!      that turns its back on a slime to pick something up is a squad that
//!      dies holding it. This module owns the balls -- what one is, who is
//!      holding it, what happens when it changes hands -- and that module owns
//!      the choosing.
//!   3. **The pylon ships it back to the stellarator, which keeps it.** Down
//!      the beams, mast to mast, along the hop counts
//!      [`crate::pylon::Network`] already worked out when it decided which
//!      masts had power. That is the whole reason the network stores hops
//!      rather than a flag: the shortest way *to* a machine is the same walk as
//!      the flood *from* one, read backwards. What happens at the far end is
//!      [`crate::stellarator::Store`]: the machine's stock is what its field is
//!      drawn out of, so a full reactor is visibly full.
//!
//! So a network is no longer only a place the player can refuel. It is the road
//! the squad's takings come home on, which is what makes pushing masts outward
//! -- and defending them, now that the crowd knocks them down -- worth doing.
//!
//! **Nothing here holds an entity it did not check.** A Mario can be killed
//! mid-errand and a mast can be knocked over with a ball halfway along it; both
//! are ordinary, and both resolve by the ball being put back on the ground where
//! it was rather than by anything being left dangling.

use std::collections::VecDeque;

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::{level::LevelData, player::FIXED_DT, pylon::Network, squad::Ally};

/// How often a kill leaves something behind, and how often that something is a
/// medkit instead.
///
/// Two neighbouring bands of the one interval [`Drops::maybe`] walks, so a kill
/// leaves nuclonium about one time in twenty, a medkit about one time in
/// twenty-five, and nothing the rest of the time. The red one is the rarer of
/// the two on purpose: it is the one that changes whether a fight is survivable,
/// and something that turns a losing fight around should not be lying about
/// everywhere.
pub const DROP_CHANCE: f32 = 0.05;
pub const MEDKIT_CHANCE: f32 = 0.04;

/// The step of the sequence [`Drops::maybe`] walks, which is the fractional part
/// of the golden ratio.
///
/// The same number [`crate::squad::GOLDEN_ANGLE`] is built out of, doing the
/// same job one dimension down: successive steps never fall near each other, so
/// a sequence of them fills the unit interval about as evenly as anything can.
/// What that buys here is that "five percent" is true over twenty kills and not
/// only over twenty thousand -- a real coin flip clumps, and a player who saw
/// nothing drop from forty enemies would reasonably conclude the feature was
/// broken.
const GOLDEN_STEP: f32 = 0.618_034;

/// How near a ball a Mario has to be to have it, and how near a mast to have
/// delivered it.
///
/// The delivery range is the arrival radius the errand is walked to as well --
/// see [`fetch`] -- so a Mario that has arrived is by construction near enough
/// to hand the ball over, rather than standing a stride short of a mast holding
/// something forever.
pub const PICKUP_RANGE: f32 = 1.3;
pub const DELIVER_RANGE: f32 = 2.6;

/// How near a live mast a ball has to be for the mast to take it up on its own,
/// how near Luna one has to be to tag along behind her, how far her call
/// reaches, how far behind her the train swims and how hard it is pulled.
///
/// [`MAST_REACH`] is wider than [`DELIVER_RANGE`] rather than equal to it,
/// because it answers a different question. That one is how close a Mario has
/// to get to hand something over; this is how far a mast reaches out for
/// something nobody is holding, and it wants to be generous enough that a ball
/// lying near the foot of a mast is visibly taken rather than visibly ignored.
///
/// [`MAGNET_RANGE`] is what makes walking over the takings of a fight collect
/// them. What the whistle is for is everything the walk did not reach -- see
/// [`grab_radius`] for the circle it draws.
pub const MAST_REACH: f32 = 6.0;
pub const MAGNET_RANGE: f32 = 4.5;
const MAGNET_RISE: f32 = 3.0;
const ESCORT_ORBIT: f32 = 1.7;
const ESCORT_PULL: f32 = 3.5;

/// How hard a ball is pulled into the hands of the Mario carrying it, and how
/// hard it sinks back to the ground when it is put down.
///
/// **Nothing made of nuclonium changes place without travelling there.** Being
/// picked up used to be a write: the tick a Mario got within [`PICKUP_RANGE`]
/// the ball was assigned to a point over its head, a metre and a half up and a
/// metre along, in no time at all. The pull is much harder than the train's --
/// this is a thing being grabbed, not a thing tagging along -- but it is the
/// same easing, so what the eye gets is a snatch rather than a substitution.
///
/// The settle is gentler again, and it is a fall rather than a grab. See
/// [`Orb::slack`] for the machinery: the ball keeps its height and the *bob* it
/// is heading for is what moves, so putting one down cannot fight the float.
const CARRY_PULL: f32 = 9.0;
const SETTLE_PULL: f32 = 5.0;

/// The grab circle: how wide it opens, how wide it can be grown to, and how
/// long holding the button takes to grow it.
///
/// **The whistle is a circle rather than a radius around Luna's own feet**, and
/// that is what makes it worth pressing. Walking over a ball already picks it
/// up -- see [`MAGNET_RANGE`] -- so a call that only reached round her would do
/// nothing that walking does not. Aimed, it is laid over the far side of a
/// fight you have just won, or over the bottom of a slope you do not want to
/// climb down.
///
/// The gesture is the squad whistle's exactly: hold to open, hold longer to
/// grow, release to take what is inside. Same aim, same growth, and drawn with
/// [`crate::squad::ring_mesh`] so the two circles a player is asked to read
/// while holding one button are visibly the same kind of mark. It is a little
/// wider than the squad's at both ends, because a ball is a smaller thing to
/// have missed than a Mario is.
///
/// There is no tap/hold split here, unlike the squad's: one mode of the picker
/// is one gesture, so the shortest possible press is simply the smallest
/// possible circle rather than a different command.
const GRAB_MIN_RADIUS: f32 = 3.0;
const GRAB_MAX_RADIUS: f32 = 12.0;
const GRAB_GROW_SECONDS: f32 = 1.0;

/// How long a ball nobody has touched lies about before it goes, and how long
/// it spends leaving.
///
/// Three minutes: long enough that what a fight dropped is still there after
/// the fight and the walk back, short enough that an afternoon of skirmishing
/// across a valley does not leave a thousand of them lit up on the lawn behind
/// it. Five hundred balls is also the point at which the stellarator stops
/// drawing what it is given, so a field that never clears is a field that
/// quietly spends the machine's whole budget on litter.
///
/// **Every kind of touch resets the clock** -- a Mario claiming one, Luna's
/// magnet, the whistle, a mast reaching for it -- so what expires is only ever
/// a ball nobody in the game has shown the slightest interest in. See
/// [`linger`].
///
/// It shrinks rather than blinking out, and it shrinks rather than fading
/// because of how these are drawn: there is one material per [`Kind`], shared
/// by every ball in the level, so a ball cannot fade on its own without a
/// material of its own. Scale is per-entity and free -- the glow is a child and
/// comes with it, and a trail's width is read off the same transform -- so the
/// whole thing dwindles together.
pub const IDLE_LIFE: f32 = 180.0;
const IDLE_FADE: f32 = 2.0;

/// How far above and below itself a Mario can reach for one.
///
/// Up is a little over a Mario's own height, so something floating at head
/// height is had and something on the parapet above is not. Down is nearly as
/// far, so a ball that came to rest on a slope just below the spot the walk
/// stopped at is still picked up.
const REACH_UP: f32 = crate::player::PLAYER_HEIGHT + 0.6;
const REACH_DOWN: f32 = 1.6;

/// How big a ball is, how far off the ground it floats, and how high above a
/// Mario's head it rides while being carried.
const BALL_RADIUS: f32 = 0.28;
const BALL_LIFT: f32 = 0.80;
pub const CARRY_HEIGHT: f32 = 1.9;

/// How much wider than the ball its glow reaches, how far that swells as it
/// breathes, and how many texels across the picture of it is.
///
/// **The glow is a textured quad, and it has to be, because nothing else in this
/// renderer can draw one.** An `emissive` on a `StandardMaterial` reaches the
/// screen two ways in Bevy and this port has neither: the lit path folds it into
/// the shading, which an `unlit` material skips outright -- `pbr.wgsl` writes
/// `out.color = base_color` and never calls `apply_pbr_lighting` -- and a value
/// over 1.0 only *reads* as light because a bloom pass smears it, which wants an
/// HDR camera this game does not have. So the first version of these balls was a
/// flat green circle wearing an `emissive` field no shader ever looked at.
///
/// The second was a translucent sphere around the core, and that was worse in an
/// instructive way: a sphere at a constant alpha has a hard silhouette, so what
/// it drew was a bigger flat circle behind a smaller one. **A glow is a falloff.**
/// It has to be a picture with alpha going to nothing at its edge, on a quad
/// turned to face the camera -- which is what [`crate::sky`] draws the sun with,
/// out of the same [`crate::sky::texture`] helper.
const HALO_SCALE: f32 = 3.2;

/// How far the glow reaches from the middle of a ball drawn at full size.
///
/// Public because whatever a unit of nuclonium is put *inside* has to know how
/// much room it takes: [`crate::stellarator`] fits five hundred of them into a
/// tube a fraction of a metre across, and the thing that has to stay inside the
/// plasma is the glow rather than the little sphere at the middle of it.
pub const GLOW_RADIUS: f32 = BALL_RADIUS * HALO_SCALE;
const HALO_SWELL: f32 = 0.14;

/// How far in front of its ball a glow is drawn, as a share of its own radius.
///
/// What it is for is at [`shimmer`]: a card aimed at the camera stands *through*
/// the ground its ball is floating over, and the straight line where the two
/// surfaces cross is the hard bite out of the bottom of every glow in the game.
/// Drawn a little nearer than it hangs, the card clears the ground and the glow
/// fades into it instead.
///
/// **It has a ceiling, and the ceiling is the ball.** The one thing the player
/// must keep seeing is the white-hot core, and what hides the brightest part of
/// the card -- the middle, where the falloff is opaque -- is the little sphere
/// sitting in front of it. That sphere reaches `1.0 / HALO_SCALE` of the glow's
/// radius, so a float past that puts the card in front of the ball and turns a
/// white ball with a green glow into a flat green disc.
///
/// Measured, at a playing camera: a third of a radius leaves the core as a
/// pinhole, a quarter leaves a distinct bright ring where the card overtakes
/// the sphere's shoulder, and a fifth is a white ball with a faint rim of glow
/// around it -- and the clip line is gone at all three. So the number is set by
/// the smallest float that clears the ground rather than by the largest one the
/// ball can hide, which is the opposite of how it started.
const HALO_FLOAT: f32 = 0.2;
const HALO_TEXELS: u32 = 64;

/// How sharply the glow falls away from the middle.
///
/// Above one, so it is bright in the centre and thin at the rim rather than a
/// linear ramp, which reads as a disc with soft edges instead of as light. Not
/// far above one, though: the middle of the falloff is hidden behind the core
/// that sits in front of it, so all the player ever sees is the outer half, and
/// a steep curve spends that half on nothing.
const HALO_FALLOFF: f32 = 1.5;

/// How fast and how far a loose ball bobs.
///
/// Much further than it was, and slower, because a bob is now the only thing a
/// ball lying on the ground *does* -- and a trail is however much ground was
/// covered, so a ball that barely moved drew nothing worth looking at. Nearly a
/// metre peak to peak at a little over half a hertz is a lazy float with a
/// visible wake, which is what a lump of the stuff the whole economy runs on
/// ought to look like sitting in a field. It rides higher than it did as well,
/// so the bottom of the bob clears the grass rather than sinking into it.
///
/// Presentation only, and off wall-clock time rather than the fixed step: it is
/// the one thing in this module that is allowed to stop when the console is
/// open, because a ball that is one centimetre lower than it would have been is
/// not a fact the simulation depends on.
///
/// There is no spin here, and there was: a sphere in one flat colour looks
/// exactly the same at every rotation, so turning it was work nobody could see.
/// Worse, it was work in the way -- the glow is a child quad aimed at the
/// camera, and a parent that rotates is a rotation the child has to undo.
const BOB_HZ: f32 = 0.55;
const BOB_RISE: f32 = 0.40;

/// How fast a delivered ball flies home along the beams, in metres a second.
///
/// Slower than [`crate::pylon`]'s supply packet, which is light. This is a
/// physical thing being moved, and being able to watch it cross the valley is
/// most of what makes a long network feel like it is doing something.
pub const SHIP_SPEED: f32 = 18.0;

/// How long a piece of trail hangs in the air, how far apart the path behind a
/// ball is marked, how wide the ribbon of it is beside the glow, how far a
/// thing has to move in one frame for that to be a teleport rather than
/// travel, and the most marks one trail keeps.
///
/// **A trail is the path the thing actually took.** What this replaced was the
/// glow itself, leaned over and stretched along the way it was going, which is
/// two multiplies and no extra entities and is wrong in three ways a player can
/// see. It is straight, so it cannot follow anything turning -- and everything
/// that wears one here turns, most of all the motes going round inside a
/// stellarator. Its length came out of a formula on the speed rather than out
/// of ground actually covered, so it was the same length whatever the ball had
/// been doing. And it flickered, which was the tell that the whole approach was
/// wrong: the stretched card is a *child* of the ball, so the offset it was
/// pushed backwards by last frame was part of the position it measured its own
/// speed from this frame. A quantity that feeds back into its own input
/// oscillates, and it did.
///
/// So a trail is now a short history of where the thing has been, in world
/// space, taken from the **ball** rather than from the glow hanging on it --
/// which is what makes the measurement honest, because a ball's transform is
/// written by the game and the glow's is written by the thing measuring it.
///
/// **Nothing here sets how long a trail is.** Its length is however much ground
/// was covered in the last [`TRAIL_LIFE`] seconds: twice the speed is twice the
/// trail, with no cap and no rate in it. Something standing still lays no new
/// marks and the ones behind it expire where they lie, so its trail shortens
/// from the tail and goes out -- which is what hanging in the air and fading
/// looks like, rather than a permanent smear on a stationary ball.
///
/// [`TRAIL_STEP`] is measured against the glow's own width rather than in
/// metres, for the reason every other number in this module is: a mote inside a
/// machine is a fiftieth of a ball, and one spacing in metres would give it
/// either a single mark or none.
///
/// It is small, and that is deliberate. Its whole job is to tell moving from
/// *not* moving; how finely a path is remembered is [`MARK_INTERVAL`]'s job,
/// and the cost is bounded by [`TRAIL_MARKS`] whatever this is set to. Set
/// coarse it silently becomes the second thing as well, and then anything
/// slower than about a metre a second lays marks further apart in time than
/// they survive -- so a ball floating gently in a field drew one lonely mark at
/// a time, blinking, instead of a trail.
const TRAIL_LIFE: f32 = 0.45;
const TRAIL_STEP: f32 = 0.06;
const TRAIL_WIDTH: f32 = 0.62;
const TRAIL_TAPER: f32 = 0.22;
const TRAIL_JUMP: f32 = 8.0;
const TRAIL_MARKS: usize = 20;

/// How far past the ball the head of its trail closes over, as a multiple of
/// the ribbon's own half width, and in how many rungs.
///
/// **A ribbon that stops at the ball stops at a wall.** The strip is as wide
/// there as it ever gets and at full brightness, and the last thing drawn is a
/// straight rung square across the direction of travel -- so what meets the eye
/// is a bright bar with a corner at each end, sitting across a round glowing
/// ball. That edge is the abrupt transition at the front of a trail, and it is
/// abrupt because it is a genuine discontinuity: full width one side of the
/// line, nothing at all the other.
///
/// So the ribbon carries on past the ball and closes. The rungs ahead of it
/// narrow on a circular profile -- `sqrt(1 - t^2)`, the outline of a sphere seen
/// side-on -- which makes the end of the strip a dome the width of the trail
/// rather than a cut edge, and puts the ball inside its own wake instead of at
/// the end of it. Nothing about the fade changes: the dome is the newest part
/// of the trail and is drawn at the head's own brightness, so what the player
/// sees is the glow wrapping round the front of the ball and streaming off the
/// back of it.
///
/// It is not enough on its own, though, and the second half is
/// [`VEIL_REACH`]: a dome at full brightness is still a hard-edged object
/// crossing a soft one.
///
/// Only when it is moving. A ball that has stopped has no direction to close
/// over, and the fallback would be a bright dome pointing wherever the last
/// segment happened to point -- the same class of artefact [`Ribbon::weave`]
/// drops zero-length segments to avoid.
///
/// Three rungs, because the profile is a quarter circle and three chords draw
/// one to well inside a pixel at the size a ball is seen at. The last of them
/// has no width at all, which is the point of the dome closing rather than
/// merely tapering.
const NOSE_REACH: f32 = 1.0;
const NOSE_RUNGS: usize = 3;

/// How far behind the ball the trail comes out from under its glow, as a
/// multiple of the glow's own radius, and how much of it shows inside that.
///
/// **The other half of the abrupt transition, and the half that actually shows
/// in a photograph.** A glow is a soft disc a couple of ball-widths across with
/// its alpha falling to nothing at the rim. A ribbon drawn at full brightness
/// through the middle of one is not part of it -- it is a second, harder object
/// lying across it, and every edge it has is visible against the falloff it is
/// drawn over. What that looks like on a slowly bobbing ball is a bright wedge
/// with a straight side sticking out of one shoulder of the glow, which is
/// exactly the "visible transition" this is against.
///
/// So the trail comes *out* of the glow rather than starting on top of it. Its
/// brightness is veiled to a fraction near the ball and lifts to full about a
/// glow-radius behind, which is where the glow itself has faded to nothing --
/// so the eye is handed one continuous falloff instead of two overlapping ones
/// with a boundary between them. The wake is unchanged everywhere it is the
/// only thing there.
///
/// The floor is not zero, and that matters for the slow case. A ball floating
/// in a field bobs about two thirds of a metre a second, so its whole trail is
/// shorter than the veil -- and at a floor of zero, the wake `next.md` asked to
/// be able to see on a hovering ball would be veiled out of existence.
const VEIL_REACH: f32 = 1.6;
const VEIL_FLOOR: f32 = 0.25;

/// The shortest time between two marks.
///
/// A ceiling on how *finely* a path is remembered, and it exists so that
/// [`TRAIL_MARKS`] can never be the thing that decides how long a trail is.
/// Without it the two gates fight: a shipment crossing the valley at eighteen
/// metres a second lays a mark every frame, runs out of room after a third of a
/// second, and ends up with a trail shorter than a Mario's -- which is precisely
/// the "length decided by something other than the travelling" this whole
/// rewrite is against.
///
/// Set so that a history laid at exactly this rate is exactly full when its
/// oldest mark expires. Marks are never closer together in time than this
/// however fast the frames come, so the buffer covers [`TRAIL_LIFE`] at any
/// frame rate, and the cap is a bound on memory rather than on anything the
/// player can see.
///
/// It is also what keeps five hundred motes affordable. Twenty rungs is more
/// than enough to draw a smooth curve at the size these are seen at, and the
/// mesh is rebuilt every frame -- so this is the difference between twenty
/// thousand vertices a frame and sixty thousand, for a picture nobody could
/// tell apart.
const MARK_INTERVAL: f32 = TRAIL_LIFE / TRAIL_MARKS as f32;

/// Whether a ball is lying about, on its way to somebody, or in somebody's
/// hands.
///
/// One component with a state rather than two components swapped between,
/// because the swap would be a `Commands` write and everything here runs inside
/// one fixed tick: a ball that became carried at the top of the tick has to be
/// carried by the bottom of it, not at the next sync point.
#[derive(Debug, PartialEq, Eq)]
pub enum Held {
    /// On the ground. `claimed` is the Mario walking to it, if any -- kept so
    /// that eight Marios do not all converge on one ball while the other seven
    /// lie untouched.
    Loose { claimed: Option<Entity> },
    /// Being carried by a Mario, and drawn over its head.
    Carried(Entity),
    /// Tagging along behind whoever picked it up -- Luna, in practice. Not the
    /// same as being carried: a train of these swims after its leader on a
    /// spring rather than being pinned to a point, which is what makes them
    /// look like a shoal and what gives their trails something to draw.
    ///
    /// See [`swim`] for the motion, [`MAGNET_RANGE`] for how one joins by
    /// being walked past, and [`call`] for how a circleful joins at once.
    Following(Entity),
}

/// One resource ball.
#[derive(Component)]
pub struct Nuclonium {
    pub held: Held,
}

/// Which of the two things a kill can leave behind this is.
///
/// **One enum rather than two kinds of ball**, because a ball is a *look* -- a
/// small bright sphere inside a glow, floating, with a wake behind it -- and
/// the look is shared exactly. What differs is what it means: green is
/// [`Nuclonium`], the substance the whole economy runs on, hauled home down the
/// beams; red is a [`crate::health::Medkit`], which nobody hauls anywhere
/// because whoever touches it is who it was for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    Nuclonium,
    Medkit,
}

impl Kind {
    /// Every kind there is, in the order [`Art`] holds their materials.
    pub const ALL: [Kind; 2] = [Kind::Nuclonium, Kind::Medkit];

    /// Which slot in [`Art`] this one's materials are in.
    fn slot(self) -> usize {
        match self {
            Kind::Nuclonium => 0,
            Kind::Medkit => 1,
        }
    }

    /// The hot middle, and the colour it bleeds outward.
    ///
    /// Both cores are nearly white with their own colour in them, because a
    /// small bright thing is white at the centre whatever colour it is -- the
    /// hue lives in the glow around it, which is the part that reads at
    /// distance. Green and red are as far apart as this palette gets, which is
    /// the point: a player has to be able to tell "worth banking" from "worth
    /// running over to" at a glance across a field.
    fn colours(self) -> (Color, Color) {
        match self {
            Kind::Nuclonium => (Color::srgb(0.72, 1.00, 0.78), Color::srgb(0.24, 1.00, 0.40)),
            Kind::Medkit => (Color::srgb(1.00, 0.78, 0.76), Color::srgb(1.00, 0.24, 0.28)),
        }
    }

    /// What a trail of this is painted, as a vertex colour.
    ///
    /// Every trail in the level shares one mesh and therefore one material, so
    /// the colour cannot live on the material the way the glow's does: it rides
    /// on the vertices instead, and the material is left white. That is also
    /// what makes a third kind of ball free -- a colour and nothing else.
    pub fn tint(self) -> [f32; 3] {
        let glow = self.colours().1.to_linear();
        [glow.red, glow.green, glow.blue]
    }
}

/// The float and the look every ball wears, whatever it turns out to mean.
///
/// Separate from [`Nuclonium`] because a medkit is not nuclonium and does not
/// want its `held` -- but it is exactly the same object to draw. Composition
/// rather than a second copy of the bobbing.
#[derive(Component)]
pub struct Orb {
    pub kind: Kind,
    /// Its own clock, so a scattering of them does not bob as one body. The
    /// golden angle again, exactly as [`crate::pylon::Emitter`] seeds its
    /// shimmer and for the same reason: a field that never repeats, out of a
    /// game with no random number generator in it.
    phase: f32,
    /// The height it hovers about while it is lying on the ground.
    ///
    /// Kept rather than read back off the transform, because [`shimmer`] writes
    /// that transform every frame -- a bob taken from where the thing already is
    /// is a bob that walks itself into the floor over a minute.
    rest: f32,
    /// How long nobody has taken any interest in it. See [`IDLE_LIFE`].
    idle: f32,
    /// How far above the height it should be bobbing at it currently is, and
    /// what is left of a fall it has not finished.
    ///
    /// **What makes putting a ball down a fall rather than a cut.** A ball let
    /// go of is a metre and a half up over a Mario's head, and its resting
    /// height is on the grass; writing the second one is the ball being in two
    /// places on two consecutive frames with nothing in between. So the drop is
    /// kept here instead: [`drop_to`] records the gap, [`shimmer`] adds it to
    /// the bob and eases it away, and the ball sinks to the ground it was going
    /// to end up on anyway.
    ///
    /// An offset rather than easing the transform straight towards the bob,
    /// because the bob is a *moving* target -- a first-order filter chasing a
    /// sine comes out shallower and late, so easing the position would quietly
    /// change how every ball in the level floats in order to fix how one of
    /// them is put down. This adds nothing to a ball that has been lying there
    /// for a second: `slack` is zero, and zero plus the bob is the bob.
    slack: f32,
}

impl Orb {
    /// Puts it down at `y`: the height it bobs about from here on.
    ///
    /// A method rather than a public field because the invariant is the whole
    /// point of the field. `rest` is the height the bob is measured *from*, so
    /// whoever puts a ball down owns it, and anything that wrote the ball's
    /// current height in mid-bob would leave it resting wherever in the sine it
    /// happened to be. Public because [`crate::health::mend`] puts one down
    /// too, when the body it was drifting towards is no longer there.
    pub fn settle(&mut self, y: f32) {
        self.rest = y;
    }

    /// Lets it go from `from` and gives it `ground` to sink to.
    ///
    /// The one way a ball is put down from a height. Returns nothing and moves
    /// nothing: the caller's transform is left exactly where it was, which is
    /// the point -- what changes is where the ball is *heading*. See
    /// [`Orb::slack`].
    pub fn drop_to(&mut self, from: f32, ground: f32) {
        self.rest = ground;
        self.slack = from - ground;
    }
}

/// How big a ball nobody has touched for `idle` seconds is drawn.
///
/// Full size for almost all of its life and then a quick shrink to nothing,
/// rather than a slow deflation over three minutes. The size of a ball is
/// information -- it is most of how far away the player reads one as being --
/// so something that spent three minutes getting smaller would be lying about
/// that the whole time, and only the last couple of seconds would be honest.
///
/// Its own function, and pure, because what wants asserting is the shape:
/// nothing visible happens until the end, and the end reaches exactly zero.
pub fn dwindle(idle: f32) -> f32 {
    ((IDLE_LIFE - idle) / IDLE_FADE).clamp(0.0, 1.0)
}

impl Nuclonium {
    /// Whether this one is free for the taking by `mario`.
    ///
    /// Three refusals and two exceptions. Something being carried is nobody
    /// else's; something claimed by another Mario is that Mario's; but your own
    /// claim is not a refusal, which is what lets [`fetch`] tear every claim up
    /// each tick and have a Mario simply take its own ball back. And `alive` is
    /// the backstop: a claim by somebody who is no longer in the field is not a
    /// claim, so a ball can never end up reserved for a ghost.
    pub fn available(&self, mario: Entity, alive: impl Fn(Entity) -> bool) -> bool {
        match self.held {
            Held::Loose { claimed: None } => true,
            Held::Loose {
                claimed: Some(other),
            } => other == mario || !alive(other),
            Held::Carried(_) | Held::Following(_) => false,
        }
    }
}

/// A delivered ball on its way back to a machine.
///
/// The route is a list of points worked out once, at the moment of delivery,
/// rather than a node index re-read every tick. A network that changes under a
/// shipment -- a mast knocked over behind it -- leaves it flying the line it set
/// out on, which is a ball taking the scenic route rather than a ball
/// teleporting or a lookup that has to handle a hole.
#[derive(Component)]
pub struct Shipment {
    legs: Vec<Vec3>,
    /// How far along the whole polyline it is, in metres.
    along: f32,
}

/// The drop roll, and the balls it owes the world.
///
/// A resource rather than a call that spawns, because the four places a kill is
/// resolved are four different systems with four different sets of borrows, and
/// none of them wants a mesh handle. They queue a position; [`shed`] spawns.
#[derive(Resource, Default)]
pub struct Drops {
    /// Where in the sequence the next roll is. See [`GOLDEN_STEP`].
    phase: f32,
    /// Places something died and left something behind, this tick, and which
    /// of the two things it left.
    queue: Vec<(Vec3, Kind)>,
    /// How many have ever dropped, which is what each ball's own clock comes
    /// off. A counter rather than the entity index, so two runs of the same
    /// session bob identically whatever else has been spawned in between.
    shed: u32,
}

impl Drops {
    /// Rolls for one kill at `at`, and reports what it left behind.
    ///
    /// **This is the only die in the game.** The step is irrational, so the
    /// sequence never repeats and never lands twice in the same place; taking
    /// the fractional part of a running sum and asking where it fell is a rate
    /// that is also *evenly spread* -- about one ball every twenty kills rather
    /// than three at once and then none for eighty.
    ///
    /// One roll settles both kinds, off the same walk, by giving each its own
    /// band of the interval. Two dice would be two Weyl sequences advancing in
    /// lockstep, which is the one way to make this trick fail: the same step
    /// from two offsets is the same sequence, so a kill that dropped nuclonium
    /// would be a kill that always dropped a medkit as well.
    pub fn maybe(&mut self, at: Vec3) -> Option<Kind> {
        self.phase = (self.phase + GOLDEN_STEP).fract();
        let dropped = match self.phase {
            phase if phase < DROP_CHANCE => Some(Kind::Nuclonium),
            phase if phase < DROP_CHANCE + MEDKIT_CHANCE => Some(Kind::Medkit),
            _ => None,
        };
        if let Some(kind) = dropped {
            self.queue.push((at, kind));
        }
        dropped
    }
}

/// How much has reached a machine.
///
/// A count rather than a currency: nothing spends it yet. What it is for today
/// is telling the player that the chain works -- see the debug HUD -- and being
/// the one place a cost would be taken from when there is something to buy.
#[derive(Resource, Default)]
pub struct Bank {
    pub stored: u32,
}

/// The quad that makes a ball look lit.
///
/// A child of the ball rather than a second top-level entity, so it is carried,
/// bobbed and despawned by whatever happens to the ball with nothing here to
/// keep in step. Its phase is copied from its ball's at spawn, so the core and
/// the glow breathe together.
///
/// It relies on its parent never being rotated -- see [`BOB_HZ`] -- which is
/// what lets [`shimmer`] write a world-space heading straight into its local
/// rotation instead of cancelling the parent's first, the way
/// [`crate::billboard::aim`] has to for a quad inside a skeleton.
#[derive(Component)]
pub struct Halo {
    phase: f32,
}

/// Somewhere a unit of nuclonium has been, and how long ago it was there.
///
/// Age rather than a timestamp, so the history means the same thing in a test
/// with no clock in it as it does in a running game -- and so a trail behaves
/// the same however long the session has been up, which a subtraction of two
/// large `f32` seconds stops doing after a few hours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub at: Vec3,
    pub age: f32,
}

/// Where one unit of nuclonium has recently been.
///
/// Sits on the **ball**, not on the glow hanging off it -- see [`TRAIL_LIFE`]
/// for why that distinction is the whole difference between a trail and a
/// flicker. Part of [`core`], so all three things made of nuclonium get one
/// without any of the three asking: a ball on the lawn, a shipment flying down
/// the beams, and a mote turning inside a machine.
#[derive(Component)]
pub struct Trail {
    /// Oldest first, so the end that expires is the end that is popped.
    path: VecDeque<Mark>,
    /// Where the last mark was laid, kept after that mark has expired.
    ///
    /// The spacing gate has to be measured against this rather than against the
    /// newest surviving mark, or a thing standing perfectly still lays a fresh
    /// mark the instant its trail runs out -- there is nothing left to be too
    /// near to -- and then does it again for ever, one mark at a time.
    last: Option<Vec3>,
    /// How long since the last mark was laid. See [`MARK_INTERVAL`].
    since: f32,
    /// What this one is painted. Every trail in the level shares one mesh and
    /// therefore one material, so the colour rides on the vertices -- see
    /// [`Kind::tint`]. White by default, which is what an unpainted trail
    /// wearing a white material comes out as.
    tint: [f32; 3],
}

impl Default for Trail {
    fn default() -> Self {
        Trail {
            path: VecDeque::new(),
            last: None,
            since: 0.0,
            tint: [1.0; 3],
        }
    }
}

impl Trail {
    /// An empty history, painted the colour of one kind of ball.
    pub fn of(kind: Kind) -> Self {
        Trail {
            tint: kind.tint(),
            ..Trail::default()
        }
    }

    /// Ages every mark by `dt` and forgets the ones that have gone out.
    ///
    /// This is the only thing that shortens a trail, and it runs whether the
    /// ball moved or not -- which is exactly why a ball that stops has its
    /// trail eaten from the tail until there is none of it left.
    ///
    /// **One expired mark is kept, and that is what stops the tail jumping.**
    /// Dropping a mark the instant it turns [`TRAIL_LIFE`] old ends the ribbon
    /// at the youngest surviving one, which is somewhere between nothing and a
    /// whole mark's worth of ground short of where the trail actually ends. So
    /// the tail did not recede, it *hopped*: it sat still while the marks aged
    /// and then snapped back a step, twenty times a trail, which is the abrupt
    /// transition at the far end of one. Keeping the expired mark leaves
    /// [`Ribbon::weave`] something to interpolate towards, and the drawn end
    /// then slides along the path at whatever speed the ball laid it down --
    /// which is the same speed the head is moving away at, so the trail is the
    /// same length from both ends.
    ///
    /// The last mark of all is dropped when it expires: with nothing left in
    /// front of it there is nothing to interpolate towards, and a lone expired
    /// mark is a trail that has gone out.
    pub fn fade(&mut self, dt: f32) {
        self.since += dt;
        for mark in &mut self.path {
            mark.age += dt;
        }
        while self.path.len() > 1 && self.path[1].age >= TRAIL_LIFE {
            self.path.pop_front();
        }
        if self.path.front().is_some_and(|mark| mark.age >= TRAIL_LIFE) && self.path.len() == 1 {
            self.path.pop_front();
        }
    }

    /// Notes where the thing is now, if it has gone `spacing` since the last
    /// mark.
    ///
    /// A distance gate rather than a mark every frame, for two reasons that
    /// happen to want the same thing: a ball bobbing on the spot is not
    /// travelling and should lay nothing, and five hundred motes laying sixty
    /// marks a second each is a mesh rebuilt out of thirty thousand points.
    ///
    /// `jump` is the teleport guard. Nuclonium is moved outright in several
    /// places -- a ball picked up snaps to the top of a Mario's head, a mote is
    /// placed the frame after it is spawned -- and a straight line drawn across
    /// that gap is a green streak across the level that then hangs there for
    /// [`TRAIL_LIFE`]. A move too big to have been travel throws the history
    /// away instead.
    pub fn record(&mut self, here: Vec3, spacing: f32, jump: f32) {
        let Some(last) = self.last else {
            // **The first sight of a thing is a datum, not a mark.** A trail
            // laid from the frame an entity appeared is a trail from wherever
            // it was *constructed*, and for a mote that is the middle of the
            // machine, on the lawn: every arrival would draw a green streak out
            // of the reactor's axis and leave it hanging for [`TRAIL_LIFE`].
            // The teleport guard below does not reliably catch that -- whether
            // it does depends on how big the machine happens to be -- and a
            // rule that holds for one model size is not a rule.
            self.last = Some(here);
            self.since = 0.0;
            return;
        };
        let moved = last.distance(here);
        if moved > jump {
            // A teleport, and it is thrown away at once rather than at the
            // next mark: a stale history is drawn every frame it survives.
            self.path.clear();
        } else if moved < spacing || self.since < MARK_INTERVAL {
            return;
        }
        self.last = Some(here);
        self.since = 0.0;
        self.path.push_back(Mark { at: here, age: 0.0 });
        while self.path.len() > TRAIL_MARKS + 1 {
            self.path.pop_front();
        }
    }

    /// How many marks are behind it. For the tests, and for nothing else.
    pub fn len(&self) -> usize {
        self.path.len()
    }

    /// Whether it has laid anything down yet.
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// How long the drawn trail is, in metres: the ground the marks cover.
    #[cfg(test)]
    ///
    /// Not stored anywhere and not decided anywhere -- it is a consequence of
    /// how fast the thing was going, which is the property [`TRAIL_LIFE`] is
    /// about. Read by the tests to say so.
    pub fn span(&self) -> f32 {
        self.path
            .iter()
            .zip(self.path.iter().skip(1))
            .map(|(from, to)| from.at.distance(to.at))
            .sum()
    }
}

/// The one mesh every trail in the world is drawn out of.
///
/// A marker on the single entity that wears it. **One mesh rather than one per
/// ball**, because the alternative at five hundred motes is five hundred
/// dynamic meshes uploaded every frame to draw a few thousand triangles. The
/// vertices are in world space and the entity never moves, which is what lets
/// unrelated things share it.
#[derive(Component)]
pub struct Ribbons;

/// One rung of a woven ribbon: a point on the path, and how wide the strip is
/// there as a share of what its age alone would give it.
///
/// The width of a rung is two separate questions and this is the second of
/// them. How faded it is comes from how old it is, which is the trail going out
/// behind the ball; how *round* it is comes from where it sits in the dome over
/// the front of the ball, which has nothing to do with time. Keeping them apart
/// is what lets the nose be drawn out of the same loop as the tail rather than
/// out of a second one. See [`NOSE_REACH`].
struct Rung {
    mark: Mark,
    girth: f32,
}

/// One frame's worth of trails, being built.
///
/// Its own type, filled by a pure method, so what a trail actually looks like
/// can be asserted without a camera, a mesh or a frame -- see the tests.
#[derive(Default)]
pub struct Ribbon {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colours: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Ribbon {
    /// Adds one thing's trail: a strip of quads running from where it is now
    /// back along everywhere it has been.
    ///
    /// The head is the ball itself at no age at all, rather than the newest
    /// mark, so the bright end of the trail is joined to the thing that is
    /// making it however long ago the last mark was laid.
    ///
    /// Each rung is turned to face `eye`: across the ribbon is perpendicular to
    /// both the direction of travel and the direction of the camera, which is
    /// what makes a flat strip read as a tube of light from wherever it is
    /// looked at -- the same trick as the glow's card, done per rung so that it
    /// survives the path bending.
    pub fn weave(&mut self, head: Vec3, trail: &Trail, eye: Vec3, width: f32) {
        // Newest first, so `age` runs from nothing at the ball to
        // [`TRAIL_LIFE`] at the tail and the fade below is just that age.
        if trail.is_empty() {
            return;
        }
        // **Points on top of each other are dropped rather than drawn.** A
        // rung is turned by the direction from it to the next point along, so
        // two coincident points give it no direction at all -- and whatever
        // gets substituted then is a full-width bar of glow lying across the
        // ball, pointing wherever the fallback pointed. That is the smear above
        // and below a ball that has stopped. There is no orientation that is
        // right for a segment with no length; the only correct thing to do with
        // one is not draw it.
        //
        // The head is the point that goes, when one has to. A ball sitting on
        // its own newest mark is a ball that has stopped, and what should be
        // left behind is its old path fading where it lies -- not a bright
        // full-alpha blob pinned to it until the marks run out.
        let near = (width * 0.02).max(1e-4);
        let mut spine: Vec<Mark> = Vec::with_capacity(trail.len() + 1);
        for mark in trail.path.iter().rev().copied() {
            if spine
                .last()
                .is_none_or(|last: &Mark| last.at.distance(mark.at) > near)
            {
                spine.push(mark);
            }
        }
        // The ball itself goes in front of its newest mark -- unless it is
        // standing on it, which is what a ball that has stopped is doing.
        let moving = spine
            .first()
            .is_some_and(|first| first.at.distance(head) > near);
        if moving {
            spine.insert(0, Mark { at: head, age: 0.0 });
        }
        if spine.len() < 2 {
            return;
        }
        // **The far end is cut where the trail actually ends rather than at the
        // last mark before it.** [`Trail::fade`] keeps one mark past its life
        // precisely so there is something here to cut against: the tail is slid
        // up the last segment to the point that is exactly [`TRAIL_LIFE`] old,
        // which moves smoothly as the marks age instead of jumping a whole
        // mark's worth of ground each time one is dropped. It also lands the
        // end vertex on a fade of exactly zero, so a trail always ends in
        // nothing rather than in whatever alpha the oldest surviving mark
        // happened to have.
        let last = spine.len() - 1;
        if spine[last].age > TRAIL_LIFE {
            let (tail, ahead) = (spine[last], spine[last - 1]);
            let span = tail.age - ahead.age;
            let over = match span > 1e-5 {
                true => ((tail.age - TRAIL_LIFE) / span).clamp(0.0, 1.0),
                false => 0.0,
            };
            spine[last] = Mark {
                at: tail.at.lerp(ahead.at, over),
                age: TRAIL_LIFE,
            };
        }
        // The dome over the front of the ball, laid down before the spine so
        // the strip runs nose first. See [`NOSE_REACH`]: it is what turns the
        // cut edge at the head into the trail closing over the thing making it.
        let mut rungs: Vec<Rung> = Vec::with_capacity(spine.len() + NOSE_RUNGS);
        let forward = match moving {
            true => (spine[0].at - spine[1].at).try_normalize(),
            false => None,
        };
        if let Some(forward) = forward {
            // Never longer than half the wake behind it. A dome is the trail
            // closing over the ball, so a ball with almost no trail gets almost
            // no dome -- otherwise the one thing a hovering ball draws is a
            // bright wedge sticking out of the front of its glow, which is the
            // artefact this whole section is about rather than a smaller
            // version of the fix.
            let wake: f32 = spine
                .windows(2)
                .map(|pair| pair[0].at.distance(pair[1].at))
                .sum();
            let reach = (width * NOSE_REACH).min(wake * 0.5);
            for step in (1..=NOSE_RUNGS).rev() {
                let along = step as f32 / NOSE_RUNGS as f32;
                rungs.push(Rung {
                    mark: Mark {
                        at: head + forward * (reach * along),
                        age: 0.0,
                    },
                    // The outline of a sphere: full width at the ball, nothing
                    // at the tip.
                    girth: (1.0 - along * along).max(0.0).sqrt(),
                });
            }
        }
        rungs.extend(spine.iter().map(|mark| Rung {
            mark: *mark,
            girth: 1.0,
        }));
        let base = self.positions.len() as u32;
        for (step, rung) in rungs.iter().enumerate() {
            let mark = &rung.mark;
            // Along the ribbon: towards the next mark down the spine, except at
            // the far end, which takes the one before it so the last rung is
            // square to the trail rather than to nothing.
            let along = match step + 1 < rungs.len() {
                true => rungs[step + 1].mark.at - mark.at,
                false => mark.at - rungs[step - 1].mark.at,
            };
            let toward = eye - mark.at;
            let across = along
                .cross(toward)
                .try_normalize()
                // Coming straight at the camera. The rung is edge-on whatever
                // is picked, so this only has to be perpendicular to the way
                // the thing is going -- which the spine guarantees is a real
                // direction, because zero-length segments were dropped above.
                .unwrap_or_else(|| along.any_orthonormal_vector());
            let fade = (1.0 - mark.age / TRAIL_LIFE).clamp(0.0, 1.0);
            // How far out from under the ball's own glow this rung is. Measured
            // straight from the ball rather than along the path, because what
            // it stands for is the falloff of a round picture centred there.
            // See [`VEIL_REACH`].
            let out = (mark.at.distance(head) / (width / TRAIL_WIDTH * VEIL_REACH)).clamp(0.0, 1.0);
            let veil = VEIL_FLOOR + (1.0 - VEIL_FLOOR) * out * out * (3.0 - 2.0 * out);
            // Narrower as it goes out as well as fainter, so it tapers to a
            // point instead of ending in a squared-off bar -- but never to
            // nothing, which is a trail that looks pinched in the middle.
            let half = width * (TRAIL_TAPER + (1.0 - TRAIL_TAPER) * fade) * rung.girth;
            for side in [-1.0_f32, 1.0] {
                let at = mark.at + across * (half * side);
                self.positions.push([at.x, at.y, at.z]);
                // Straight down the middle of the glow's own picture: `u` is
                // pinned at the centre and `v` crosses it, so the ribbon's
                // edges melt exactly the way a ball's rim does, out of the one
                // texture. See [`HALO_SCALE`].
                self.uvs.push([0.5, side * 0.5 + 0.5]);
                // Squared, so most of the fade happens late and a trail reads
                // as a bright head with a thin tail rather than as a bar of
                // even grey. The colour is the ball's own -- see [`Kind::tint`]
                // for why it is here and not on the material.
                let [red, green, blue] = trail.tint;
                self.colours.push([red, green, blue, fade * fade * veil]);
            }
        }
        for step in 0..rungs.len() as u32 - 1 {
            let corner = base + step * 2;
            self.indices.extend_from_slice(&[
                corner,
                corner + 1,
                corner + 2,
                corner + 2,
                corner + 1,
                corner + 3,
            ]);
        }
    }

    /// The middle of every rung, in the order they were laid: the path the
    /// ribbon was actually drawn down, as opposed to the history it was drawn
    /// from. For the tests, which is the only place the difference between
    /// those two can be checked.
    #[cfg(test)]
    pub fn spine(&self) -> Vec<Vec3> {
        self.positions
            .chunks_exact(2)
            .map(|pair| (Vec3::from(pair[0]) + Vec3::from(pair[1])) * 0.5)
            .collect()
    }

    /// How long the drawn ribbon is *behind* the ball, in metres: the wake,
    /// without the dome the head closes over the ball with.
    ///
    /// What "the length of a trail" means everywhere else in this file -- the
    /// ground covered in the last [`TRAIL_LIFE`] seconds -- is a statement
    /// about the wake. The nose is a fixed reach past the ball whatever it is
    /// doing (see [`NOSE_REACH`]), so counting it would put a constant on the
    /// end of a measurement whose whole point is that nothing is added to it.
    #[cfg(test)]
    pub fn wake(&self, head: Vec3) -> f32 {
        let spine = self.spine();
        let ball = spine
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.distance(head).total_cmp(&b.1.distance(head)))
            .map_or(0, |(step, _)| step);
        spine[ball..]
            .windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .sum()
    }

    /// Writes what has been woven over the shared mesh.
    ///
    /// Every attribute is replaced whole rather than edited in place: the
    /// number of trails in the world changes every frame, so there is no
    /// stable buffer to edit.
    pub fn lay(self, mesh: &mut Mesh) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colours);
        mesh.insert_indices(Indices::U32(self.indices));
    }
}

/// The meshes and materials every ball is drawn with.
///
/// Built once and shared, exactly like [`crate::pylon::GridArt`]: a kill should
/// cost a couple of entities and no allocation.
#[derive(Resource, Clone)]
pub struct Art {
    ball: Handle<Mesh>,
    /// The glow's quad, already the size the glow is drawn at, so nothing has
    /// to scale it except the breathing.
    card: Handle<Mesh>,
    /// The one mesh every trail in the world is written into, rebuilt each
    /// frame by [`trail`]. See [`Ribbons`].
    trails: Handle<Mesh>,
    /// And the one every pool of light on the ground is written into, rebuilt
    /// each frame by [`spill`]. See [`Washes`].
    washes: Handle<Mesh>,
    /// What those pools are drawn with: the same falloff the glows wear, added
    /// to the ground rather than blended over it. See [`SPILLS`].
    wash: Handle<StandardMaterial>,
    /// What every trail in the level is painted with: white, so the colour can
    /// come off the vertices. See [`Kind::tint`].
    paint: Handle<StandardMaterial>,
    /// One core and one glow per [`Kind`], indexed by [`Kind::slot`].
    core: [Handle<StandardMaterial>; 2],
    halo: [Handle<StandardMaterial>; 2],
}

/// Builds the shared art. Called from the game's own startup, beside the
/// pylons' and the stellarator's.
pub fn prepare(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Art {
    // The falloff, once, shared by every glow and every trail. White in the
    // picture and coloured by whatever draws with it, which is the split
    // `sky::sun_disc` makes -- and it is what makes a second kind of ball cost
    // a colour rather than a texture.
    let falloff = images.add(crate::sky::texture(HALO_TEXELS, |u, v| {
        let fade = (1.0 - (u * u + v * v).sqrt()).clamp(0.0, 1.0);
        [255, 255, 255, (255.0 * fade.powf(HALO_FALLOFF)) as u8]
    }));
    // Added rather than blended, which is the whole difference between
    // light falling on the grass and paint lying on it: a blended disc
    // replaces what is under it and washes the grass out towards its own
    // colour, and an added one can only ever make it brighter. Colourless
    // in the material for the trails' reason -- one mesh holds the green
    // pools and the red ones, so the colour rides on the vertices.
    //
    // Fog off, for the reason [`crate::shadow::prepare`] turns it off: fog
    // replaces a surface's colour with the sky's, and what a haze does to a
    // patch of light is make it fainter rather than sky-coloured.
    let wash = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(falloff.clone()),
        alpha_mode: AlphaMode::Add,
        depth_bias: SPILL_BIAS,
        unlit: true,
        fog_enabled: false,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    // One material, in one colour, with or without the falloff on it. The
    // cores are opaque and the glows are not, and that difference is the whole
    // of the branch: a core has to write depth, or the glow hanging behind it
    // shows straight through the sphere it is supposed to be lighting.
    let mut paint = |tint: Color, texture: Option<Handle<Image>>| {
        let lit = texture.is_some();
        let mut material = StandardMaterial {
            base_color: tint,
            base_color_texture: texture,
            // Unlit, the way the beams and the supply packet are: a ball lying
            // in a shadow still has to read as a thing worth walking to, and
            // the renderer this port draws through flattens everything else.
            //
            // No `emissive` anywhere here, and its absence is deliberate rather
            // than an oversight -- see [`HALO_SCALE`] for the two reasons this
            // build cannot show one, and for what is doing that job instead.
            unlit: true,
            ..default()
        };
        if lit {
            // A glow is seen from behind as well as in front, and casts
            // nothing: it is light, and a shadow cast by one is the tell that
            // it is a disc of glass.
            material.alpha_mode = AlphaMode::Blend;
            material.double_sided = true;
            material.cull_mode = None;
        }
        materials.add(material)
    };
    let art = Art {
        ball: meshes.add(Sphere::new(BALL_RADIUS)),
        card: meshes.add(Rectangle::new(
            BALL_RADIUS * 2.0 * HALO_SCALE,
            BALL_RADIUS * 2.0 * HALO_SCALE,
        )),
        // Empty, and kept in the main world as well as on the card: `trail`
        // rewrites it from the CPU every frame, the same way `sky::repaint`
        // keeps the dome it recolours.
        trails: meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new())
            .with_inserted_indices(Indices::U32(Vec::new())),
        ),
        washes: meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new())
            .with_inserted_indices(Indices::U32(Vec::new())),
        ),
        wash,
        paint: paint(Color::WHITE, Some(falloff.clone())),
        core: Kind::ALL.map(|kind| paint(kind.colours().0, None)),
        halo: Kind::ALL.map(|kind| paint(kind.colours().1, Some(falloff.clone()))),
    };
    commands.insert_resource(art.clone());
    // The one thing every trail is drawn by. It sits at the origin and never
    // moves, because the vertices [`trail`] writes into it are already in world
    // space -- which is the point: a single mesh can hold the trails of things
    // that have nothing else to do with each other.
    //
    // Never culled, for the same reason. Its bounds are whatever the trails in
    // it happen to span this frame, which is a box that changes every frame and
    // is usually most of the level; asking the renderer to keep that up to date
    // costs more than drawing a few thousand transparent triangles.
    //
    // Wearing the glow's own material, so a trail is the same green as the
    // thing making it by construction rather than by two numbers being kept in
    // step.
    commands.spawn((
        Ribbons,
        bevy::light::NotShadowCaster,
        NoFrustumCulling,
        Mesh3d(art.trails.clone()),
        MeshMaterial3d(art.paint.clone()),
        Transform::default(),
        Visibility::default(),
    ));
    // And the one every pool of light on the ground is drawn by, on the same
    // terms and for the same reasons: world-space vertices, never culled, one
    // draw call for every glowing thing in the level at once. See [`spill`].
    commands.spawn((
        Washes,
        bevy::light::NotShadowCaster,
        NoFrustumCulling,
        Mesh3d(art.washes.clone()),
        MeshMaterial3d(art.wash.clone()),
        Transform::default(),
        Visibility::default(),
    ));
    // The grab circle, hidden until the button is held. The squad's own ring
    // mesh, in the balls' green rather than the squad's yellow: the two circles
    // are the same mark made by the same gesture, and the colour is the only
    // thing that says which of them is open. See [`call`].
    commands.spawn((
        GrabCircle,
        bevy::light::NotShadowCaster,
        Mesh3d(meshes.add(crate::squad::ring_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.36, 1.0, 0.56, 0.85),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        })),
        Visibility::Hidden,
    ));
    art
}

/// Whether a Mario standing at `mario` can get its hands on a ball at `ball`.
///
/// **Flat, with a separate allowance up and down, and the flatness is a fix.**
/// This was a plain three-dimensional distance, and that is what stopped the
/// squad picking up what the fighting dropped. The arrival radius a fetch is
/// walked to is three quarters of [`PICKUP_RANGE`] already, which leaves about
/// a third of a metre of the budget for height -- so a ball resting anywhere
/// above a Mario's ankles was out of reach *at the exact spot the walk stops
/// at*, and the Mario stood beside it for the rest of the session. Every ball
/// the console scatters is snapped to the floor, which is why it never showed
/// up in a test: the ones that floated were the ones a kill left behind.
///
/// Grounding those at the moment they drop (see [`shed`]) is the other half of
/// the same fix. Both halves are worth having: one keeps balls where they can
/// be walked to, and this one means a ball that ends up somewhere odd anyway --
/// on a step, on a slope, over a Mario's head -- is still collectable rather
/// than silently inert.
pub fn within_reach(mario: Vec3, ball: Vec3) -> bool {
    let apart = ball - mario;
    Vec3::new(apart.x, 0.0, apart.z).length() <= PICKUP_RANGE
        && apart.y <= REACH_UP
        && apart.y >= -REACH_DOWN
}

/// Puts one ball in the world.
///
/// Shared, the way every other spawn in this port is, so the console and the
/// kill sites cannot produce subtly different balls.
pub fn spawn(commands: &mut Commands, art: &Art, kind: Kind, at: Vec3, phase: f32) -> Entity {
    let ball = commands
        .spawn((
            Orb {
                kind,
                phase,
                rest: at.y + BALL_LIFT,
                idle: 0.0,
                slack: 0.0,
            },
            core(art, kind),
            Transform::from_translation(at + Vec3::Y * BALL_LIFT),
            Visibility::default(),
        ))
        .id();
    // What it *means* is a second component, added by whoever wanted one. Both
    // kinds float and glow identically; only one of them is worth hauling.
    match kind {
        Kind::Nuclonium => {
            commands.entity(ball).insert(Nuclonium {
                held: Held::Loose { claimed: None },
            });
        }
        Kind::Medkit => {
            commands.entity(ball).insert(crate::health::Medkit);
        }
    }
    commands.entity(ball).with_child(halo(art, kind, phase));
    ball
}

/// The solid middle of one ball, as a bundle: everything but its glow and its
/// place.
///
/// Public because a unit of nuclonium is drawn in three places -- lying on the
/// lawn, flying home down the beams, and turning inside a stellarator's coils
/// (see [`crate::stellarator::Orbit`]) -- and the substance has to look like
/// one substance in all three. A second module building its own green sphere is
/// two green spheres that drift apart the first time either is tuned.
pub fn core(art: &Art, kind: Kind) -> impl Bundle + use<> {
    (
        // On the core rather than on the glow, and that is the fix rather than
        // a detail: this transform is written by the game, and the glow's is
        // written by the thing that was trying to measure it. See
        // [`TRAIL_LIFE`].
        Trail::of(kind),
        bevy::light::NotShadowCaster,
        Mesh3d(art.ball.clone()),
        MeshMaterial3d(art.core[kind.slot()].clone()),
    )
}

/// The glow that hangs on a ball, as a bundle. Spawn it as a child of the core.
///
/// Its own function because three places need one, for [`core`]'s reason, and a
/// glow assembled three times is a glow that ends up different in one of them.
pub fn halo(art: &Art, kind: Kind, phase: f32) -> impl Bundle + use<> {
    let (card, material) = (&art.card, &art.halo[kind.slot()]);
    (
        Halo { phase },
        bevy::light::NotShadowCaster,
        Mesh3d(card.clone()),
        MeshMaterial3d(material.clone()),
        // Centred on the ball. [`shimmer`] turns it to face the camera every
        // frame, so the rotation here is only what it wears for the one frame
        // before that runs.
        Transform::default(),
    )
}

/// Spawns whatever the tick's kills left behind.
///
/// Its own system so that the four places a kill happens need nothing but a
/// `ResMut<Drops>` and a position -- see [`Drops`].
pub fn shed(
    mut commands: Commands,
    art: Res<Art>,
    level: Res<LevelData>,
    mut drops: ResMut<Drops>,
) {
    let Drops { queue, shed, .. } = &mut *drops;
    for (at, kind) in queue.drain(..) {
        *shed = shed.wrapping_add(1);
        // **Put on the floor, not where the kill happened.** A kill is resolved
        // at a body's own origin, or at the point a bullet landed on it, or at
        // the middle of something that flies -- and a ball left at any of those
        // hangs in the air for ever, because nothing here falls. A Mario sent
        // to fetch one walks under it, arrives, and cannot reach it; see
        // [`within_reach`] for the other half of what that broke.
        //
        // The cast starts at the kill rather than above it, so a drop inside a
        // building lands on that building's floor rather than on its roof.
        let ground = level
            .floor_height(at)
            .map(|height| Vec3::new(at.x, height, at.z))
            .unwrap_or(at);
        spawn(
            &mut commands,
            &art,
            kind,
            ground,
            *shed as f32 * crate::squad::GOLDEN_ANGLE,
        );
    }
}

/// Picking up, carrying, and handing over.
///
/// Runs *after* the walk step, so a Mario that reached a ball this tick has it
/// this tick rather than next. Every branch here is written to survive the
/// carrier having been killed between one tick and the next, which during a
/// fight over a mast is not a rare case.
pub fn haul(
    level: Res<LevelData>,
    allies: Query<&Transform, With<Ally>>,
    // The transform is read rather than written: where a ball being carried is
    // *drawn* is [`swim`]'s, per frame. All this decides is whose it is.
    mut balls: Query<(Entity, &mut Nuclonium, &mut Orb, &Transform), Without<Ally>>,
) {
    // Who is already carrying, so a Mario that walks over a second ball on its
    // way to the mast does not end up holding two.
    let mut laden: Vec<Entity> = balls
        .iter()
        .filter_map(|(_, ball, _, _)| match ball.held {
            Held::Carried(mario) => Some(mario),
            Held::Loose { .. } | Held::Following(_) => None,
        })
        .collect();
    laden.sort_unstable();

    for (_, mut ball, mut orb, at) in &mut balls {
        match ball.held {
            Held::Loose { claimed } => {
                let Some(mario) = claimed else { continue };
                let Ok(carrier) = allies.get(mario) else {
                    // Killed on the way. The ball is nobody's again.
                    ball.held = Held::Loose { claimed: None };
                    continue;
                };
                if laden.binary_search(&mario).is_ok() {
                    continue;
                }
                if within_reach(carrier.translation, at.translation) {
                    ball.held = Held::Carried(mario);
                    laden.push(mario);
                    laden.sort_unstable();
                }
            }
            Held::Carried(mario) => {
                if allies.get(mario).is_err() {
                    // Dropped where its carrier fell, and it falls: the height
                    // it is at is left alone and the ground under it becomes
                    // what it is sinking to. See [`Orb::slack`].
                    ball.held = Held::Loose { claimed: None };
                    let dropped = at.translation;
                    let ground = level
                        .floor_height(dropped + Vec3::Y * crate::player::PLAYER_HEIGHT)
                        .map(|height| height + BALL_LIFT)
                        .unwrap_or(dropped.y);
                    orb.drop_to(dropped.y, ground);
                    continue;
                }
                // Where a carried ball is drawn is not decided here, for the
                // reason a followed one's is not: it is flown into the Mario's
                // hands by [`swim`], per drawn frame, off the pose that Mario
                // is actually drawn at. Pinning it here put it there outright
                // and did it thirty times a second, which is both halves of the
                // stutter at once.
            }
            // Luna's train is `escort`'s business, not a Mario's -- and where
            // it swims to is `swim`'s. The hand-over at the far end is
            // `escort`'s too: see there for why one place does the delivering
            // for all three ways a ball can arrive at a mast.
            Held::Following(_) => {}
        }
    }
}

/// Luna's train: what joins it, what leaves it, and what a mast takes off it.
///
/// **A ball near Luna tags along, and one very near a live mast is taken up by
/// the mast.** Neither needs a Mario, and that is the point of both: the squad
/// is a way of collecting things while you are busy elsewhere, not a toll on
/// collecting anything at all. Walking over the takings of a fight you just won
/// should pick them up, and a ball that ends up under a mast you have already
/// built should not sit there waiting for somebody to be sent for it.
///
/// **Where the train actually swims is not here.** It was, and on the fixed
/// step it juddered: see [`swim`], which does the gliding once per drawn frame
/// against the pose the player can see. What is left here is every decision --
/// joining, being dropped when whoever was leading is killed, and being handed
/// over at a mast -- because those have to happen the same number of times a
/// second on every machine.
///
/// The follow is a spring rather than a fixed offset, and deliberately a slack
/// one. A train pinned to Luna is a train that moves exactly as she does, which
/// looks like scenery welded to her back; one that lags swings out on the
/// corners, crosses over itself, and -- since a trail is however much ground
/// was covered -- draws the wake that says these are things being dragged
/// along rather than things she is wearing.
#[allow(clippy::too_many_arguments)]
pub fn escort(
    mut commands: Commands,
    art: Res<Art>,
    level: Res<LevelData>,
    network: Res<Network>,
    mut bank: ResMut<Bank>,
    leaders: Query<(Entity, &Transform), With<crate::player::Player>>,
    // Read-only for [`haul`]'s reason: nothing in this system moves a ball any
    // more, it only decides what is holding one.
    mut balls: Query<
        (Entity, &mut Nuclonium, &mut Orb, &Transform),
        Without<crate::player::Player>,
    >,
) {
    let leader = leaders.single().ok();
    for (entity, mut ball, mut orb, at) in &mut balls {
        match ball.held {
            Held::Loose { .. } => {
                // Near enough to be swept up. Claimed or not: a Mario walking
                // over for it can find another, and a ball that Luna has walked
                // past is a ball the player expected to have.
                if let Some((who, leader)) = leader {
                    // Flat, with a band up and down, for `within_reach`'s
                    // reason: a ball floats, and a ground she is walking on can
                    // be a metre below the one it came to rest on. A rule
                    // measured straight through the air is one you can stand on
                    // top of and not satisfy.
                    let apart = at.translation - leader.translation;
                    if Vec3::new(apart.x, 0.0, apart.z).length() <= MAGNET_RANGE
                        && apart.y.abs() <= MAGNET_RISE
                    {
                        ball.held = Held::Following(who);
                    }
                }
            }
            Held::Following(who) => {
                if !leader.is_some_and(|(entity, _)| entity == who) {
                    // Whoever was leading it is gone. It stays where it is and
                    // sinks to the ground under it -- see [`Orb::slack`], which
                    // is why this writes no height at all.
                    ball.held = Held::Loose { claimed: None };
                    let ground = level
                        .floor_height(at.translation + Vec3::Y * crate::player::PLAYER_HEIGHT)
                        .map(|height| height + BALL_LIFT)
                        .unwrap_or(at.translation.y);
                    orb.drop_to(at.translation.y, ground);
                    continue;
                }
                // Still hers, and *where* it swims to is not decided here. See
                // [`swim`]: the follow is drawn rather than simulated, because
                // the thing it is following is drawn between two of these ticks
                // rather than at them.
            }
            // Already over a Mario's head, put there by `haul`. Nothing to
            // steer -- but it still gets the hand-over below, which is why
            // there is one of those rather than three.
            Held::Carried(_) => {}
        }
        // Under a live mast: the mast takes it, however it got there. All three
        // ways a ball reaches one -- handed over by a Mario, dragged in behind
        // Luna, or simply lying where a mast was later built -- are the same
        // event, and a second copy of it is a second place to forget the bank.
        let here = at.translation;
        let arrived = network
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.hops.is_some())
            .find(|(_, node)| {
                let apart = node.at - here;
                Vec3::new(apart.x, 0.0, apart.z).length() <= MAST_REACH
            })
            .map(|(index, _)| index);
        if let Some(node) = arrived {
            deliver(&mut commands, &art, &mut bank, &network, entity, here, node);
        }
    }
}

/// Everything a ball can be following: Luna, and her Marios.
///
/// A named type for clippy's sake. The `Without` is load-bearing as well as
/// tidy -- a ball has a `Transform` too, and Bevy proves two `Transform`
/// queries in one system disjoint from their filters alone.
type Bodies<'w, 's> = Query<
    'w,
    's,
    &'static Transform,
    (Without<Orb>, Or<(With<crate::player::Player>, With<Ally>)>),
>;

/// Swims everything somebody is dragging along after the thing dragging it,
/// once per drawn frame.
///
/// **This is the fix for the judder, and where it runs is the whole of it.**
/// The simulation ticks thirty times a second; Luna is *drawn* between two of
/// those ticks, because [`crate::player::sync_visual`] interpolates her into
/// [`crate::player::RenderPose`] every frame -- which is what makes her glide
/// at whatever rate the monitor happens to run at. A train that chased her
/// simulation transform on the fixed step was therefore taking thirty steps a
/// second behind a leader taking a hundred and forty-four, and landing each of
/// them on a position she had already left. Nothing about the spring was wrong;
/// it was being solved against a stale target and drawn without interpolation,
/// and both of those show up as the same stutter.
///
/// So the follow is presentation. It eases towards the pose the player can
/// actually see, at the rate they see it, and the easing is frame-rate
/// independent by construction: the fraction of the remaining gap closed in
/// `dt` seconds is `1 - e^(-k*dt)`, which is the same curve however finely the
/// frames are cut. A fixed `lerp(want, 0.1)` -- the obvious spelling -- is not:
/// it chases twice as hard at twice the frame rate, so the train would sit
/// further behind on a slow machine than on a fast one.
///
/// [`escort`] keeps every *decision* -- what joins a train, what leaves one,
/// what a mast takes -- because those are simulation and have to happen the
/// same number of times per second on every machine. Only the gliding is here.
///
/// Two kinds of thing swim: the nuclonium in Luna's train, and a red ball that
/// has noticed somebody who needs it (see [`crate::health::mend`]). They differ
/// in where they aim and how hard they are pulled and in nothing else, which is
/// why one system does both rather than each growing its own copy of the
/// easing.
pub fn swim(
    time: Res<Time>,
    pose: Res<crate::player::RenderPose>,
    leaders: Query<Entity, With<crate::player::Player>>,
    bodies: Bodies,
    mut balls: Query<(
        &Orb,
        Option<&Nuclonium>,
        Option<&crate::health::Drawn>,
        &mut Transform,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let player = leaders.single().ok();
    // Where a leader is *seen* to be, which is not always where the simulation
    // has it: the player is interpolated between fixed steps and nobody else
    // is, so his rendered pose comes out of the resource and a Mario's is
    // simply where the last tick left it.
    let seen = |who: Entity| match Some(who) == player {
        true => Some(pose.translation),
        false => bodies.get(who).ok().map(|at| at.translation),
    };
    for (orb, ball, drawn, mut at) in &mut balls {
        let (goal, pull) = match (ball.map(|ball| &ball.held), drawn) {
            (Some(Held::Following(who)), _) => {
                let Some(leader) = seen(*who) else { continue };
                // Its own bearing off its own clock, so a train of eight is a
                // shoal around her rather than eight balls in one place.
                let (sin, cos) = orb.phase.sin_cos();
                (
                    leader + Vec3::new(sin, 0.0, cos) * ESCORT_ORBIT + Vec3::Y * CARRY_HEIGHT,
                    ESCORT_PULL,
                )
            }
            (Some(Held::Carried(who)), _) => {
                // Snatched up rather than assigned: the ball flies into the
                // Mario's hands from wherever it was lying. See [`CARRY_PULL`],
                // and [`haul`], which decides that it is held and deliberately
                // does not say where.
                let Some(carrier) = seen(*who) else { continue };
                (carrier + Vec3::Y * CARRY_HEIGHT, CARRY_PULL)
            }
            (None, Some(drawn)) => {
                let Some(body) = seen(drawn.toward) else {
                    continue;
                };
                (
                    body + Vec3::Y * crate::health::MEDKIT_HEIGHT,
                    crate::health::MEDKIT_PULL,
                )
            }
            _ => continue,
        };
        at.translation = at.translation.lerp(goal, 1.0 - (-pull * dt).exp());
    }
}

/// Forgets the balls nobody ever came for.
///
/// The clock is on the [`Orb`], so both colours age by the same rule, and it is
/// reset by any state that is not "lying about, unspoken for" -- a Mario's
/// claim counts, not only a Mario's hands. See [`IDLE_LIFE`] for why three
/// minutes and why the ball shrinks out rather than blinking out.
///
/// Fixed step rather than per frame: how long a thing survives is a fact about
/// the game, and a frame-rate that decided it would mean balls lasting half as
/// long on a fast machine.
pub fn linger(
    mut commands: Commands,
    mut balls: Query<(
        Entity,
        &mut Orb,
        Option<&Nuclonium>,
        Option<&crate::health::Drawn>,
    )>,
) {
    for (entity, mut orb, ball, drawn) in &mut balls {
        // A red ball that has noticed somebody is being interacted with just as
        // much as a green one somebody has claimed, and neither should expire
        // in the middle of arriving.
        let touched = drawn.is_some()
            || ball.is_some_and(|ball| !matches!(ball.held, Held::Loose { claimed: None }));
        if touched {
            orb.idle = 0.0;
            continue;
        }
        orb.idle += FIXED_DT;
        if orb.idle >= IDLE_LIFE {
            commands.entity(entity).despawn();
        }
    }
}

/// The live grab circle: how long the button has been down, where it is aimed,
/// and how wide it has grown.
///
/// The shape [`crate::squad::Whistle`] has, and a separate resource rather than
/// a shared one: they are two different circles, opened by the same button in
/// two different modes, and one of them being up must not resize the other.
#[derive(Resource, Default)]
pub struct Grab {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    pub radius: f32,
}

/// How wide the grab circle has grown after being held this long.
///
/// Linear from the instant of the press, unlike [`crate::squad::circle_radius`]
/// which spends its first fifth of a second deciding whether the press was a
/// tap. There is only one gesture in this mode, so there is nothing to tell
/// apart and no reason to make the player hold through a deadband before the
/// circle answers.
pub fn grab_radius(held_for: f32) -> f32 {
    let grown = (held_for / GRAB_GROW_SECONDS).clamp(0.0, 1.0);
    GRAB_MIN_RADIUS + (GRAB_MAX_RADIUS - GRAB_MIN_RADIUS) * grown
}

/// The ring drawn on the ground while the grab circle is open.
#[derive(Component)]
pub struct GrabCircle;

/// The ring's own transform.
///
/// Every exclusion is load-bearing, for [`crate::squad`]'s reason: Bevy proves
/// two queries disjoint from their filters alone, so a system that writes
/// `Transform` has to name every other `Transform` query beside it or the
/// schedule refuses to build -- which in a windowed build is a game that opens
/// and shuts without a word.
type CircleQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (
        With<GrabCircle>,
        Without<Camera3d>,
        Without<crate::player::Player>,
        Without<Nuclonium>,
    ),
>;

/// The whistle: a circle drawn on the ground, and everything loose inside it
/// called to heel.
///
/// **The same gesture the squad whistle is**, pointed at the balls instead of
/// at the Marios -- hold to open a circle where the view is looking, hold
/// longer to grow it, release to take what is in it. That is deliberate to the
/// point of sharing the aiming, the growth and the ring mesh with
/// [`crate::squad::whistle`]: one button now does four jobs (see
/// [`crate::action`]), and four jobs that answered to four different shapes of
/// press would be four things to learn rather than one.
///
/// It replaced a flat twenty-five metre sweep around Luna's own feet, which had
/// two things wrong with it. Nothing was drawn, so the only way to find out
/// what a press would take was to press it; and being centred on her, it could
/// not be pointed at the far side of a fight, down a slope, or at one heap of
/// balls rather than the heap beside it. A circle answers all of that by being
/// visible and aimed.
///
/// Runs at the render rate rather than on the fixed step, exactly as the squad
/// whistle does and for the same reason: the circle grows on wall-clock time
/// and is drawn every frame, while what comes out of it is a one-shot write the
/// fixed step then acts on.
#[allow(clippy::too_many_arguments)]
pub fn call(
    time: Res<Time>,
    mut input: ResMut<crate::input::InputState>,
    level: Res<LevelData>,
    mut grab: ResMut<Grab>,
    camera: Query<&Transform, (With<Camera3d>, Without<crate::player::Player>)>,
    leaders: Query<(Entity, &Transform), With<crate::player::Player>>,
    mut balls: Query<(&mut Nuclonium, &Transform), Without<crate::player::Player>>,
    mut ring: CircleQuery,
) {
    let (Ok(camera), Ok((who, leader))) = (camera.single(), leaders.single()) else {
        return;
    };
    let released = crate::input::InputState::take(&mut input.grab_released);
    if input.grab || released {
        // Refreshed on the release as well as on the hold, so a press too short
        // to have been seen held still lands its circle where the player was
        // looking rather than wherever the last one was.
        grab.aim = crate::squad::aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
        );
    }
    if input.grab {
        let held = grab.held_for.unwrap_or(0.0) + time.delta_secs();
        grab.held_for = Some(held);
        grab.radius = grab_radius(held);
    }
    if released {
        // Recomputed from the hold rather than read off the resource, because a
        // press and its release inside one frame never grew anything: the
        // radius sitting there would be the *previous* circle's.
        grab.radius = grab_radius(grab.held_for.take().unwrap_or(0.0));
        for (mut ball, at) in &mut balls {
            // The circle's own height rule, out of [`crate::squad::in_circle`]:
            // a circle is a flat thing drawn on the ground and reads as one, so
            // a ball on the parapet above is not in a circle drawn on the lawn
            // beneath it.
            if matches!(ball.held, Held::Loose { .. })
                && crate::squad::in_circle(at.translation, grab.aim, grab.radius)
            {
                ball.held = Held::Following(who);
            }
        }
    }
    if let Ok((mut transform, mut visibility)) = ring.single_mut() {
        let showing = grab.held_for.is_some();
        *visibility = match showing {
            true => Visibility::Visible,
            false => Visibility::Hidden,
        };
        if showing {
            // Just clear of the ground, so the ring is not half-buried in the
            // slope it is drawn on.
            transform.translation = grab.aim + Vec3::Y * 0.05;
            transform.scale = Vec3::new(grab.radius, 1.0, grab.radius);
        }
    }
}

/// Hands one ball over to the mast at `node` and sends it home.
///
/// Shared by the Mario's hand-over and the mast's own reach, because they are
/// the same event seen from two sides -- and a second copy of it is a second
/// place the bank can be forgotten.
fn deliver(
    commands: &mut Commands,
    art: &Art,
    bank: &mut Bank,
    network: &Network,
    ball: Entity,
    at: Vec3,
    node: usize,
) {
    commands.entity(ball).despawn();
    let Some(mut legs) = network.supply_route(node) else {
        // A mast that says it has power but has no route to the machine that
        // gives it any is a contradiction the network cannot produce. Crediting
        // it anyway is the honest failure: the work was done.
        bank.stored += 1;
        return;
    };
    // **The flight starts where the ball is, not where the network is.** The
    // route [`crate::pylon::Network::supply_route`] hands back begins at the
    // mast's own head, which is up to [`MAST_REACH`] away and several metres up
    // -- so a ball handed in at the foot of a mast used to vanish from the
    // player's hand and reappear at the top of the tower, mid-flight, on the
    // next tick. Nothing in this game may change place without travelling: the
    // route gets one more leg on the front of it, and what the player watches
    // is the ball rising off the grass into the beams it is about to fly down.
    //
    // Prepended here rather than inside `supply_route` because it is not part
    // of the route. Where the network carries something from a mast is a
    // property of the network; where a particular ball happened to be lying
    // when the mast reached for it is a property of the ball.
    if legs.first().is_some_and(|first| first.distance(at) > 1e-3) {
        legs.insert(0, at);
    }
    let shipment = commands
        .spawn((
            Shipment { legs, along: 0.0 },
            core(art, Kind::Nuclonium),
            Transform::from_translation(at),
            Visibility::default(),
        ))
        .id();
    // Wearing the same glow, so what the player follows across the valley looks
    // like the thing that was handed in.
    commands
        .entity(shipment)
        .with_child(halo(art, Kind::Nuclonium, 0.0));
}

/// Flies delivered balls home along the beams, and hands what arrives to the
/// machine it arrived at.
///
/// One straight run through a list of points, advanced in metres. When it runs
/// off the end it has reached a machine, and two counters go up: the global
/// [`Bank`], which is what the player is shown, and that one machine's
/// [`crate::stellarator::Store`], which is what the machine is *drawn* out of.
///
/// **The machine is found by where the route ended rather than remembered from
/// where it started.** The last leg of a supply route is a machine's own feed
/// point -- [`crate::pylon::Network::feeds`] is built out of them -- so the
/// nearest machine to the end of the flight is the machine at the end of the
/// flight, exactly, and nothing has to hold an entity across a stellarator
/// being demolished mid-flight. A shipment that finds nothing there still
/// banks: the squad did the work, and losing the count because a building came
/// down under it would be a silent theft.
/// The machines a shipment can land at.
///
/// A named type for clippy's sake, and the `Without` is load-bearing for
/// [`crate::squad`]'s reason: two `Transform` queries in one system have to
/// name each other or the schedule refuses to build.
type Machines<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, &'static mut crate::stellarator::Store),
    (With<crate::stellarator::Stellarator>, Without<Shipment>),
>;

pub fn ship(
    mut commands: Commands,
    mut bank: ResMut<Bank>,
    mut machines: Machines,
    mut flying: Query<
        (Entity, &mut Shipment, &mut Transform),
        Without<crate::stellarator::Stellarator>,
    >,
) {
    for (entity, mut shipment, mut at) in &mut flying {
        shipment.along += SHIP_SPEED * FIXED_DT;
        match point_along(&shipment.legs, shipment.along) {
            Some(point) => at.translation = point,
            None => {
                commands.entity(entity).despawn();
                bank.stored += 1;
                let landed = at.translation;
                let nearest = machines
                    .iter_mut()
                    .min_by(|a, b| {
                        a.0.translation
                            .distance_squared(landed)
                            .total_cmp(&b.0.translation.distance_squared(landed))
                    })
                    .map(|(_, store)| store);
                if let Some(mut store) = nearest {
                    store.held += 1;
                }
            }
        }
    }
}

/// Where `along` metres down a polyline is, or `None` past the end of it.
///
/// Its own function so the flight can be exercised without a world: a route is
/// a list of points and a distance, and nothing about walking one needs an ECS.
pub fn point_along(legs: &[Vec3], along: f32) -> Option<Vec3> {
    if legs.len() < 2 {
        return None;
    }
    let mut left = along.max(0.0);
    for pair in legs.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let length = from.distance(to);
        if length <= 1e-4 {
            continue;
        }
        if left <= length {
            return Some(from.lerp(to, left / length));
        }
        left -= length;
    }
    None
}

/// Bobs the loose balls and turns every glow to face the camera.
///
/// Render rate, because it is only how a thing looks. A carried one is left
/// alone: it is pinned over its Mario's head by [`haul`], and bobbing it as well
/// would fight that write every frame.
///
/// **Nothing here measures motion any more.** It used to, and the flicker that
/// came of it is described at [`TRAIL_LIFE`]: a glow that both reads its own
/// world position and writes its own offset is a loop. Speed is now somebody
/// else's question entirely -- [`trail`] asks it of the ball, which is the thing
/// that actually moves.
#[allow(clippy::type_complexity)]
pub fn shimmer(
    time: Res<Time>,
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<Halo>)>,
    mut balls: Query<
        (
            &mut Orb,
            Option<&Nuclonium>,
            Option<&crate::health::Drawn>,
            &mut Transform,
        ),
        Without<Halo>,
    >,
    mut halos: Query<(&Halo, &ChildOf, &mut Transform), (Without<Nuclonium>, Without<Camera3d>)>,
    // Whatever each glow is hanging on, read rather than written: where the
    // card goes is a question about where its ball *is*, and taking that off
    // the card's own transform is the feedback loop described at
    // [`TRAIL_LIFE`]. Read-only, so it conflicts with nothing above.
    hangs_on: Query<&GlobalTransform, Without<Halo>>,
) {
    let elapsed = time.elapsed_secs();
    // The glows breathe whatever their ball is doing -- carried, loose or flying
    // home -- because one that stopped pulsing the moment somebody picked it up
    // would read as the thing having been switched off.
    //
    // Turned to face the camera as well, which is the half that makes it a glow
    // rather than a card seen edge-on. `Rectangle` faces local +Z, and the
    // parent is never rotated (see [`Halo`]), so the world heading goes straight
    // into the local rotation. The position is last frame's -- a `GlobalTransform`
    // written before this ball moved -- which for a radially symmetric picture is
    // a fraction of a degree nobody can see.
    if let Ok(view) = camera.single() {
        let eye = view.translation();
        for (halo, hanging, mut at) in &mut halos {
            let Ok(ball) = hangs_on.get(hanging.parent()) else {
                continue;
            };
            let ball = ball.compute_transform();
            let Some(toward) = (eye - ball.translation).try_normalize() else {
                continue;
            };
            let swell =
                1.0 + HALO_SWELL * (elapsed * std::f32::consts::TAU * BOB_HZ + halo.phase).sin();
            at.rotation = Quat::from_rotation_arc(Vec3::Z, toward);
            // Drawn nearer than it hangs, by its own radius. **This is the
            // hard edge across the bottom of a glowing ball**, and it is the
            // depth buffer rather than anything about the picture: a card
            // aimed at the camera is a flat plane standing through the thing
            // it is drawn on, so the half of it below the ball is inside the
            // ground and the half above is not, and what the eye gets is a
            // soft round glow with a ruler-straight bite out of it along the
            // line where the two surfaces cross. A ball floating beside a wall
            // is the same picture turned on its side.
            //
            // Scaled about the eye, exactly as [`crate::shadow::FLOAT`] lifts a
            // shadow off a hillside: under a perspective projection that leaves
            // the card on precisely the pixels it was on at precisely the size,
            // and changes only the depth it is tested at. How far is
            // [`HALO_FLOAT`], and it is bounded by the ball rather than by the
            // ground.
            let reach = GLOW_RADIUS * ball.scale.y * swell * HALO_FLOAT;
            let near = crate::shadow::float_toward(eye, ball.translation, reach);
            at.scale = Vec3::splat(swell * near);
            // The offset is in the ball's own units, which is what makes it
            // right for a mote a fiftieth the size as well: the parent's scale
            // multiplies it back into the world.
            at.translation = toward * (GLOW_RADIUS * swell * HALO_FLOAT);
        }
    }
    let dt = time.delta_secs();
    for (mut orb, ball, drawn, mut at) in &mut balls {
        // Every ball dwindles, whoever is holding it -- a ball nobody came for
        // is a ball nobody came for even if a mast is about to reach it. See
        // [`dwindle`]; the glow is a child and shrinks with it, and the trail
        // reads its width off this same scale.
        at.scale = Vec3::splat(dwindle(orb.idle));
        // Anything in somebody's hands, swimming along behind Luna, or drifting
        // towards somebody who needs it is placed by whoever is moving it --
        // see [`swim`]. Bobbing it as well would be two systems writing one
        // transform and arguing about it every frame.
        if drawn.is_some() || ball.is_some_and(|ball| !matches!(ball.held, Held::Loose { .. })) {
            // Anything held is somewhere it was carried to rather than
            // somewhere it fell, so a fall it never finished is not owed to it.
            orb.slack = 0.0;
            continue;
        }
        // What is left of the drop it was let go from, eased away on the same
        // curve everything else in this module eases on. See [`Orb::slack`].
        orb.slack *= (-SETTLE_PULL * dt).exp();
        // About `rest` rather than about wherever it currently is. Bobbing a
        // transform relative to itself is an integration, and an integration of
        // a sine sampled at the frame rate drifts -- downwards, into the floor,
        // over a minute of standing still.
        at.translation.y = orb.rest
            + orb.slack
            + BOB_RISE * (elapsed * std::f32::consts::TAU * BOB_HZ + orb.phase).sin();
    }
}

/// Lays down where everything made of nuclonium has been, and redraws the one
/// mesh all of it is trailed with.
///
/// Render rate, beside [`shimmer`], because a trail is a picture of the last
/// half second and a picture is owed one per frame.
///
/// Reads `GlobalTransform`, so a mote inside a machine and a ball on the lawn
/// are the same case: what is recorded is where the thing ended up in the
/// world, whoever moved it and whatever it is parented to. The value is one
/// frame old -- propagation runs after this -- which matters not at all for a
/// history whose whole point is that it is behind.
///
/// The width of a trail comes off the same transform, so it is the width of the
/// glow that is making it. That one line is what lets a ball on the lawn and a
/// mote a fiftieth its size share every other number in this file.
pub fn trail(
    time: Res<Time>,
    art: Res<Art>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<Trail>)>,
    mut trails: Query<(&mut Trail, &GlobalTransform)>,
) {
    let Ok(view) = camera.single() else {
        // No camera, no picture. The histories are deliberately *not* aged in
        // this case: a frame nobody saw should not eat anybody's trail.
        return;
    };
    let eye = view.translation();
    let dt = time.delta_secs();
    let mut ribbon = Ribbon::default();
    for (mut trail, global) in &mut trails {
        let here = global.translation();
        let width = GLOW_RADIUS * global.compute_transform().scale.y * TRAIL_WIDTH;
        trail.fade(dt);
        trail.record(here, width * TRAIL_STEP, width * TRAIL_JUMP);
        ribbon.weave(here, &trail, eye, width);
    }
    if let Some(mut mesh) = meshes.get_mut(&art.trails) {
        ribbon.lay(&mut mesh);
    }
}

/// The light a ball throws on the ground under it: how many balls do it at
/// once, how far out from the camera one still does, how wide the pool is
/// beside the glow making it, how bright it starts, and how far the ball can
/// float above the floor before there is none of it left.
///
/// **This is what "emissive" has to mean in this renderer, and it is not a
/// compromise.** There is no light entity in this game and no shader that would
/// read one: every surface in the world is on [`crate::n64::N64Material`],
/// whose whole lighting model is one key direction and an ambient floor shared
/// by the entire level -- see that module, which says plainly why there is no
/// `DirectionalLight` either. A `PointLight` next to a ball would be a
/// component nothing looks at. And the two ways a `StandardMaterial` can put
/// `emissive` on the screen are both missing as well, for the reasons written
/// out at [`HALO_SCALE`].
///
/// The console had exactly this problem and answered it the same way. It could
/// not cast a shadow either, so a shadow was *geometry*: a soft disc laid on
/// the floor under the thing, faded by how high up it was. That is
/// [`crate::shadow`], and this is that idea with the sign flipped -- a bright
/// disc, added to the ground rather than multiplied into it, laid under
/// something that gives off light instead of under something that blocks it.
/// The two even share the placement, because "where is the floor under this,
/// and which way does it lean" is one question with one right answer:
/// [`crate::shadow::place`] is what both ask.
///
/// What it buys is that a ball lying in the dark now reads as a lamp rather
/// than as a sticker: the grass under it is green, the pool breathes as the
/// ball bobs, and a Mario carrying one lights the ground as it walks. What it
/// does not do is light a wall the ball is floating beside -- for the same
/// reason a shadow in this game never falls on one.
///
/// Sixteen at a time, nearest the camera first. The count is a bound on the
/// ground queries rather than on the drawing: every pool in the level is one
/// mesh and one draw call -- the trails' trick, and for the trails' reason --
/// but each one has to ask the collision where the floor under it is, and five
/// hundred motes turning inside a machine would be five hundred of those a
/// frame for a picture the size of a coin.
const SPILLS: usize = 16;
const SPILL_RANGE: f32 = 30.0;
const SPILL_SPREAD: f32 = 3.4;
const SPILL_STRENGTH: f32 = 0.55;
const SPILL_REACH: f32 = 4.0;
const SPILL_BIAS: f32 = 500.0;

/// How bright the pool under a ball `drop` metres above its floor is, as a
/// share of [`SPILL_STRENGTH`].
///
/// Squared rather than linear, which is what a light falling off with distance
/// does and also what makes the bob visible: a ball at the top of its float is
/// noticeably dimmer on the grass than one at the bottom, so the pool breathes
/// with it instead of sitting there at a constant brightness while the thing
/// making it moves.
pub fn pooled(drop: f32) -> f32 {
    let left = (1.0 - drop / SPILL_REACH).clamp(0.0, 1.0);
    left * left
}

/// How much of its light a ball `apart` metres from the camera still lays down.
///
/// Only the [`SPILLS`] nearest balls get a pool at all, so without this the
/// sixteenth and seventeenth swap places as the camera turns and a pool blinks
/// on and off in the distance. Fading the last quarter of the range to nothing
/// means the two that swap are both at nothing when they do it.
pub fn far_off(apart: f32) -> f32 {
    let start = SPILL_RANGE * 0.75;
    (1.0 - (apart - start) / (SPILL_RANGE - start)).clamp(0.0, 1.0)
}

/// The one mesh every pool of light in the world is drawn out of. See
/// [`Ribbons`], which is the same arrangement for the same reason.
#[derive(Component)]
pub struct Washes;

/// One frame's worth of pools, being built.
///
/// Its own type beside [`Ribbon`], filled by a pure method, so what a pool
/// actually looks like can be asserted without a camera, a mesh or a frame.
#[derive(Default)]
pub struct Wash {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colours: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Wash {
    /// Adds one pool: a square of the glow's own picture, lying flat on the
    /// ground at `lying` and turned to face along `normal`.
    ///
    /// Square rather than a fan of triangles because the picture is round
    /// already -- the falloff reaches nothing well before the corners, so what
    /// is drawn is a circle and the corners cost four vertices rather than
    /// twenty-four. The same reason the glow itself is a card.
    pub fn pour(&mut self, lying: Vec3, normal: Vec3, radius: f32, tint: [f32; 3], strength: f32) {
        if radius <= 0.0 || strength <= 0.0 {
            return;
        }
        // Any two directions across the ground will do: the picture is
        // radially symmetric, so which way round the square is laid cannot be
        // seen.
        let across = normal.any_orthonormal_vector();
        let along = normal.cross(across);
        let base = self.positions.len() as u32;
        for (side, up) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let at = lying + across * (radius * side) + along * (radius * up);
            self.positions.push([at.x, at.y, at.z]);
            self.uvs.push([side * 0.5 + 0.5, up * 0.5 + 0.5]);
            let [red, green, blue] = tint;
            self.colours.push([red, green, blue, strength]);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    }

    /// How many pools are in it. For the tests.
    #[cfg(test)]
    pub fn pools(&self) -> usize {
        self.indices.len() / 6
    }

    /// The brightness each pool was poured at, in the order they were poured.
    /// For the tests.
    #[cfg(test)]
    pub fn strengths(&self) -> Vec<f32> {
        self.colours
            .chunks_exact(4)
            .map(|quad| quad[0][3])
            .collect()
    }

    /// Writes what has been poured over the shared mesh. [`Ribbon::lay`]'s
    /// twin, and whole-attribute for its reason.
    pub fn lay(self, mesh: &mut Mesh) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colours);
        mesh.insert_indices(Indices::U32(self.indices));
    }
}

/// Lays down the light every ball throws on the ground beneath it.
///
/// Render rate, beside [`trail`] and for the same reason: it is a picture of
/// where everything is right now, and a picture is owed one per frame.
///
/// The nearest [`SPILLS`] balls to the camera and no others -- see there. The
/// pick is a partial sort rather than a full one: nothing here cares what order
/// the sixteen come in, only which sixteen they are, so the list is split about
/// the sixteenth in linear time and the rest is left as it lies.
pub fn spill(
    level: Res<LevelData>,
    gravity: Res<crate::gravity::Gravity>,
    art: Res<Art>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<Orb>)>,
    balls: Query<(&Orb, &GlobalTransform)>,
) {
    let Ok(view) = camera.single() else {
        return;
    };
    let eye = view.translation();
    let mut lit: Vec<(f32, Vec3, f32, Kind)> = balls
        .iter()
        .map(|(orb, global)| {
            let here = global.translation();
            let scale = global.compute_transform().scale.y;
            (here.distance(eye), here, scale, orb.kind)
        })
        .filter(|(apart, _, scale, _)| *apart < SPILL_RANGE && *scale > 0.0)
        .collect();
    if lit.len() > SPILLS {
        lit.select_nth_unstable_by(SPILLS, |a, b| a.0.total_cmp(&b.0));
        lit.truncate(SPILLS);
    }
    let mut wash = Wash::default();
    for (apart, here, scale, kind) in lit {
        let up = gravity.up(here);
        // The same question the shadow asks, asked the same way: where is the
        // floor under this, and which way does it lean.
        let Some((floor, slope)) = crate::shadow::place(&level, here, up) else {
            continue;
        };
        let drop = (here - floor).dot(up).max(0.0);
        let radius = GLOW_RADIUS * scale * SPILL_SPREAD;
        let lying = floor + slope * crate::shadow::LIFT;
        wash.pour(
            lying,
            slope,
            radius,
            kind.tint(),
            SPILL_STRENGTH * pooled(drop) * far_off(apart),
        );
    }
    if let Some(mut mesh) = meshes.get_mut(&art.washes) {
        wash.lay(&mut mesh);
    }
}

/// Carries out `nuclonium <n>` and `nuclonium clear` from the console.
///
/// A scattering of balls around the player, so the fetch-and-ship chain can be
/// looked at without waiting for the drop rate to produce one. In the overlay
/// with the rest of the console's requests, for [`crate::pylon::command`]'s
/// reason: the console is open at the moment the line is typed.
pub fn command(
    mut commands: Commands,
    art: Res<Art>,
    level: Res<LevelData>,
    mut console: ResMut<crate::console::ConsoleState>,
    player: Query<&Transform, With<crate::player::Player>>,
    standing: Query<(Entity, &Orb)>,
) {
    for request in console.take_requests() {
        let (kind, count) = match request {
            // `medkit 0` is how the red ones are swept, for the same reason
            // there is no `medkit clear`: a count of none is a clear already.
            crate::console::Request::ClearNuclonium => (Kind::Nuclonium, 0),
            crate::console::Request::Nuclonium(count) => (Kind::Nuclonium, count),
            crate::console::Request::Medkits(count) => (Kind::Medkit, count),
            other => {
                console.defer(other);
                continue;
            }
        };
        // Only the kind being asked about. Scattering medkits to look at them
        // should not sweep up the nuclonium the squad has been collecting,
        // which is what one `clear` over everything did.
        for (ball, orb) in &standing {
            if orb.kind == kind {
                commands.entity(ball).despawn();
            }
        }
        let Ok(player) = player.single() else {
            continue;
        };
        let centre = player.translation;
        for step in 0..count {
            // The same spiral the squad's formation slots use, so a dozen balls
            // land spread out rather than in a stack.
            let offset = crate::squad::slot(step + 1, 2.4);
            let spot = centre + Vec3::new(offset.x, 0.0, offset.y);
            let Some(height) = level.floor_height(spot + Vec3::Y * 20.0) else {
                continue;
            };
            spawn(
                &mut commands,
                &art,
                kind,
                Vec3::new(spot.x, height, spot.z),
                step as f32 * crate::squad::GOLDEN_ANGLE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use std::f32::consts::SQRT_2;

    /// The far end of a trail recedes; it does not hop.
    ///
    /// **This is the abrupt transition, written down as a number.** A trail is
    /// the ground covered in the last [`TRAIL_LIFE`] seconds, so something
    /// travelling at a steady speed in a straight line should draw a ribbon of
    /// exactly one length, frame after frame, for ever. Ending the ribbon at
    /// the oldest mark that has not yet expired does not do that: that mark's
    /// age sawtooths between [`TRAIL_LIFE`] and one [`MARK_INTERVAL`] short of
    /// it, so the drawn length sawtooths with it -- the tail sits still while
    /// the marks age and then snaps back a whole mark's worth of ground. At
    /// walking pace that is a couple of centimetres and invisible; on a
    /// shipment crossing the valley it is half a metre, twenty times a second.
    ///
    /// So the test is on the *drawn* ribbon rather than on the history behind
    /// it, and it asks for two things: that the length is what the travelling
    /// says it should be, and that it stops changing once the trail is full.
    #[test]
    fn a_trails_tail_slides_back_rather_than_hopping_a_mark_at_a_time() {
        let (step, speed, width) = (1.0 / 60.0, 6.0_f32, 0.5);
        let mut trail = Trail::default();
        let mut at = Vec3::ZERO;
        let mut drawn: Vec<f32> = Vec::new();
        for frame in 0..240 {
            trail.fade(step);
            at += Vec3::X * speed * step;
            trail.record(at, width * TRAIL_STEP, width * TRAIL_JUMP);
            let mut ribbon = Ribbon::default();
            // From above, so the ribbon lies in the ground plane and its rungs
            // are square to the way the thing is going.
            ribbon.weave(at, &trail, at + Vec3::Y * 20.0, width);
            // Only once the history has filled: the first half-second is a
            // trail growing, and it is meant to grow.
            if frame as f32 * step > TRAIL_LIFE * 2.0 {
                drawn.push(ribbon.wake(at));
            }
        }
        let wanted = speed * TRAIL_LIFE;
        let (low, high) = drawn
            .iter()
            .fold((f32::MAX, 0.0_f32), |(low, high), length| {
                (low.min(*length), high.max(*length))
            });
        assert!(
            (low - wanted).abs() < wanted * 0.02 && (high - wanted).abs() < wanted * 0.02,
            "a trail at {speed} m/s should be {wanted} m long, and ran {low}..{high}"
        );
        assert!(
            high - low < wanted * 0.01,
            "the drawn length wobbled by {} m, which is the tail hopping",
            high - low
        );
    }

    /// A ball nobody ever comes for gives up and goes, and one somebody has
    /// spoken for does not.
    ///
    /// Driven at the component rather than through three minutes of frames:
    /// what is worth pinning is the rule, and a test that really waited out
    /// [`IDLE_LIFE`] would be the slowest one in the suite by two orders of
    /// magnitude.
    #[test]
    fn a_ball_nobody_ever_comes_for_gives_up_and_goes() {
        let mut world = World::new();
        let orb = |idle| Orb {
            kind: Kind::Nuclonium,
            phase: 0.0,
            rest: 0.0,
            idle,
            slack: 0.0,
        };
        // Both are one tick short of their three minutes.
        let nearly = IDLE_LIFE - FIXED_DT * 0.5;
        let forgotten = world
            .spawn((
                orb(nearly),
                Nuclonium {
                    held: Held::Loose { claimed: None },
                },
            ))
            .id();
        let mario = world.spawn_empty().id();
        let spoken_for = world
            .spawn((
                orb(nearly),
                Nuclonium {
                    held: Held::Loose {
                        claimed: Some(mario),
                    },
                },
            ))
            .id();
        world.run_system_once(linger).unwrap();
        assert!(
            world.get::<Orb>(forgotten).is_none(),
            "a ball nobody has touched in three minutes is still lying there"
        );
        assert!(
            world.get::<Orb>(spoken_for).is_some(),
            "a ball a Mario is walking to expired underneath it"
        );
        assert_eq!(
            world.get::<Orb>(spoken_for).map(|orb| orb.idle),
            Some(0.0),
            "being claimed did not put the clock back to nothing"
        );
    }

    /// The shrink is at the end and nowhere else.
    #[test]
    fn a_ball_is_full_size_until_the_moment_it_leaves() {
        assert_eq!(dwindle(0.0), 1.0);
        assert_eq!(
            dwindle(IDLE_LIFE - IDLE_FADE - 1.0),
            1.0,
            "a ball a minute old is the same size as a fresh one, which is what \
             stops its size lying about how far away it is"
        );
        let half = dwindle(IDLE_LIFE - IDLE_FADE * 0.5);
        assert!((half - 0.5).abs() < 1e-3, "half way out it was {half}");
        assert_eq!(dwindle(IDLE_LIFE), 0.0, "and it reaches nothing exactly");
        assert_eq!(dwindle(IDLE_LIFE * 2.0), 0.0);
    }

    /// The grab circle opens on the press and grows to a cap.
    #[test]
    fn the_grab_circle_opens_at_once_and_grows_to_a_cap() {
        assert_eq!(
            grab_radius(0.0),
            GRAB_MIN_RADIUS,
            "the shortest possible press still takes what is at your feet"
        );
        let half = grab_radius(GRAB_GROW_SECONDS * 0.5);
        assert!(half > GRAB_MIN_RADIUS && half < GRAB_MAX_RADIUS, "{half}");
        assert_eq!(grab_radius(GRAB_GROW_SECONDS), GRAB_MAX_RADIUS);
        assert_eq!(grab_radius(60.0), GRAB_MAX_RADIUS, "and stops there");
        // Wider than the squad's at both ends, because a ball is a smaller
        // thing to have missed than a Mario is. See [`GRAB_MIN_RADIUS`].
        assert!(GRAB_MIN_RADIUS > crate::squad::circle_radius(0.0));
        assert!(GRAB_MAX_RADIUS > crate::squad::circle_radius(60.0));
    }

    #[test]
    fn one_kill_in_twenty_leaves_something_behind() {
        let mut drops = Drops::default();
        let (mut dropped, mut kits) = (0, 0);
        for _ in 0..2000 {
            match drops.maybe(Vec3::ZERO) {
                Some(Kind::Nuclonium) => dropped += 1,
                Some(Kind::Medkit) => kits += 1,
                None => {}
            }
        }
        // Five percent of two thousand is a hundred. A Weyl sequence is not a
        // coin flip and is held to a much tighter band than one would be: what
        // would fail here is somebody changing the step to something rational,
        // which collapses the sequence onto a handful of values.
        assert!(
            (95..=105).contains(&dropped),
            "{dropped} drops out of 2000, wanted about 100"
        );
        // And the red band beside it: rarer, off the same one walk. Both
        // being right at once is the thing worth pinning -- two bands of one
        // Weyl sequence is the trick, and a fat-fingered bound shows up as one
        // rate stealing from the other rather than as a total that is wrong.
        assert!(
            (72..=88).contains(&kits),
            "{kits} medkits out of 2000, wanted about 80"
        );
        assert_eq!(
            drops.queue.len(),
            dropped + kits,
            "every drop queued a ball"
        );
    }

    #[test]
    fn the_drops_are_spread_rather_than_clumped() {
        // The other half of choosing a Weyl sequence: no long dry spell. Over
        // any hundred kills there is at least one ball, which a real 5% coin
        // misses about half a percent of the time -- rarely enough to be a bug
        // report and often enough to happen.
        let mut drops = Drops::default();
        let mut since = 0;
        let mut longest = 0;
        for _ in 0..2000 {
            if drops.maybe(Vec3::ZERO) == Some(Kind::Nuclonium) {
                longest = longest.max(since);
                since = 0;
            } else {
                since += 1;
            }
        }
        assert!(longest < 40, "{longest} kills with nothing dropped");
    }

    #[test]
    fn a_claim_by_somebody_who_has_died_is_not_a_claim() {
        let me = Entity::from_raw_u32(1).unwrap();
        let ghost = Entity::from_raw_u32(2).unwrap();
        let alive = |who: Entity| who == me;
        let loose = Nuclonium {
            held: Held::Loose { claimed: None },
        };
        assert!(loose.available(me, alive));
        let mine = Nuclonium {
            held: Held::Loose { claimed: Some(me) },
        };
        assert!(mine.available(me, alive), "my own claim is not a refusal");
        let theirs = Nuclonium {
            held: Held::Loose {
                claimed: Some(ghost),
            },
        };
        assert!(
            theirs.available(me, alive),
            "a dead claimant frees the ball"
        );
        let carried = Nuclonium {
            held: Held::Carried(ghost),
        };
        assert!(!carried.available(me, alive), "something held is not loose");
        let trailing = Nuclonium {
            held: Held::Following(me),
        };
        assert!(
            !trailing.available(me, alive),
            "a ball already swimming after Luna is not one to be fetched"
        );
    }

    /// A Mario reaches sideways, not through the air.
    ///
    /// The regression this is here for: a ball a metre off the ground, which is
    /// where a kill used to leave one, standing exactly where a fetch walks to.
    #[test]
    fn a_mario_can_pick_up_what_it_walked_to_even_if_it_is_floating() {
        let mario = Vec3::ZERO;
        // Where the walk actually stops: the arrival radius a fetch is given.
        let arrive = PICKUP_RANGE * 0.75;
        assert!(within_reach(mario, Vec3::new(arrive, BALL_LIFT, 0.0)));
        assert!(
            within_reach(mario, Vec3::new(arrive, 1.0, 0.0)),
            "a ball left at a slime's chest height was unreachable"
        );
        // Overhead is a limit, though: something on the parapet above is not
        // a thing a Mario on the ground has its hands on.
        assert!(!within_reach(mario, Vec3::new(0.0, 4.0, 0.0)));
        assert!(!within_reach(mario, Vec3::new(0.0, -4.0, 0.0)));
        // And sideways is still the range it always was.
        assert!(!within_reach(
            mario,
            Vec3::new(PICKUP_RANGE + 0.1, 0.0, 0.0)
        ));
    }

    /// A trail is however much ground was covered, and nothing else.
    #[test]
    fn a_trail_is_as_long_as_the_travelling_made_it() {
        // Marked every tenth of a metre, expiring after `TRAIL_LIFE`.
        let (spacing, jump, dt) = (0.1, 100.0, 1.0 / 60.0);
        let mut walk = Trail::default();
        let mut sprint = Trail::default();
        for step in 0..60 {
            let along = step as f32 * dt;
            walk.fade(dt);
            walk.record(Vec3::new(along * 2.0, 0.0, 0.0), spacing, jump);
            sprint.fade(dt);
            sprint.record(Vec3::new(along * 8.0, 0.0, 0.0), spacing, jump);
        }
        assert!(!walk.is_empty(), "something moving laid no trail at all");
        // **Four times the speed, four times the trail.** Nothing anywhere sets
        // a length; this is the consequence of a fixed lifetime and a distance
        // gate, and it is the whole design.
        let ratio = sprint.span() / walk.span();
        assert!(
            (3.0..5.0).contains(&ratio),
            "a sprint left a trail {ratio} times a walk's, wanted about four"
        );
        // And it is about what the lifetime says it should be: half a second of
        // travel at two metres a second is a metre or so.
        assert!(
            (0.6..1.1).contains(&walk.span()),
            "a walk's trail spanned {}",
            walk.span()
        );

        // And the mark budget is not allowed to become the answer. A shipment
        // crosses the valley at `SHIP_SPEED`, which is fast enough to fill the
        // history in a third of the time it is meant to cover; see
        // [`MARK_INTERVAL`], which is the whole reason it still comes out at a
        // full lifetime of travel rather than at whatever twenty-odd marks
        // happened to reach back to.
        let mut flying = Trail::default();
        for step in 0..120 {
            flying.fade(dt);
            flying.record(
                Vec3::new(step as f32 * dt * SHIP_SPEED, 0.0, 0.0),
                spacing,
                jump,
            );
        }
        let wanted = SHIP_SPEED * TRAIL_LIFE;
        assert!(
            (flying.span() - wanted).abs() < wanted * 0.2,
            "a shipment's trail spanned {} of a wanted {wanted}",
            flying.span()
        );
    }

    /// Standing still is no trail -- after a moment, and not before.
    #[test]
    fn a_trail_hangs_where_it_was_and_then_goes_out() {
        let (spacing, jump, dt) = (0.1, 100.0, 1.0 / 60.0);
        let mut trail = Trail::default();
        for step in 0..30 {
            trail.fade(dt);
            trail.record(Vec3::new(step as f32 * 0.1, 0.0, 0.0), spacing, jump);
        }
        let ran = trail.span();
        assert!(ran > 1.0, "the run laid nothing: {ran}");
        // Now stop dead. The trail is still there for a moment -- it hangs --
        // and it shortens from the far end as the marks go out.
        for _ in 0..12 {
            trail.fade(dt);
            trail.record(Vec3::new(2.9, 0.0, 0.0), spacing, jump);
        }
        let hanging = trail.span();
        assert!(
            hanging > 0.0 && hanging < ran,
            "a stopped trail went from {ran} to {hanging}"
        );
        // And a moment later there is nothing left of it, which is the half the
        // old stretched card could never do: it drew a tail on a ball that had
        // been sitting still for a minute.
        for _ in 0..40 {
            trail.fade(dt);
            trail.record(Vec3::new(2.9, 0.0, 0.0), spacing, jump);
        }
        assert!(trail.is_empty(), "a standing ball kept its trail for ever");
    }

    /// A bobbing ball trails its bob, and nothing beyond it.
    ///
    /// The bob is deliberately big enough to be worth drawing now -- see
    /// [`BOB_RISE`] -- so what this holds is the other end: a ball that has been
    /// floating in one spot for ten seconds has a trail the length of a bob and
    /// not a smear the length of everywhere it has been.
    #[test]
    fn a_ball_bobbing_on_the_spot_trails_only_its_own_bob() {
        let width = GLOW_RADIUS * TRAIL_WIDTH;
        let (spacing, jump, dt) = (width * TRAIL_STEP, width * TRAIL_JUMP, 1.0 / 60.0);
        let mut trail = Trail::default();
        let bob = |elapsed: f32| BOB_RISE * (elapsed * std::f32::consts::TAU * BOB_HZ).sin();
        for step in 0..600 {
            trail.fade(dt);
            trail.record(Vec3::new(0.0, bob(step as f32 * dt), 0.0), spacing, jump);
        }
        assert!(
            trail.span() > 0.1,
            "a floating ball drew nothing: {}",
            trail.span()
        );
        assert!(
            trail.span() < BOB_RISE * 4.0,
            "ten seconds of bobbing smeared into a trail {} long",
            trail.span()
        );
    }

    /// A ball that has stopped is not given a bar of glow lying across it.
    ///
    /// The artifact this is here for: the head of the ribbon is the ball, and
    /// when the ball has come to rest on its own newest mark that rung has no
    /// direction to be turned by. Whatever was substituted -- world up, in the
    /// first version -- drew a full-width smear above and below the ball, on a
    /// ball that was not going anywhere.
    #[test]
    fn a_ball_that_has_stopped_is_not_given_a_bar_of_glow_across_it() {
        let mut trail = Trail::default();
        for (step, age) in [(0.0_f32, 0.30), (1.0, 0.15), (2.0, 0.05)] {
            trail.path.push_back(Mark {
                at: Vec3::new(step, 0.0, 0.0),
                age,
            });
        }
        // The ball is sitting exactly on its newest mark: it has stopped.
        let head = Vec3::new(2.0, 0.0, 0.0);
        let mut ribbon = Ribbon::default();
        ribbon.weave(head, &trail, Vec3::new(2.0, 100.0, 0.0), 0.5);
        assert_eq!(
            ribbon.positions.len(),
            6,
            "the head was drawn on top of the mark it is standing on"
        );
        // Seen from above, with the path running along x, every rung has to be
        // spread along z. A rung spread along y is the artifact.
        for rung in ribbon.positions.chunks(2) {
            let (left, right) = (Vec3::from(rung[0]), Vec3::from(rung[1]));
            assert!(
                (left.y - right.y).abs() < 1e-4 && (left.z - right.z).abs() > 1e-3,
                "a rung was laid across the ball rather than along its path: \
                 {left:?} {right:?}"
            );
        }
        // And nothing at the ball is at full brightness any more: what is left
        // is the path, fading where it lies.
        let brightest = ribbon
            .colours
            .iter()
            .map(|colour| colour[3])
            .fold(0.0_f32, f32::max);
        assert!(
            brightest < 1.0,
            "a stopped ball kept a lit head: {brightest}"
        );
    }

    /// Nothing is drawn from where a thing was made.
    ///
    /// The frame a mote is spawned it sits at its machine's origin, down on the
    /// lawn, and is placed properly the frame after. A history that started at
    /// the first frame would draw a streak out of the middle of the reactor
    /// every time a unit arrived.
    #[test]
    fn a_trail_does_not_start_at_the_place_a_thing_was_spawned() {
        let (spacing, jump, dt) = (0.1, 100.0, 1.0 / 60.0);
        let mut trail = Trail::default();
        // Frame one: the origin, which is not anywhere it has been.
        trail.record(Vec3::ZERO, spacing, jump);
        assert!(trail.is_empty(), "the spawn point was recorded as a place");
        // Frame two: a long way off, and still nothing joining the two.
        trail.fade(dt);
        trail.record(Vec3::new(4.0, 1.0, 0.0), spacing, jump);
        assert!(trail.span() < 1e-4, "a streak was drawn out of the origin");
    }

    /// A teleport is not travel either.
    #[test]
    fn snatching_a_ball_off_the_ground_does_not_draw_a_line_to_it() {
        let (spacing, jump, dt) = (0.1, 1.0, 1.0 / 60.0);
        let mut trail = Trail::default();
        for step in 0..20 {
            trail.fade(dt);
            trail.record(Vec3::new(step as f32 * 0.1, 0.0, 0.0), spacing, jump);
        }
        assert!(trail.len() > 5);
        // Picked up: the ball jumps to the top of a Mario's head, which is
        // further in one frame than anything travels.
        trail.record(Vec3::new(1.9, CARRY_HEIGHT, 0.0), spacing, jump);
        assert_eq!(
            trail.len(),
            1,
            "the history survived a teleport and will be drawn as a streak across it"
        );
    }

    /// A ball let go of falls to the ground instead of cutting to it.
    ///
    /// The third of the abrupt changes of place: a ball is dropped from over a
    /// dead Mario's head, or put down when the leader it was following is gone,
    /// and its resting height is on the grass a metre and a half below. Writing
    /// that height is the ball being in two places on consecutive frames.
    ///
    /// The fall is kept as an offset rather than as an easing of the transform,
    /// because the height a loose ball wants is a moving target -- see
    /// [`Orb::slack`].
    #[test]
    fn a_ball_let_go_of_falls_to_the_ground_rather_than_cutting_to_it() {
        let (from, ground) = (1.9_f32, 0.4_f32);
        let mut world = World::new();
        let mut clock: Time = Time::default();
        clock.advance_by(std::time::Duration::from_millis(16));
        world.insert_resource(clock);
        let mut orb = Orb {
            kind: Kind::Nuclonium,
            phase: 0.0,
            rest: 0.0,
            idle: 0.0,
            slack: 0.0,
        };
        orb.drop_to(from, ground);
        assert_eq!(orb.rest, ground, "it is not heading for the ground");
        assert_eq!(orb.slack, from - ground, "the fall was not remembered");
        let ball = world
            .spawn((
                orb,
                Nuclonium {
                    held: Held::Loose { claimed: None },
                },
                Transform::from_translation(Vec3::Y * from),
            ))
            .id();
        let height = |world: &World| world.get::<Transform>(ball).unwrap().translation.y;
        world.run_system_once(shimmer).unwrap();
        let after = height(&world);
        assert!(after < from, "it never started falling");
        assert!(
            after > ground + (from - ground) * 0.5,
            "a sixtieth of a second took it {} of the way down",
            (from - after) / (from - ground)
        );
        // And it lands, rather than easing for ever a centimetre up.
        for _ in 0..120 {
            world.run_system_once(shimmer).unwrap();
        }
        assert!(
            world.get::<Orb>(ball).unwrap().slack.abs() < 0.01,
            "two seconds later it is still {} m above where it settled",
            world.get::<Orb>(ball).unwrap().slack
        );
        assert!(
            (height(&world) - ground).abs() <= BOB_RISE + 0.01,
            "it settled at {} rather than bobbing about {ground}",
            height(&world)
        );
    }

    /// A glow is drawn in front of its ball rather than through the ground.
    ///
    /// **The hard edge across the bottom of a glowing ball is the depth
    /// buffer.** The glow is a card aimed at the camera, so it stands through
    /// whatever its ball is floating over, and the line where the two surfaces
    /// cross is drawn as a ruler-straight bite out of a soft round picture. A
    /// ball hanging beside a wall is the same photograph turned on its side,
    /// which is what the round-nine note is about.
    ///
    /// The card is brought forward instead -- and scaled about the eye as it
    /// goes, so it lands on exactly the pixels it was on at exactly the size.
    /// That second half is what this pins, because getting it wrong is a glow
    /// that swells as the camera walks up to it. See [`HALO_FLOAT`].
    #[test]
    fn a_glow_is_drawn_in_front_of_its_ball_at_the_size_it_would_have_been() {
        let mut world = World::new();
        let mut clock: Time = Time::default();
        clock.advance_by(std::time::Duration::from_millis(16));
        world.insert_resource(clock);
        let apart = 10.0;
        world.spawn((
            Camera3d::default(),
            GlobalTransform::from(Transform::from_xyz(0.0, 0.0, apart)),
        ));
        let ball = world
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        let halo = world
            .spawn((
                Halo { phase: 0.0 },
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(ball),
            ))
            .id();
        world.run_system_once(shimmer).unwrap();
        let card = *world.get::<Transform>(halo).unwrap();
        // However far through its breath the one advanced frame left it. See
        // [`HALO_SWELL`]: a glow that is a fraction bigger is brought a
        // fraction further forward, because what has to clear the ground is
        // the size it is actually drawn at.
        let swell = 1.0 + HALO_SWELL * (0.016 * std::f32::consts::TAU * BOB_HZ).sin();
        // Straight at the camera, by its own radius times the float.
        assert!(
            (card.translation - Vec3::Z * (GLOW_RADIUS * swell * HALO_FLOAT)).length() < 1e-3,
            "the glow was not brought forward: {:?}",
            card.translation
        );
        // And smaller in the same proportion, which is what leaves it the same
        // size on the screen: a card at `d - f` drawn at `scale` covers the same
        // pixels as one at `d` drawn at `scale * d / (d - f)`.
        let seen = |scale: f32, at: f32| scale / at;
        assert!(
            (seen(card.scale.x, apart - card.translation.z) - seen(swell, apart)).abs() < 1e-4,
            "the glow changed size as it came forward: {} at {}",
            card.scale.x,
            apart - card.translation.z
        );
    }

    /// A ball lays light on the ground under it, and less of it the higher it
    /// floats.
    ///
    /// The pool is what "emissive" means in a renderer with no lights in it at
    /// all -- see [`SPILLS`] -- and the two things worth pinning about it are
    /// that it is brightest with the ball on the deck and that it is gone by
    /// the time the ball is well above the floor. The second is what stops a
    /// ball carried over a Mario's head painting the grass it walks over.
    #[test]
    fn the_light_under_a_ball_fades_as_it_rises() {
        assert!(
            (pooled(0.0) - 1.0).abs() < 1e-6,
            "a ball on the ground is dim"
        );
        assert_eq!(pooled(SPILL_REACH), 0.0, "the pool never quite goes out");
        assert_eq!(pooled(SPILL_REACH * 4.0), 0.0, "it came back on");
        let mut last = pooled(0.0);
        for step in 1..=20 {
            let now = pooled(SPILL_REACH * step as f32 / 20.0);
            assert!(now <= last, "the pool brightened as the ball rose");
            last = now;
        }
        // And the far end of the pool of *balls* is faded rather than switched:
        // only the nearest [`SPILLS`] get one, so the ones that swap places at
        // the edge have to be at nothing when they do it.
        assert!((far_off(0.0) - 1.0).abs() < 1e-6);
        assert_eq!(far_off(SPILL_RANGE), 0.0);
    }

    /// One pool is a flat square of light lying on the ground it was poured on.
    #[test]
    fn a_pool_of_light_lies_flat_on_the_ground_it_is_poured_on() {
        let mut wash = Wash::default();
        let lying = Vec3::new(3.0, 1.0, -2.0);
        wash.pour(lying, Vec3::Y, 2.0, Kind::Nuclonium.tint(), 0.5);
        assert_eq!(wash.pools(), 1, "a pool did not reach the mesh");
        assert_eq!(
            wash.strengths(),
            vec![0.5],
            "the brightness was not carried"
        );
        for corner in &wash.positions {
            let at = Vec3::from(*corner);
            assert!((at.y - lying.y).abs() < 1e-5, "a corner left the ground");
            assert!(
                (Vec3::new(at.x, lying.y, at.z).distance(lying) - 2.0 * SQRT_2).abs() < 1e-4,
                "the pool was not two radii across: {at:?}"
            );
        }
        // Nothing to see is nothing drawn, rather than a square of black.
        let mut dark = Wash::default();
        dark.pour(lying, Vec3::Y, 2.0, Kind::Nuclonium.tint(), 0.0);
        dark.pour(lying, Vec3::Y, 0.0, Kind::Nuclonium.tint(), 1.0);
        assert_eq!(dark.pools(), 0, "an invisible pool was drawn anyway");
    }

    /// The head of a trail closes over the ball rather than stopping at it.
    ///
    /// **This is the abrupt transition at the front of a trail.** The ribbon
    /// used to end on the ball: full width, full brightness, and then a rung
    /// square across the direction of travel with nothing past it. That edge is
    /// a real discontinuity and it reads as one -- a bright bar with two corners
    /// laid over a round glowing ball.
    ///
    /// So the strip carries on past the ball and closes over it on a circular
    /// profile. What this pins is the shape: it reaches past the head, it
    /// narrows the whole way, and it ends at nothing. See [`NOSE_REACH`].
    #[test]
    fn the_head_of_a_trail_closes_over_the_ball() {
        let width = 0.5;
        let mut trail = Trail::default();
        for step in [0.0_f32, 1.0] {
            trail.path.push_back(Mark {
                at: Vec3::new(step, 0.0, 0.0),
                age: TRAIL_LIFE * 0.3,
            });
        }
        let head = Vec3::new(2.0, 0.0, 0.0);
        let mut ribbon = Ribbon::default();
        ribbon.weave(head, &trail, head + Vec3::Y * 20.0, width);
        let spine = ribbon.spine();
        // Travelling along +x, so the dome is out in front of the ball.
        let nose = spine[0];
        assert!(
            nose.x > head.x + 1e-3,
            "the ribbon stopped at the ball: {nose:?}"
        );
        assert!(
            (nose.x - head.x - width * NOSE_REACH).abs() < 1e-3,
            "the dome reached {} rather than {}",
            nose.x - head.x,
            width * NOSE_REACH
        );
        let girth = |rung: usize| {
            (Vec3::from(ribbon.positions[rung * 2]) - Vec3::from(ribbon.positions[rung * 2 + 1]))
                .length()
        };
        assert!(girth(0) < 1e-4, "the dome did not close: {}", girth(0));
        for rung in 0..NOSE_RUNGS {
            assert!(
                girth(rung) < girth(rung + 1),
                "the dome widened the wrong way at rung {rung}"
            );
        }
    }

    /// A ball that has stopped grows no dome.
    ///
    /// There is no direction to close over -- and the fallback would be a
    /// bright bar pointing wherever the last segment happened to point, which
    /// is the smear this file already drops zero-length segments to avoid. What
    /// a stopped ball leaves is its old path fading where it lies.
    #[test]
    fn a_ball_that_has_stopped_has_nothing_in_front_of_it() {
        let mut trail = Trail::default();
        for step in [0.0_f32, 1.0] {
            trail.path.push_back(Mark {
                at: Vec3::new(step, 0.0, 0.0),
                age: TRAIL_LIFE * 0.3,
            });
        }
        // Standing on its own newest mark, which is what having stopped means.
        let head = Vec3::new(1.0, 0.0, 0.0);
        let mut ribbon = Ribbon::default();
        ribbon.weave(head, &trail, head + Vec3::Y * 20.0, 0.5);
        let spine = ribbon.spine();
        assert!(
            spine.iter().all(|point| point.x <= head.x + 1e-4),
            "a stopped ball grew a dome: {spine:?}"
        );
    }

    /// What actually reaches the mesh: a strip that faces the camera, is
    /// brightest at the ball and fades to nothing at the far end.
    #[test]
    fn a_woven_trail_faces_the_camera_and_fades_behind_the_ball() {
        let mut trail = Trail::default();
        // Three marks laid along +x, oldest first, aged so the tail is nearly
        // out. The ball itself is at the far +x end.
        for (step, age) in [(0.0_f32, TRAIL_LIFE * 0.9), (1.0, TRAIL_LIFE * 0.45)] {
            trail.path.push_back(Mark {
                at: Vec3::new(step, 0.0, 0.0),
                age,
            });
        }
        let head = Vec3::new(2.0, 0.0, 0.0);
        // Looking down from straight above, so "across the ribbon" has to come
        // out along z: perpendicular to the travel and to the eye at once.
        let eye = Vec3::new(2.0, 100.0, 0.0);
        let mut ribbon = Ribbon::default();
        ribbon.weave(head, &trail, eye, 0.5);

        // The dome over the front of the ball, and then the ball and its two
        // marks. See [`NOSE_REACH`].
        let rungs = NOSE_RUNGS + 3;
        assert_eq!(ribbon.positions.len(), rungs * 2, "two corners to a rung");
        assert_eq!(
            ribbon.indices.len(),
            (rungs - 1) * 6,
            "one quad between each pair of rungs"
        );
        // The tip of the dome has no width to be spread across anything, which
        // is what closing over the ball means; every other rung is square to
        // the view.
        for rung in ribbon.positions.chunks(2).skip(1) {
            let (left, right) = (Vec3::from(rung[0]), Vec3::from(rung[1]));
            assert!(
                (left.z + right.z).abs() < 1e-4 && (left.z - right.z).abs() > 1e-3,
                "a rung was not spread across the view: {left:?} {right:?}"
            );
            assert!((left.y).abs() < 1e-4, "the ribbon left its own plane");
        }
        // Veiled where the ball's own glow already is, out at full strength
        // behind it, and gone at the tail. See [`VEIL_REACH`]: the brightest
        // part of a trail is not the end joined to the ball, it is the first
        // part of it the glow is no longer covering.
        let alpha: Vec<f32> = ribbon.colours.iter().map(|colour| colour[3]).collect();
        let ball = NOSE_RUNGS * 2;
        assert!(
            (alpha[ball] - VEIL_FLOOR).abs() < 1e-4,
            "the head was not veiled under the glow: {alpha:?}"
        );
        assert!(
            alpha[ball + 2] > alpha[ball],
            "the trail never came out from under the glow: {alpha:?}"
        );
        assert!(
            alpha[ball + 4] < 0.02,
            "the tail did not fade out: {alpha:?}"
        );
        // Narrower as it goes, too.
        let width = |rung: usize| {
            (Vec3::from(ribbon.positions[rung * 2]) - Vec3::from(ribbon.positions[rung * 2 + 1]))
                .length()
        };
        assert!(width(NOSE_RUNGS) > width(NOSE_RUNGS + 1));
        assert!(width(NOSE_RUNGS + 1) > width(NOSE_RUNGS + 2));
        assert!(width(NOSE_RUNGS + 2) > 0.0, "the tail pinched to nothing");
    }

    /// Nothing to draw draws nothing, rather than a triangle of rubbish.
    #[test]
    fn a_ball_with_no_history_has_no_trail() {
        let mut ribbon = Ribbon::default();
        ribbon.weave(Vec3::ZERO, &Trail::default(), Vec3::Z * 10.0, 0.5);
        assert!(ribbon.positions.is_empty() && ribbon.indices.is_empty());
    }

    #[test]
    fn a_shipment_walks_its_route_and_then_runs_off_the_end() {
        let legs = vec![
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 4.0),
        ];
        assert_eq!(point_along(&legs, 0.0), Some(Vec3::ZERO));
        assert_eq!(point_along(&legs, 5.0), Some(Vec3::new(5.0, 0.0, 0.0)));
        // Past the first corner and onto the second leg.
        assert_eq!(point_along(&legs, 12.0), Some(Vec3::new(10.0, 0.0, 2.0)));
        assert_eq!(point_along(&legs, 14.0), Some(Vec3::new(10.0, 0.0, 4.0)));
        assert_eq!(point_along(&legs, 14.1), None, "arrived");
        // A route of one point is not a route.
        assert_eq!(point_along(&[Vec3::ZERO], 0.0), None);
    }
}
