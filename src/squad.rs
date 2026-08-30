//! The Marios: a field of allies, whistled into a squad and sent somewhere.
//!
//! Ported from `sm64py/squad.py`. Two commands on one button, told apart by
//! how long it is held:
//!
//!   * **held** -- a circle grows on the ground where the view is pointing, up
//!     to a cap. Every ally inside it when the button comes up joins the squad
//!     and follows.
//!   * **tapped** -- the squad is sent to whatever spot the same aim resolves
//!     to. They walk there, spread out around it, and are on their own again
//!     once they arrive.
//!
//! Pikmin's shape rather than an RTS's: there is no cursor to drag a box with,
//! so the selection is aimed the same way a throw would be.
//!
//! The aiming and the formation are plain arithmetic over positions and the
//! level's collision, so both are exercised headless -- the Panda3D build
//! checks the same maths in `tools/check_squad.py`.
//!
//! Every distance here is the Panda3D build's, converted from SM64 units to
//! the port's world scale of 1/100.

use crate::{
    animation::{AllyAnimationRoot, AnimationState, CharacterAnimations},
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    level::LevelData,
    player::{Player, PLAYER_HEIGHT},
    ActiveCharacter,
};
use bevy::prelude::*;

// -- aiming -----------------------------------------------------------------

/// How near the aim may land, measured out from the player in the horizontal
/// plane. Inside it the circle is drawn around his own feet.
const AIM_MIN_RANGE: f32 = 2.5;

/// How far the *placement* gestures reach: the build preview, the pylon and
/// the nuclonium call. A machine is put down within sight of the person putting
/// it down, and past this the gesture stops meaning anything.
///
/// **An order is not one of these and does not use it.** See [`order_reach`].
pub const PLACE_REACH: f32 = 26.0;

/// Walking the target back toward the player when the ray met nothing at all,
/// and how far out to start walking back from.
///
/// Aiming at the sky is the case: there is no spot up there to put anything on,
/// so the aim is brought down to somewhere in front of him and walked in until
/// there is floor under it. Bounded rather than taken from the reach, so an
/// order -- whose reach is the whole level -- does not pay two hundred floor
/// queries a frame for a view tilted at the horizon.
const AIM_BACKOFF: f32 = 2.0;
const AIM_SKY_RANGE: f32 = 26.0;

/// Longer than a tap, in seconds. Under this the press is an order to the
/// squad it already has; over it, a whistle for a new one.
pub const TAP_SECONDS: f32 = 0.18;

/// The whistle circle: where it starts, where it stops, and how long it takes
/// to grow between the two.
const CIRCLE_MIN_RADIUS: f32 = 2.2;
const CIRCLE_MAX_RADIUS: f32 = 11.0;
const CIRCLE_GROW_SECONDS: f32 = 1.1;

/// The mark left where an order landed: how long it stays up, how long it
/// takes to pop open, and how much wider than the cluster it is drawn.
///
/// It fades rather than blinking out because the ring is an answer to "where
/// did that go", and an answer that vanishes on a frame boundary reads as a
/// glitch rather than as a mark expiring.
const ORDER_RING_SECONDS: f32 = 1.6;
const ORDER_RING_POP_SECONDS: f32 = 0.16;
const ORDER_RING_MARGIN: f32 = 1.4;

/// How solid a ring is drawn at full strength. Shared by both of them, so the
/// order's mark at its freshest is exactly as present as the whistle's circle.
const RING_ALPHA: f32 = 0.85;

/// How far above his feet an ally may be and still be whistled. A circle is a
/// flat thing drawn on the ground and reads as one, so the height is generous
/// but not unbounded: somebody on the castle roof is not in a circle drawn on
/// the lawn beneath him.
const RECRUIT_HEIGHT: f32 = 8.0;

// -- the formation ----------------------------------------------------------

/// Where the group gathers relative to the leader, and how far apart its
/// members stand once they are there.
const FOLLOW_DISTANCE: f32 = 3.3;
const FOLLOW_SPACING: f32 = 1.7;
const FOLLOW_ARRIVE: f32 = 1.1;

/// The same, for a spot they have been sent to. Wider, because nothing is
/// moving the target around and a tight cluster there just means they shove
/// each other.
const SEND_SPACING: f32 = 2.0;
pub(crate) const SEND_ARRIVE: f32 = 1.4;

/// The angle between one slot in a cluster and the next. The golden angle is
/// what keeps a spiral from lining its points up into spokes, which is the
/// same reason a sunflower uses it: any simpler step leaves the allies in rows
/// with gaps between them.
pub(crate) const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// How near an ally has to be before it counts as standing on its slot.
const ALLY_RADIUS: f32 = 0.4;

/// How close a Mario walks to what it is going to hit, measured from that
/// thing's body rather than from its middle. Inside its own punch's reach, so
/// that arriving and connecting are not two separate strides.
///
/// **Wider than `enemy::PERSONAL_SPACE`, and that is the constraint rather than
/// a preference.** `enemy::spread` will not let a Mario stand nearer a body than
/// the two radii plus that gap, so a strike range shorter than it is an order to
/// stand somewhere the shove immediately undoes -- and the Mario spends the
/// fight being walked in and pushed out, inside the thing it is punching. As an
/// absolute 1.2 m it was a metre and a half inside an ant, which is exactly what
/// that looked like.
pub(crate) const STRIKE_RANGE: f32 = 0.5;

/// The amble an ally falls back on with nobody to follow: how far from where
/// it was left it will wander, how near it has to get before that counts as
/// arriving, and how long it stands about afterwards before ambling somewhere
/// else.
///
/// It is a walk to a fixed spot followed by a rest, rather than a point the
/// ally continuously chases. That distinction is the whole of it: the first
/// version orbited a target around the ally at a speed it could out-walk in
/// one step, so it alternated between walking and standing every few ticks.
/// Each of those changes restarts the clip, and a walk cycle restarted three
/// times a second never gets past its first frames -- which is a field of
/// Marios stuck mid-stride, going nowhere.
const WANDER_RADIUS: f32 = 6.0;
const WANDER_ARRIVE: f32 = 0.5;
const WANDER_REST: f32 = 2.0;
const WANDER_REST_SPREAD: f32 = 3.0;

/// How fast it ambles, which is not how fast it follows: an ally under orders
/// has to keep up with the player, and one with nowhere to be does not. This
/// is the speed Mario's walk clip was authored to cover ground at, so an
/// ambling Mario's feet are planted -- the same reason the amble is long
/// enough to be a walk at all rather than a step and a stop.
const AMBLE_SPEED: f32 = crate::animation::MARIO_STRIDE_SPEED;

/// An ally in the field. The only thing the squad writes onto one is its goal;
/// an ally with none is nobody's business and goes back to ambling.
///
/// Every Mario has a [`crate::path::Route`] whether or not it is walking one,
/// because the alternative is worse than it looks: the systems that decide and
/// walk both ask for it, so a Mario spawned without one is not a Mario that
/// walks straight -- it is a Mario that drops out of those queries entirely and
/// stands still for ever, with nothing anywhere reporting a problem. Required
/// rather than remembered, so that cannot be got wrong.
#[derive(Component)]
#[require(crate::path::Route)]
pub struct Ally {
    /// Where it should be standing, and how near counts as there.
    pub goal: Option<(Vec2, f32)>,
    /// What it decided to do with itself this tick.
    ///
    /// Written by [`crate::goap::plan`] and read by nothing but [`move_allies`],
    /// which walks towards whatever it says and asks no questions about why.
    /// [`Self::goal`] is still the *record* of what the player ordered; this is
    /// the decision about whether that is the thing to be doing right now, which
    /// is a different question and now has its own module. See [`crate::goap`]
    /// for why a priority chain could not answer it.
    pub plan: crate::goap::Goal,
    /// Where it ambles around when it has no goal: the spot it was left, the
    /// spot it is currently walking to, how long it still has to stand about
    /// first, and its phase, which is what keeps a crowd from moving as one
    /// body.
    home: Vec3,
    stroll: Vec2,
    rest_left: f32,
    phase: f32,
    pub velocity: Vec3,
    pub state: AnimationState,
    /// How long is left of the punch it is throwing, or zero.
    ///
    /// Thrown and resolved by [`crate::enemy::ally_combat`]; kept here because
    /// a Mario in the middle of one stands still to throw it, which is this
    /// module's business.
    pub swing_left: f32,
    /// How long it still cannot be hurt for, which is what stops a Mario
    /// standing in a crowd losing all twenty points in a third of a second.
    ///
    /// The player's equivalent lives on [`crate::player::Controller`] and does
    /// the same job for the same reason. Written down by
    /// [`crate::enemy::maul`] and counted off by it.
    pub hurt_left: f32,
    /// Whether it was out of its depth last tick.
    ///
    /// Kept rather than recomputed where it is wanted, because what it is for is
    /// noticing the *edge*: entering and leaving the water is a splash, and a
    /// depth read fresh each tick can say "wet" a hundred times running without
    /// anything having happened. [`crate::player::Controller::submersion`] is
    /// the same field doing the same job for the player.
    pub swimming: bool,
}

impl Ally {
    /// A new Mario standing where it was put, about to amble.
    pub fn new(home: Vec3, phase: f32) -> Self {
        let mut ally = Self {
            goal: None,
            plan: crate::goap::Goal::Idle,
            home,
            stroll: Vec2::new(home.x, home.z),
            rest_left: 0.0,
            phase,
            velocity: Vec3::ZERO,
            state: AnimationState::default(),
            swing_left: 0.0,
            hurt_left: 0.0,
            swimming: false,
        };
        ally.amble_somewhere_else();
        ally
    }

    /// Picks the next spot to amble to, and how long to stand about first.
    ///
    /// The golden angle again, advanced per ally: successive destinations do
    /// not line up into a path that retraces itself, and two allies never pick
    /// the same one at the same moment -- with no random number generator
    /// anywhere, so a whole crowd stays reproducible in a test.
    fn amble_somewhere_else(&mut self) {
        self.phase += GOLDEN_ANGLE;
        let spread = |scale: f32| (self.phase * scale).sin().abs();
        let reach = WANDER_RADIUS * (0.5 + 0.5 * spread(0.37));
        self.stroll = Vec2::new(self.home.x, self.home.z)
            + Vec2::new(self.phase.sin(), self.phase.cos()) * reach;
        self.rest_left = WANDER_REST + WANDER_REST_SPREAD * spread(0.21);
    }

    /// Standing still, wherever that is.
    fn stand(&mut self, dt: f32) {
        self.velocity = Vec3::ZERO;
        self.state.motion = crate::player::Motion::Idle;
        self.state.speed = 0.0;
        self.state.still_for += dt;
    }
}

/// One Mario, one spot it was sent to, and how that journey is going.
///
/// The last two fields are only ever read by [`update_goals`] and exist to
/// answer one question: **is this Mario still on its way, or is it simply
/// stuck?** Without asking it, an order is discharged by arrival and by nothing
/// else, so a Mario whose slot in the cluster landed on the far side of a fence
/// -- or in the moat, or under a wall -- walks into that fence for the rest of
/// the session at full [`crate::goap::OBEY_APPEAL`], deaf to every slime beside
/// it. That is one of the ways a squad "just stops and does nothing", and it is
/// not a scoring bug: the Mario really has not arrived.
///
/// So the order is also discharged by giving up. [`Self::closest`] is the best
/// the Mario has ever managed, and a stretch of [`STUCK_SECONDS`] without
/// beating it by [`PROGRESS`] means the walk is not going anywhere. The order
/// then counts as fulfilled -- see [`crate::goap::Goal::Hold`] for what that is
/// worth -- which is the difference between a Mario pressed against a fence
/// doing nothing and a Mario pressed against a fence hitting what is on its own
/// side of it.
#[derive(Clone, Copy, Debug)]
pub struct Sent {
    pub who: Entity,
    /// The spot in the cluster it was given.
    pub at: Vec2,
    /// Whether it counts as having got there -- by arriving, or by having
    /// spent long enough failing to.
    pub arrived: bool,
    /// What progress is currently being measured against: the corner of the
    /// route it is walking at, or the spot itself when it is walking straight
    /// there.
    ///
    /// **Against the corner and not against the spot, which is the difference
    /// between a stall clock and a clock that runs out on every long way
    /// round.** A route deliberately goes the wrong way for a while: out of the
    /// courtyard before it can start crossing the lawn. Measured against the
    /// spot, that Mario is losing ground for ten seconds and the order is
    /// retired half way through being carried out -- precisely the behaviour
    /// the routing exists to prevent, produced by the routing. Measured against
    /// the corner it is walking at, it is doing fine.
    towards: Vec2,
    /// The nearest it has got to [`Self::towards`] since that became the thing
    /// it was walking at.
    closest: f32,
    /// How long since it last beat that.
    stuck_for: f32,
    /// How long the pathfinder has been telling this Mario there is no way
    /// there at all. See [`LOST_SECONDS`].
    lost_for: f32,
}

/// Which allies are following and which have been sent somewhere.
///
/// Entities rather than indices, so a Mario that is despawned mid-order drops
/// out cleanly instead of shuffling the formation under everyone else.
#[derive(Resource, Default)]
pub struct Squad {
    pub members: Vec<Entity>,
    /// Sent to a spot, and how they are getting on. They keep the spot once
    /// they are standing on it: an ally sent somewhere holds it until whistled
    /// up again, which is what makes sending them an order rather than a
    /// suggestion.
    pub sent: Vec<Sent>,
    /// Where the followers are gathering, kept between ticks so it can trail the
    /// leader rather than be recomputed from where he happens to be facing.
    ///
    /// See [`update_goals`] for what goes wrong without it.
    anchor: Option<Vec2>,
}

/// The live whistle: how long the button has been down and how big the circle
/// has grown, or `None` while nothing is held.
#[derive(Resource, Default)]
pub struct Whistle {
    pub held_for: Option<f32>,
    pub aim: Vec3,
    pub radius: f32,
}

impl Whistle {
    /// The circle is only drawn once the press has outlasted a tap.
    pub fn showing(&self) -> bool {
        self.held_for.is_some_and(|held| held >= TAP_SECONDS)
    }
}

/// The ring left on the ground where the last order landed.
///
/// **Separate from the whistle's own circle, and it has to be.** That one is
/// live while the button is down and gone the instant it comes up -- which is
/// the exact moment the player most wants to know where the order went. So the
/// order leaves its own mark, sized to the cluster the squad is spreading into
/// and fading out over [`ORDER_RING_SECONDS`].
///
/// Written by [`whistle`] on the release and read by [`order_ring`], both at
/// the render rate: the mark is presentation and nothing in the simulation
/// looks at it.
#[derive(Resource, Default)]
pub struct OrderMark {
    /// Where the order landed -- the aim point itself, not a slot.
    pub at: Vec3,
    /// How wide to draw it, in world units.
    pub radius: f32,
    /// Seconds since it was left, or `None` when no order has been given yet.
    pub age: Option<f32>,
}

impl OrderMark {
    /// Leaves a fresh mark on the spot an order came to rest.
    ///
    /// Off the [`Landed`] the send answered rather than off the aim that was
    /// pointed at, which is the difference between a ring that reports and a
    /// ring that decorates: the Marios go to the cluster, so the cluster is
    /// what gets ringed.
    pub fn left_at(&mut self, landed: Landed) {
        self.at = landed.at;
        self.radius = landed.radius;
        self.age = Some(0.0);
    }

    /// How solid the mark is drawn now: 1 when fresh, 0 once it is spent.
    pub fn fade(&self) -> f32 {
        match self.age {
            Some(age) if age < ORDER_RING_SECONDS => 1.0 - age / ORDER_RING_SECONDS,
            _ => 0.0,
        }
    }

    /// How wide it is drawn now, which is not its final width for the first
    /// few frames: it pops open from a little under so the mark reads as
    /// something that just happened.
    pub fn drawn_radius(&self) -> f32 {
        let open = (self.age.unwrap_or(0.0) / ORDER_RING_POP_SECONDS).clamp(0.0, 1.0);
        self.radius * (0.55 + 0.45 * open)
    }
}

/// The order itself, put on the ground.
///
/// A crosshair now lands exactly where it is pointed -- part way up a wall, out
/// over the moat -- and an order is a place to walk to, so it is worth putting
/// on the ground before a cluster is laid out around it. See [`aim_point`].
///
/// **Kept where it was pointed whenever there is ground under it.** Snapping
/// every order to the middle of a survey cell would move it up to half a cell
/// from the thing the player was actually pointing at, every single time, to
/// fix the case where it was pointed at nothing. So the snap is only for that
/// case; otherwise the aim keeps its own x and z and takes only its height off
/// the survey, which is what puts an order aimed at a wall at the foot of the
/// wall rather than on its face.
fn landing(field: &crate::flow::FlowField, aim: Vec3) -> Vec3 {
    let cell = field.cell_at(aim);
    if !field.survey_of(cell).walkable {
        return field.standable(aim).unwrap_or(aim);
    }
    // The ray's own height is the exact ground under the crosshair; the
    // survey's is an average over a couple of metres of cell. So the ray's is
    // kept whenever the two agree, and the survey's is taken only when they do
    // not -- which is the crosshair having stopped part way up something, a
    // body's height or more above the ground at its foot.
    let ground = field.centre_of(cell).y;
    let y = match (aim.y - ground).abs() > PLAYER_HEIGHT {
        true => ground,
        false => aim.y,
    };
    Vec3::new(aim.x, y, aim.z)
}

/// Where the `count` members of a squad sent to `centre` should stand.
///
/// The golden-angle spiral [`slot`] draws, with the slots that will not hold a
/// Mario thrown away and the spiral drawn further to make up the number. A
/// cluster on a narrow bank therefore stretches along the bank instead of
/// spilling into the water, and one landed against a wall packs into the ground
/// in front of it -- the shape gives before the ground does, which is the right
/// way round for a formation nobody authored.
///
/// **A slot holds a Mario when it can be walked to from the middle of the
/// cluster in a straight line, and nothing weaker will do.** The obvious test
/// -- ask the survey whether the cell is walkable -- is the one that let the
/// targets out over the moat through, and it is worth being exact about why,
/// because from outside it looks like the check simply was not running.
/// `walkable` is built by dropping a query from the sky and asking what it
/// hits, so it means *there is ground somewhere below this*, and over the moat
/// there certainly is: the bed of it, eight metres down and under water. Every
/// cell of open water on the castle is "walkable". So is the air over every
/// ledge on the map, all the way down to whatever is at the bottom. Filtering
/// on it moved the spots not one inch.
///
/// [`crate::flow::FlowField::clear`] asks the question that was actually meant.
/// It walks the segment cell by cell and refuses any edge that climbs or drops
/// more than the field's step limit, or that has a fence across it, or that
/// leaves the ground altogether -- which is to say it refuses exactly the lip
/// of the moat, the top of the parapet and the far side of the railings, in the
/// same terms the pathfinder itself uses. A cluster whose every spot is `clear`
/// of its middle is a cluster standing on one piece of ground.
///
/// Falls back to the bare arithmetic slot once [`CLUSTER_TRIES`] slots per
/// member have been refused. That is a spot the size of a doorstep with a
/// squad of twenty pointed at it, and an imperfect order is a better answer
/// than no order.
fn cluster(field: &crate::flow::FlowField, centre: Vec3, count: usize) -> Vec<Vec2> {
    let flat = Vec2::new(centre.x, centre.z);
    // `clear` has no opinion about two points in the same cell, so the cell
    // itself is asked about separately -- otherwise a squad of one sent over a
    // cliff would be placed on the cliff.
    let holds = |spot: Vec2| {
        let there = Vec3::new(spot.x, centre.y, spot.y);
        field.survey_of(field.cell_at(there)).walkable && field.clear(centre, there)
    };
    let mut spots = Vec::with_capacity(count);
    let mut index = 0;
    while spots.len() < count && index < count * CLUSTER_TRIES {
        let spot = flat + slot(index, SEND_SPACING);
        if holds(spot) {
            spots.push(spot);
        }
        index += 1;
    }
    // Whatever the ground would not take, put where it was asked for.
    while spots.len() < count {
        spots.push(flat + slot(spots.len(), SEND_SPACING));
    }
    spots
}

/// The sweep's answer to "can the squad get there from here", when it has one.
///
/// **A wrapper around one array read, and it exists to hold a caveat.** The
/// flow field floods outward from the player every rebuild, so a cell it never
/// reached is a cell with no way to it -- exactly the question an order needs
/// answered, and answered without a budget, which is more than any per-body
/// search can promise. But the field is only that witness *after* it has swept.
/// A freshly built one has every cell at [`crate::route::UNREACHED`], and a
/// system that read it straight would retire every order the squad was ever
/// given on the grounds that nowhere at all is reachable.
///
/// So the sweep has to vouch for itself first, and the ground the player is
/// standing on is how: he is the source it floods from, so a sweep that has run
/// has reached his cell and one that has not has reached nothing. It errs the
/// safe way in the one case where that is not exact -- a player in the air, in
/// the water or on a ledge stands on a cell the survey calls unwalkable, which
/// the flood seeds *around* rather than in, so for those ticks the sweep
/// declines to say anything and no order is retired on its word.
struct Swept<'a> {
    field: &'a crate::flow::FlowField,
    /// Whether the sweep has run at all. Nothing is refused when it has not.
    ran: bool,
    /// The height to look the ground up at, which is the player's -- an order
    /// is a spot in the plane and the grid is indexed in the plane.
    y: f32,
}

impl Swept<'_> {
    fn of(field: &crate::flow::FlowField, player: Vec3) -> Swept<'_> {
        Swept {
            ran: field.survey_of(field.cell_at(player)).steps.is_some(),
            field,
            y: player.y,
        }
    }

    /// Whether there is a way to `at`. `true` whenever the sweep cannot say.
    fn reaches(&self, at: Vec2) -> bool {
        if !self.ran {
            return true;
        }
        let cell = self.field.cell_at(Vec3::new(at.x, self.y, at.y));
        self.field.survey_of(cell).steps.is_some()
    }
}

/// How many slots off the spiral may be refused per member before a cluster
/// gives up and lays the rest down where the arithmetic put them.
///
/// Six is generous: a spiral at [`SEND_SPACING`] reaches about two metres times
/// the square root of the index, so six tries a head is a squad of eight
/// looking as far as sixteen metres out for eight standable spots. Past that
/// the order was given somewhere there is genuinely nowhere to stand, and
/// reaching further would only scatter the group across the map looking for
/// ground.
const CLUSTER_TRIES: usize = 6;

/// What an order came to.
///
/// Where it landed rather than where it was pointed, because the two are not
/// the same once the spot has been put on the ground -- and the ring the
/// player is shown has to be drawn on the first of those. See [`OrderMark`].
#[derive(Clone, Copy, Debug)]
pub struct Landed {
    /// The middle of the cluster, on ground something can stand on.
    pub at: Vec3,
    /// How wide the cluster actually came out, plus a margin -- so a squad that
    /// had to stretch along a bank to find its footing is ringed by the ground
    /// it took rather than by the ground a clear lawn would have given it.
    pub radius: f32,
}

/// Offset of the index'th member of a loose cluster, in the plane.
///
/// Not rotated to face anything: the cluster is placed by its caller, and the
/// leader turning on the spot should not send everyone shuffling around him to
/// keep a formation they were never in.
pub fn slot(index: usize, spacing: f32) -> Vec2 {
    let radius = spacing * (index as f32).sqrt();
    let angle = index as f32 * GOLDEN_ANGLE;
    Vec2::new(radius * angle.sin(), radius * angle.cos())
}

/// How wide the circle has grown after being held this long.
pub fn circle_radius(held_for: f32) -> f32 {
    let grown = ((held_for - TAP_SECONDS) / CIRCLE_GROW_SECONDS).clamp(0.0, 1.0);
    CIRCLE_MIN_RADIUS + (CIRCLE_MAX_RADIUS - CIRCLE_MIN_RADIUS) * grown
}

/// How far an order reaches: as far as the level goes.
///
/// **There is no range on an order and there should not be.** A whistle is the
/// player pointing at a place and saying "go there", and the only honest answer
/// to a place he can see across the courtyard is for the squad to walk across
/// the courtyard. The old twenty-six metre cap did not refuse a long order, it
/// silently *moved* it: the target was pulled back down the bearing to the cap
/// and the squad marched off to a spot a third of the way to the thing he was
/// pointing at, which reads as the order having been misunderstood.
///
/// The level's own diagonal, so the reach is "everywhere" without the raycast
/// being handed an infinity it cannot make a segment out of.
fn order_reach(level: &LevelData) -> f32 {
    let (low, high) = level.bounds();
    (high - low).length().max(AIM_SKY_RANGE)
}

/// Where on the ground the crosshair is pointing, within `reach` of the eye.
///
/// The crosshair is the middle of the screen and the aim is the ray out of it,
/// **cast against the level's collision triangles by the same
/// [`LevelData::surface_hit`] a shot is traced with**. Left and right is where
/// the view points; up and down is range, since a view tilted down meets the
/// ground nearer and one tilted up throws the meeting further out. That is the
/// whole of the aim, and it is why the reticle never has to leave the middle
/// of the screen.
///
/// **A raycast rather than the ray march this used to be.** The march sampled
/// the ray every metre and a half asking only "is this point under the floor",
/// then handed back a spot on the bearing *from the player* to the crossing
/// rather than the crossing itself. Two errors compounded: the sampling missed
/// walls and ledges entirely -- there is no floor above a parapet, so a
/// crosshair on one resolved to the lawn behind it -- and the reprojection
/// slid the answer sideways whenever the camera sat off the player's shoulder,
/// which is always. An order given at a wall landed somewhere else, and there
/// was nothing drawn to show where. Now the hit is the answer, unmoved, and
/// [`OrderMark`] rings it.
///
/// **`reach` is the only limit, and it is the caller's to set** -- there is no
/// second clamp behind it. What the ray reaches, the aim may land on: a hit at
/// eighty metres is the answer if eighty metres is what was asked for. See
/// [`order_reach`], which is the whole level, and [`PLACE_REACH`], which is not.
///
/// Two cases are still not the ray's own answer, and both are about there being
/// no answer to give. A ray that meets nothing -- out over the moat, off the
/// edge of the world, or simply pointed at the sky -- is brought down to
/// [`AIM_SKY_RANGE`] and walked back toward the player until there is floor
/// under it. One that lands inside [`AIM_MIN_RANGE`] is pushed out to it, so
/// the whistle circle is not drawn around his own boots.
pub fn aim_point(
    level: &LevelData,
    origin: Vec3,
    direction: Vec3,
    player: Vec3,
    reach: f32,
) -> Vec3 {
    let direction = direction.normalize_or(Vec3::NEG_Z);
    let flat = Vec2::new(direction.x, direction.z).length();
    if flat < 1e-4 {
        // Straight down. Nothing to aim along; put it at his feet.
        return player;
    }
    let heading = Vec2::new(direction.x, direction.z) / flat;
    // The cast starts abreast of the player rather than at the camera: the
    // ground between the eye and his back is behind him, and a hit there
    // points the order the wrong way.
    let start = (player - origin).dot(direction).max(1.0);
    let from = origin + direction * start;
    let Some((hit, _)) = level.surface_hit(from, from + direction * reach) else {
        return backed_off(level, player, heading, reach.min(AIM_SKY_RANGE));
    };
    if Vec2::new(hit.x - player.x, hit.z - player.z).length() < AIM_MIN_RANGE {
        return backed_off(level, player, heading, AIM_MIN_RANGE);
    }
    // Exactly where the crosshair is, which is the point of casting at all.
    hit
}

/// The furthest spot at or inside `range` on `heading` with floor under it,
/// measured out from the player in the plane.
///
/// The fallback for every aim the cast could not answer outright: pointed at
/// nothing, or landed so near it is inside the player. It steps in by
/// [`AIM_BACKOFF`] until the floor query bites, and gives up onto
/// `AIM_MIN_RANGE` -- his own feet, near enough -- rather than onto nothing.
fn backed_off(level: &LevelData, player: Vec3, heading: Vec2, range: f32) -> Vec3 {
    let mut range = range;
    while range > AIM_MIN_RANGE {
        let candidate = Vec3::new(
            player.x + heading.x * range,
            player.y + PLAYER_HEIGHT,
            player.z + heading.y * range,
        );
        if let Some(height) = level.floor_height(candidate) {
            return Vec3::new(candidate.x, height, candidate.z);
        }
        range -= AIM_BACKOFF;
    }
    let x = player.x + heading.x * AIM_MIN_RANGE;
    let z = player.z + heading.y * AIM_MIN_RANGE;
    let y = level
        .floor_height(Vec3::new(x, player.y + PLAYER_HEIGHT, z))
        .unwrap_or(player.y);
    Vec3::new(x, y, z)
}

impl Squad {
    /// Whistles up everyone inside the circle, returning how many joined.
    ///
    /// One already on the way somewhere is called back rather than ignored:
    /// the whistle is how an order is taken back, and an ally who kept walking
    /// to the last spot because he was already walking would read as deaf.
    pub fn recruit(&mut self, inside: &[Entity]) -> usize {
        let mut joined = 0;
        for ally in inside {
            if self.members.contains(ally) {
                continue;
            }
            self.sent.retain(|order| order.who != *ally);
            self.members.push(*ally);
            joined += 1;
        }
        joined
    }

    /// Sends the whole squad to a spot, spread around it over ground it can
    /// actually stand on.
    ///
    /// **The spread is filtered by the survey, and that is the whole of it.**
    /// The cluster used to be pure arithmetic -- the target plus a golden-angle
    /// offset -- laid down without ever asking whether there was anything under
    /// it. Order a squad of eight onto the lip of the moat and the lip holds
    /// four of them; the other four were sent out over the water, which no
    /// Mario can reach, so each walked to the edge, leaned on it for
    /// [`STUCK_SECONDS`], and was written off. What the player saw was half a
    /// squad obeying and half of it milling about on a bank.
    ///
    /// Now [`cluster`] draws slots from the same spiral and keeps only the ones
    /// the field will vouch for, so a cluster on a narrow bank reaches further
    /// along it rather than spilling off it.
    ///
    /// Answers a [`Landed`]: where the order really came to rest and how wide
    /// the cluster ended up, so the ring drawn for the player is drawn on the
    /// spot the Marios were actually sent to rather than on the spot he
    /// pointed at.
    pub fn send(&mut self, field: &crate::flow::FlowField, aim: Vec3) -> Landed {
        let at = landing(field, aim);
        // **Everyone under command, not only those still following.** The
        // whistle is how a Mario joins the squad and the whistle is how it
        // leaves; an order is neither. Draining only `members` meant the first
        // tap emptied the squad into `sent` and every tap after it ordered
        // nobody, so redirecting a squad already on the march -- the most
        // ordinary thing a player does with one -- meant whistling the whole
        // group up again first. See [`Self::recruit`], which is still the only
        // way in, and [`Self::disband`], which is still the way out.
        let squad: Vec<Entity> = self
            .members
            .drain(..)
            .chain(self.sent.drain(..).map(|order| order.who))
            .collect();
        let spots = cluster(field, at, squad.len());
        let mut widest = 0.0_f32;
        for (ally, spot) in squad.into_iter().zip(spots) {
            widest = widest.max(Vec2::new(at.x, at.z).distance(spot));
            self.sent.push(Sent {
                who: ally,
                at: spot,
                arrived: false,
                towards: spot,
                closest: f32::INFINITY,
                stuck_for: 0.0,
                lost_for: 0.0,
            });
        }
        Landed {
            at,
            radius: widest + ORDER_RING_MARGIN,
        }
    }

    pub fn disband(&mut self) -> usize {
        let count = self.members.len() + self.sent.len();
        self.members.clear();
        self.sent.clear();
        count
    }

    /// Where the `index`'th follower should be standing, and how near counts.
    ///
    /// Handed out rather than read off `Ally::goal` so that [`crate::goap`] can
    /// score an order without the walk step having already written it down --
    /// the record of what was asked for and the decision about it are separate
    /// things now. `None` before the leader has been seen at all, which is the
    /// first tick of a session.
    pub fn follow_slot(&self, index: usize) -> Option<(Vec2, f32)> {
        self.anchor
            .map(|anchor| (anchor + slot(index, FOLLOW_SPACING), FOLLOW_ARRIVE))
    }

    /// Sent somewhere and not there yet.
    pub fn marching(&self) -> usize {
        self.sent.iter().filter(|order| !order.arrived).count()
    }
}

/// Is an ally inside a whistle circle drawn at `centre`?
pub fn in_circle(ally: Vec3, centre: Vec3, radius: f32) -> bool {
    let flat = Vec2::new(ally.x - centre.x, ally.z - centre.z).length();
    flat <= radius + ALLY_RADIUS && (ally.y - centre.y).abs() <= RECRUIT_HEIGHT
}

/// Puts one ally in the field, as whichever character was asked for.
///
/// **Either playable character can be an ally**, which is the whole of what
/// "Luna is AI-playable too" means here: the squad is not a crowd of Marios
/// with a Luna hard-wired into the player's hands, it is a field of characters
/// of which one happens to be driven by a controller. An AI Luna is the same
/// model at the same scale, animating off the same clip table and fighting
/// with the same rules, as the Luna the player is driving -- see
/// [`crate::ActiveCharacter::model`], which is where both of them get their
/// scene from.
///
/// Shared by the console's population counts and by the Mario warp pipe, so no
/// two callers can produce subtly different allies -- the same reason
/// `enemy::spawn` is shared between the level's placements and the enemy pipes.
pub fn spawn_ally(
    commands: &mut Commands,
    assets: &AssetServer,
    character: ActiveCharacter,
    home: Vec3,
    phase: f32,
) -> Entity {
    let (model, scale) = character.model();
    commands
        .spawn((
            Ally::new(home, phase),
            // Drawn between two ticks rather than at them, the way the player
            // has always been. See [`Glide`].
            Glide::default(),
            // An ally is on the player's side, and goes for what it notices on
            // the other one exactly as an enemy goes for him.
            crate::enemy::Side::Friendly,
            // And can be worn down like one. A Mario's twenty points is seven
            // ant touches -- long enough that one sent at something wins or
            // loses on whether the rest of the squad went with it -- and a Luna
            // carries the player's hundred, which is what makes filling the
            // field with one or the other a decision.
            crate::health::Health::new(character.ally_health()),
            crate::enemy::Aggro::default(),
            // Allies animate off the same tables the playable characters do,
            // and this is which table.
            character,
            // And stand on the ground the same way, so they get the same disc
            // under them as the player.
            crate::shadow::ShadowCaster::new(
                crate::player::PLAYER_RADIUS,
                crate::player::PLAYER_HEIGHT,
            ),
            WorldAssetRoot(assets.load(model)),
            Transform::from_translation(home).with_scale(Vec3::splat(scale)),
        ))
        .id()
}

/// The allies the console's population counts answer for: the field's standing
/// crowd, with the warp pipe's own brood left out of it.
///
/// The character comes with them, because there are two counts now -- one per
/// playable character -- and reconciling either one means knowing which of the
/// standing allies are that one.
type StandingCrowd<'w, 's> =
    Query<'w, 's, (Entity, &'static ActiveCharacter), (With<Ally>, Without<crate::pipe::Brood>)>;

/// Keeps the field's Mario population at whatever the console asks for.
///
/// Spawning is reconciled against a count rather than driven by a command, so
/// the console's existing `<name> <value>` grammar is all it takes to fill the
/// lawn with Marios or clear it -- and the count is a live number rather than
/// a one-shot that cannot be undone.
///
/// The Mario pipe's brood is not in the count. A pipe is responsible for
/// exactly what it produced and for replacing it when it dies, and a count that
/// swept those up would either despawn a Mario the instant it came out of the
/// pipe or stop the lawn filling at all. It is the same rule from the other
/// side: the enemy pipes leave the hand-placed enemies to the level.
pub fn maintain_population(
    mut commands: Commands,
    assets: Res<AssetServer>,
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    player: Query<&Transform, With<Player>>,
    allies: StandingCrowd,
    mut squad: ResMut<Squad>,
) {
    // One reconciliation per character, against that character's own count.
    // Two independent numbers rather than a total and a ratio: `ally_count 8`
    // has always meant eight Marios and still does, and asking for four Lunas
    // beside them should not take any of the Marios away.
    let live: Vec<(Entity, ActiveCharacter)> = allies
        .iter()
        .map(|(entity, character)| (entity, *character))
        .collect();
    // Where in the cluster the next arrival stands. Counted across both
    // characters, so a Luna and a Mario are never put down in the same slot.
    let mut placed = live.len();
    for character in ActiveCharacter::ALL {
        let wanted = match character {
            ActiveCharacter::Luna => tuning.luna_count,
            ActiveCharacter::Mario => tuning.ally_count,
        }
        .round() as usize;
        let standing: Vec<Entity> = live
            .iter()
            .filter(|(_, kind)| *kind == character)
            .map(|(entity, _)| *entity)
            .collect();
        if standing.len() > wanted {
            for entity in standing.iter().skip(wanted) {
                squad.members.retain(|member| member != entity);
                squad.sent.retain(|order| order.who != *entity);
                commands.entity(*entity).despawn();
                placed -= 1;
            }
            continue;
        }
        let Ok(leader) = player.single() else {
            return;
        };
        // New arrivals stand around the leader in the same cluster the squad
        // uses to follow him, so a crowd summoned from the console is not a
        // pile.
        for _ in standing.len()..wanted {
            let offset = slot(placed, FOLLOW_SPACING * 1.5);
            let x = leader.translation.x + offset.x;
            let z = leader.translation.z + offset.y;
            let y = level
                .floor_height(Vec3::new(x, leader.translation.y + PLAYER_HEIGHT, z))
                .unwrap_or(leader.translation.y);
            let home = Vec3::new(x, y, z);
            spawn_ally(
                &mut commands,
                &assets,
                character,
                home,
                placed as f32 * GOLDEN_ANGLE,
            );
            placed += 1;
        }
    }
}

/// Refreshes every goal, once a tick, before the allies move.
pub fn update_goals(
    mut squad: ResMut<Squad>,
    // Which spots there is a way to. See [`Swept`].
    field: Res<crate::flow::FlowField>,
    player: Query<&Transform, With<Player>>,
    mut allies: Query<(&mut Ally, &Transform, &crate::path::Route)>,
) {
    let Ok(leader) = player.single() else {
        return;
    };
    // Drop anyone who is no longer in the field. Their goal goes with them.
    squad.members.retain(|ally| allies.contains(*ally));
    squad.sent.retain(|order| allies.contains(order.who));

    // Behind the leader, so walking forward drags the group along rather than
    // through him -- but *behind* meaning the side they are already on, not the
    // side his shoulders happen to be pointing away from.
    //
    // Taking it from his facing is what made the formation jitter, and it is
    // worth being precise about why, because the fix looks like a nicety and is
    // not. The anchor sat on a three-metre arm off his back. Turning on the spot
    // -- which a mouse does several times a second and which moves the player
    // nowhere at all -- swept that arm around him at a speed no Mario can walk,
    // and the whole squad spent its time chasing a target orbiting them. On
    // screen: eight Marios shuffling on the spot, never arriving, their walk
    // clips restarting.
    //
    // [`slot`] already says this in its own doc -- "the leader turning on the
    // spot should not send everyone shuffling around him" -- and takes care not
    // to rotate the cluster. The anchor it was placed at then rotated it anyway.
    //
    // A trailing anchor has no facing in it. It stays wherever it is relative to
    // him and is simply held at arm's length, so turning moves it not at all and
    // walking pulls it round behind him on its own.
    let here = Vec2::new(leader.translation.x, leader.translation.z);
    let trail = squad
        .anchor
        .map(|anchor| anchor - here)
        .filter(|arm| arm.length_squared() > 1e-6)
        .unwrap_or_else(|| {
            // Nothing to trail yet -- the first tick, or the leader standing
            // exactly on it. His back is as good a guess as any, and it is only
            // ever a seed.
            let behind = leader.rotation * Vec3::Z;
            -Vec2::new(behind.x, behind.z)
        });
    let anchor = here + trail.normalize_or_zero() * FOLLOW_DISTANCE;
    squad.anchor = Some(anchor);
    for (index, entity) in squad.members.iter().enumerate() {
        if let Ok((mut ally, _, _)) = allies.get_mut(*entity) {
            ally.goal = Some((anchor + slot(index, FOLLOW_SPACING), FOLLOW_ARRIVE));
        }
    }
    let dt = crate::player::FIXED_DT;
    let swept = Swept::of(&field, leader.translation);
    for order in squad.sent.iter_mut() {
        let Ok((mut ally, transform, route)) = allies.get_mut(order.who) else {
            continue;
        };
        ally.goal = Some((order.at, SEND_ARRIVE));
        if order.arrived {
            continue;
        }
        let here = Vec2::new(transform.translation.x, transform.translation.z);
        if here.distance(order.at) <= SEND_ARRIVE {
            order.arrived = true;
            continue;
        }
        // **Nowhere to walk is not the same as not getting anywhere, and it
        // must not be paid for at the same rate.** The stall clock below is
        // deliberately slow, because most of what it watches is a Mario making
        // a detour and it must not retire an order half way round one. But a
        // search that came back stranded has already settled every cell on this
        // side of whatever is in the way and found no route among them -- there
        // is nothing to wait six seconds for, and waiting is a Mario standing
        // against a wall with its walk clip running. So an order the
        // pathfinder has refused outright is retired in [`LOST_SECONDS`].
        //
        // See [`crate::path::Route::unreachable`] for why "stopped short" on
        // its own is not enough to go on: most partial routes are a search that
        // wanted more budget, and those get there in the end.
        //
        // The sweep is asked as well, and it is the better witness of the two.
        // A per-body A* is metered -- it settles a few thousand cells and gives
        // up -- so on a map with more ground than that it can never *prove* a
        // spot unreachable, only fail to find it in time. The flow field floods
        // the whole grid from the player with no budget at all, once a rebuild,
        // so what it does not reach is not reachable. See [`Swept`].
        //
        // Only while this Mario is actually walking *the order*: it may be off
        // fighting or fetching, and a route lost on the way to a slime says
        // nothing about the spot the player sent it to.
        let stranded = route.stranded() || !swept.reaches(order.at);
        if matches!(ally.plan, crate::goap::Goal::Obey { .. }) && stranded {
            order.lost_for += dt;
            if order.lost_for >= LOST_SECONDS {
                order.arrived = true;
                continue;
            }
        } else {
            // A route found again -- the world moved, or it was only ever the
            // one tick a body spends in the air -- puts the clock back.
            order.lost_for = 0.0;
        }
        // What it is actually walking at, which is a corner of its route when
        // it has one. See [`Sent::towards`].
        let aim = route.aim().map_or(order.at, |leg| Vec2::new(leg.x, leg.z));
        // A new thing to be walking at is a fresh start: the record it was
        // holding was about somewhere else.
        if aim.distance(order.towards) > PROGRESS {
            order.towards = aim;
            order.closest = f32::INFINITY;
            order.stuck_for = 0.0;
            continue;
        }
        // Getting somewhere, or not. Measured against the best it has ever
        // done rather than against last tick, because a Mario swinging round a
        // pond spends most of a detour not gaining ground and is not stuck --
        // what a stuck Mario cannot do is beat its own record, ever again.
        let range = Vec2::new(transform.translation.x, transform.translation.z).distance(aim);
        if range < order.closest - PROGRESS {
            order.closest = range;
            order.stuck_for = 0.0;
            continue;
        }
        order.stuck_for += dt;
        if order.stuck_for >= STUCK_SECONDS {
            // The order is as carried out as it is ever going to be. It keeps
            // the spot -- it will drift back towards it whenever it can, and a
            // fence it cannot pass is one the fight has to come through too --
            // but it stops being the only thing this Mario is allowed to want.
            order.arrived = true;
        }
    }
}

/// How much nearer its spot a Mario has to get for the walk to count as going
/// somewhere, and how long it may fail to before the order is written off.
///
/// Six seconds is a long time to give a body that walks at seven metres a
/// second -- long enough to swim a moat, round a pond, or wait out a press of
/// bodies at a gate -- and that is deliberate. Writing an order off early is
/// the squad ignoring the player; writing it off late is a few seconds of a
/// Mario leaning on a fence. Only one of those is a bug worth risking.
const PROGRESS: f32 = 0.25;
const STUCK_SECONDS: f32 = 6.0;

/// How long a Mario is given to find a way to its spot before an order to a
/// place with no way to it is written off.
///
/// A whole second rather than the tick the answer arrives on, because `lost` is
/// not only ever about the destination. A body in the air off a ledge, mid-jump
/// or a stride into the moat is somewhere the survey calls unwalkable, so the
/// search fails at the *start* end for a tick or two and says so the same way.
/// A second outlives all of those and is still a twentieth of the stall clock.
const LOST_SECONDS: f32 = 1.0;

/// How quickly a swimming ally is pulled to the height it floats at, as a
/// fraction of the remaining gap a second.
///
/// A pull rather than a snap for [`settle`]'s reason, and slow enough that
/// walking off a ledge into the moat is a body sinking and bobbing back up
/// rather than one that changes height between two frames.
const SWIM_RISE: f32 = 3.0;

/// How far off a straight line a Mario will swing to keep clear of what is in
/// the way, and how finely it looks between the two extremes.
///
/// Nine tries at ten degrees covers a right angle either side, tried in pairs
/// so that neither side of an obstacle is preferred and a squad splits round a
/// pond rather than filing along one bank of it. A right angle is the limit on
/// purpose: past it the Mario is walking away from where it is going, and the
/// thing that gets a body out of a dead end is [`crate::goap`] choosing
/// something else to do, not a walk step reversing.
const STEER_STEP: f32 = std::f32::consts::PI / 18.0;
const STEER_TRIES: usize = 9;

/// How far ahead a Mario looks, in strides.
///
/// One step is a thirtieth of a second of walking and far too short to steer
/// on: by the time the next footfall is wet, the one after it is in the middle
/// of the moat. A metre and a half is about four ticks of warning at marching
/// pace -- forty degrees of turn, which is enough to bend round a shoreline
/// without the swerve reading as a flinch.
const STEER_LOOKAHEAD: f32 = 45.0;

/// What a Mario finds in front of it costs, before [`crate::goap::Goal::caution`]
/// says how much this particular Mario minds.
///
/// Everything here is a *number* rather than a refusal, and that is the whole
/// design. The old rule was "take the first dry heading, and if there is none,
/// walk in anyway", which is a hard preference with a hard fallback: it cannot
/// express "go the long way round unless the long way is very long", and it
/// could not see a cliff at all. Scoring the candidates instead says all of it
/// in one comparison, and the fallback comes out for free -- with every heading
/// hazardous, the straight one wins on the detour term below and the Mario
/// wades in.
///
/// The relative sizes are the design:
///
///   * **A wall or a fence** costs the most by a long way. It is the one thing
///     here a Mario genuinely cannot cross, so walking at it is not a risk, it
///     is a body standing still.
///   * **A drop** costs more than water. A moat is a swim; a castle wall is a
///     fall, and the squad following the player along a parapet should not
///     shed a third of itself over the side.
///   * **Nothing to stand on at all** costs the same as a drop. Over this level
///     it is the edge of the map or the inside of a hill, and neither is
///     somewhere to walk.
///   * **Deep water** is the cheapest hazard, because it is the one the game
///     actually expects them to cross -- see [`crate::goap::WET_PENALTY`], which
///     is the same preference expressed against the *choice* rather than
///     against the route.
const WALL_COST: f32 = 4.0;
const DROP_COST: f32 = 1.4;
const VOID_COST: f32 = 1.4;
const WET_COST: f32 = 1.0;

/// What walking anywhere other than straight at the target costs.
///
/// `1 - cos(turn)`, so it is nothing for a shrug of a few degrees, a quarter at
/// sixty, and a whole unit at a right angle -- the fraction of the stride the
/// detour throws away. It is measured in the same units as the hazards above,
/// and comparing the two is what decides every question this function is asked:
/// at [`crate::goap::Goal::caution`] of 0.35, an order wades once the dry way
/// round costs more than about fifty degrees of turn; at 1.6, an ambling Mario
/// will turn the full right angle rather than get its feet wet.
const DETOUR_COST: f32 = 1.0;

/// The steepest ground there is, as rise over run.
///
/// **This is what sorts a hill from a cliff, in both directions, and it is
/// derived rather than chosen.** The level itself files a triangle as ground
/// only while it leans less than [`crate::level::GROUND_NORMAL_Y`] off
/// horizontal, which is a shade under forty-six degrees, or a rise of about one
/// in one. So over a lookahead of `reach`, the most the ground can legally
/// climb or fall is `reach * WALKABLE_SLOPE`, and anything beyond that is
/// something other than the ground continuing: a wall above, a drop below.
///
/// Written as a fraction of the stride rather than as a height, because a fixed
/// height is wrong at both ends. As a *climb* it would refuse the castle's own
/// ramps -- a Mario that treats a legal slope as a wall never leaves the lawn --
/// and as a *drop* it would call every steep path down a cliff.
///
/// The level's own number, so that ground the collision grid is happy to call
/// ground is never mistaken for either by anything walking over it.
use crate::level::WALKABLE_SLOPE;

/// How far above the probe a Mario looks for the ground ahead.
///
/// [`LevelData::ground_at`] answers with the highest ground *below* where it is
/// asked, so asking at foot height would find no ground at all on any slope
/// rising faster than about a metre in three -- and a Mario would treat every
/// hill on the castle grounds as a hole. Asked from high enough that the
/// steepest legal climb is inside the answer, [`WALKABLE_SLOPE`] then sorts what
/// comes back.
fn steer_reach_up(reach: f32) -> f32 {
    reach * WALKABLE_SLOPE
}

/// How much a Mario minds what is in the way, and whether water is part of it.
///
/// Two things rather than one because they answer to different callers.
/// `caution` comes off the goal -- an order is walked more boldly than an
/// errand, see [`crate::goap::Goal::caution`] -- while `water` is a fact about
/// the situation: a Mario already swimming, or one going to something that is
/// itself in the water, has nothing to gain by keeping out of it, and a Mario
/// that circles a pond it is standing in is the most broken-looking thing this
/// module can produce.
#[derive(Clone, Copy, Debug)]
pub struct Care {
    pub caution: f32,
    pub water: bool,
}

/// Bends a step around what is in front of it.
///
/// Returns the heading to actually walk, as a unit vector. The straight line is
/// tried first and taken whenever it is clear, so this costs one look ahead for
/// everything not near a shore, a wall or an edge.
///
/// **It weighs rather than refuses**, and that is what makes it both safe and
/// obedient. A Mario already standing in the moat, or one whose ball has rolled
/// into it, would otherwise be pinned: every direction is wet, no heading is
/// acceptable, and it stands there for the rest of the session. Here the
/// straight line is the cheapest thing on the list the moment every detour is
/// as bad as it is, so a Mario with nowhere better to go goes where it meant to
/// and swims -- see [`move_allies`], which is what carries it once it is in.
///
/// Separate from the walk step and taking a `LevelData` rather than a query, so
/// the rule can be exercised against a hand-built water box with no world
/// around it.
pub fn steer(level: &LevelData, from: Vec3, heading: Vec2, reach: f32, care: Care) -> Vec2 {
    let knee = Vec3::Y * crate::enemy::STEP_UP;
    // What one candidate heading runs into, in the units [`DETOUR_COST`] is
    // measured in. Nothing here is a veto: the sum is compared, and the
    // cheapest heading wins even when the cheapest is dreadful.
    let hazard = |direction: Vec2| -> f32 {
        let ahead = from + Vec3::new(direction.x, 0.0, direction.y) * reach;
        let mut cost = 0.0;
        // What it would be standing on a stride from here, asked before
        // anything else because the wall probe below needs it.
        let footing = level.ground_at(ahead + Vec3::Y * steer_reach_up(reach));
        // A fence. The knee is the same height [`crate::enemy::walk`] probes at
        // and for the same reason: a kerb passes under it, a railing does not.
        // Steepness is measured with the collision grid's own threshold, so a
        // Mario's idea of a wall and the level's are one idea.
        //
        // **The cast runs between the two ends' own ground heights rather than
        // at a fixed altitude, and that is not a nicety.** A horizontal ray from
        // knee height on any rising ground goes *into* the hill: at the foot of
        // a slope every direction but downhill reads as a wall, so the cheapest
        // heading is whichever one the Mario was already pointed at, it walks
        // into the slope, `resolve_walls` holds it out, and it stands there for
        // the rest of the session with a perfectly good route in hand. That is
        // the whole of "they still get stuck on the fence when they are ordered
        // somewhere" -- it was not the fence, it was the bank behind it.
        // [`crate::flow::FlowField::wall_between`] has followed the ground since
        // it was written, and for exactly this reason; this is the same probe
        // finally asking the same question.
        let far = match footing {
            Some((height, _)) => Vec3::new(ahead.x, height, ahead.z),
            None => ahead,
        };
        //
        // **Three rays a body's width apart, not one.** A single line down the
        // middle answers whether a *point* could take the step, and a Mario is a
        // cylinder that `LevelData::resolve_walls` holds a radius clear of
        // everything: a heading that threads the centre line past the end of a
        // wall walks the body's shoulder straight into it, and what that looks
        // like is a Mario pinned against the inside of a fence, sliding, with a
        // perfectly good route in hand. The survey has probed edges this way
        // since it learned the same lesson -- see
        // [`crate::flow::FlowField::wall_between`]. This is the walk step
        // finally asking the same question.
        let across = Vec3::new(-direction.y, 0.0, direction.x) * crate::player::PLAYER_RADIUS;
        if [Vec3::ZERO, across, -across].iter().any(|offset| {
            level
                .surface_hit(from + knee + *offset, far + knee + *offset)
                .is_some_and(|(_, normal)| normal.y.abs() <= crate::level::GROUND_NORMAL_Y)
        }) {
            cost += WALL_COST;
        }
        // And how far up or down that footing is.
        match footing {
            None => cost += VOID_COST,
            Some((height, _)) => {
                let step = height - from.y;
                let walkable = reach * WALKABLE_SLOPE;
                // **And the ground in between, not only the two ends.** A
                // stride that rises eighty centimetres over a metre and a half
                // is a gentle slope by the numbers and may still have a
                // knee-high lip at the bottom of it that the body cannot get
                // over -- see [`LevelData::climbable`], which is that question
                // asked properly. Only worth asking when the far end is *above*
                // the near one; nothing is climbed on the way down.
                if step > 0.0 && !level.climbable(from, far) {
                    cost += WALL_COST;
                }
                // Measured at the floor rather than at the walker's own height:
                // a shoreline is a slope, and a point sampled at the height it
                // is standing now reads as dry right up until it is swimming.
                let deep = level
                    .water_depth(far)
                    .is_some_and(|depth| depth > SWIMMING_DEPTH);
                if step > walkable {
                    cost += WALL_COST;
                } else if step < -walkable && !deep {
                    // **A drop into water is a swim rather than a fall.** Two
                    // reasons, and the second is the one that bites: a moat is
                    // not a cliff, and a Mario already swimming in one is
                    // floating several metres above its bed, so counting that
                    // as a drop would put the same cost on every heading it
                    // could take and leave the shore no more attractive than
                    // the deep end. What is wrong with going that way, if
                    // anything, is that it is wet -- which is the line below.
                    cost += DROP_COST;
                }
                if deep && care.water {
                    cost += WET_COST;
                }
            }
        }
        cost * care.caution
    };
    let straight = hazard(heading);
    if straight <= 0.0 {
        return heading;
    }
    let mut best = (straight, heading);
    for try_ in 1..=STEER_TRIES {
        let turn = try_ as f32 * STEER_STEP;
        // What the turn itself costs, paid once for the pair: the two sides of
        // a deflection are the same length.
        let detour = (1.0 - turn.cos()) * DETOUR_COST;
        // Nothing further round can beat what is already in hand, however clear
        // it is -- the detour alone has passed it. Stopping here rather than
        // running the arc out is most of what this costs on a busy shoreline.
        if detour >= best.0 {
            break;
        }
        for side in [turn, -turn] {
            let (sin, cos) = side.sin_cos();
            let bent = Vec2::new(
                heading.x * cos - heading.y * sin,
                heading.x * sin + heading.y * cos,
            );
            // Strictly better, so the smaller deflection and then the left-hand
            // one hold a tie: a heading is never swapped for an equally good
            // one, which is what would make a Mario on a shoreline waver.
            let cost = detour + hazard(bent);
            if cost < best.0 {
                best = (cost, bent);
            }
        }
    }
    best.1
}

/// How deep the water has to be before a Mario swims in it rather than walking
/// through it.
///
/// The player's own number, so a Mario in the squad and the Mario the player is
/// driving change behaviour at the same depth -- see
/// [`crate::player::submersion`], which is the rule this mirrors.
pub(crate) const SWIMMING_DEPTH: f32 = crate::player::SUBMERGED_DEPTH;

/// Walks each ally toward its plan, swims it if it is out of its depth, or lets
/// it amble where it stands.
pub fn move_allies(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    // Only to tell a corner that has been turned from one on the far side of a
    // wall. See [`crate::path::Route::leg`].
    field: Res<crate::flow::FlowField>,
    mut sounds: ResMut<SoundQueue>,
    // One still in the air out of the Mario pipe is flown by `pipe::fly`, and
    // walking it toward a goal at the same time would drag it out of its arc.
    mut allies: Query<
        (Entity, &mut Ally, &mut Transform, &mut crate::path::Route),
        Without<crate::pipe::Launched>,
    >,
) {
    let dt = crate::player::FIXED_DT;
    for (_entity, mut ally, mut transform, mut route) in &mut allies {
        // The order it was given outlives being released by exactly one tick,
        // which is how being released is noticed: it stands where the order left
        // it and ambles from there, rather than walking back to wherever it was
        // recruited. The *plan* is rewritten from scratch every tick by
        // `goap::plan`, so nothing here has to expire it.
        if !matches!(
            ally.plan,
            crate::goap::Goal::Obey { .. } | crate::goap::Goal::Hold { .. }
        ) && ally.goal.take().is_some()
        {
            ally.home = transform.translation;
            ally.amble_somewhere_else();
        }
        // How deep it is standing, which decides everything below: whether it
        // walks or swims, how fast, and whether it is held at the surface or
        // dropped onto the floor.
        let depth = level.water_depth(transform.translation);
        let swimming = depth.is_some_and(|depth| depth > SWIMMING_DEPTH);
        if swimming != ally.swimming {
            ally.swimming = swimming;
            sounds.push_at(Sfx::Splash, transform.translation);
        }
        // Mid-punch it stands where it is and throws it. `ally_combat` owns
        // the swing itself; what it means here is that a Mario is not walking.
        if ally.swing_left > 0.0 {
            ally.velocity = Vec3::ZERO;
            continue;
        }
        // Standing about between ambles -- but only when there is genuinely
        // nothing to do, which is exactly what an idle plan means. An ally with
        // nowhere to be is still for seconds at a time, which is what lets the
        // idle clip actually play.
        if ally.rest_left > 0.0 && !ally.plan.urgent() {
            ally.rest_left -= dt;
            ally.stand(dt);
            settle(&level, &mut transform, depth, swimming, dt);
            continue;
        }
        let here = Vec2::new(transform.translation.x, transform.translation.z);
        // Where it decided to go. Ambling is the only thing without a
        // destination of its own, and it walks to the spot it picked last time.
        //
        // Everything about *choosing* between a fight, an order, a ball and a
        // mast now lives in [`crate::goap`], which is a pure function over one
        // struct. What is left here is a body walking to a point -- see that
        // module for why the two had to come apart.
        let (target, arrive) = ally
            .plan
            .destination()
            .unwrap_or((ally.stroll, WANDER_ARRIVE));
        let to_target = target - here;
        let distance = to_target.length();
        if distance <= arrive {
            if !ally.plan.urgent() {
                // Arrived nowhere in particular: stand about a while, then
                // amble somewhere else. A real plan is somewhere in particular
                // -- picking a new stroll on top of one would send the Mario
                // away from the ball it is standing on before `nuclonium::haul` had
                // a chance to notice it had got there.
                ally.amble_somewhere_else();
            }
            ally.stand(dt);
            settle(&level, &mut transform, depth, swimming, dt);
            continue;
        }
        // **Where the feet actually point**, which is not always the goal. A
        // route is a list of corners each of which the Mario can see from the
        // one before, so following one turns "get to the far side of the
        // castle" -- a question no amount of looking ahead a stride answers --
        // into a series of walks across open ground, which is the only question
        // [`steer`] is any good at. With no route it walks at the goal, exactly
        // as it did before there was any of this. See [`crate::path`].
        let aim = route
            .leg(&field, transform.translation, tuning.path_spread)
            .map(|leg| Vec2::new(leg.x, leg.z))
            .unwrap_or(target);
        // Round the pond and along the top of the wall rather than through
        // one and off the other -- but only as far as this job is worth going
        // round. Water stops counting once the Mario is in it or what it came
        // for is, because a detour round a pond it is standing in is a Mario
        // walking in circles. See [`steer`].
        //
        // The straight line is towards the *corner*; how fast it walks and when
        // it has arrived are still measured against the goal, because arriving
        // at a corner is not arriving.
        let straight = (aim - here).normalize_or(to_target / distance);
        let goal_is_wet = level
            .water_depth(Vec3::new(target.x, transform.translation.y, target.y))
            .is_some_and(|depth| depth > SWIMMING_DEPTH);
        let heading = steer(
            &level,
            transform.translation,
            straight,
            STEER_LOOKAHEAD * dt,
            Care {
                caution: ally.plan.caution(),
                water: !swimming && !goal_is_wet,
            },
        );
        // Ease off over the last stride so arriving is not a hard stop. A plan
        // is a job and is walked at the squad's marching pace; ambling is not.
        // Swimming is its own speed, and the player's own -- a Mario in the
        // squad and the Mario the player is driving cross the moat together.
        let pace = match (swimming, ally.plan.urgent()) {
            (true, _) => tuning.mario_swim,
            (false, true) => tuning.ally_speed,
            (false, false) => AMBLE_SPEED,
        };
        let speed = pace * (distance / arrive.max(0.001)).min(1.0);
        let step = heading * speed * dt;
        // Stopped by walls, as the same cylinder the player is stopped by --
        // [`steer`] prefers not to walk into one, and this is what happens when
        // preferring was not enough. Without it a Mario is the one body on the
        // field that fences do not apply to: it walks through the railing, out
        // over the moat, and is put down on whatever the floor query hands
        // back.
        let wanted = transform.translation + Vec3::new(step.x, 0.0, step.y);
        transform.translation =
            level.resolve_walls(wanted, Vec3::Y, crate::player::PLAYER_RADIUS, PLAYER_HEIGHT);
        // Re-read after the step: it has moved, so the depth it is riding is
        // the depth where it now is rather than where it set off from.
        let arrived_depth = level.water_depth(transform.translation);
        settle(&level, &mut transform, arrived_depth, swimming, dt);
        // Faced along the way it is actually going rather than at the thing it
        // is going to, so a Mario swinging round a bay is not walking sideways
        // for the length of the detour.
        transform.rotation = Quat::from_rotation_y(step.x.atan2(step.y));
        ally.velocity = Vec3::new(step.x / dt, 0.0, step.y / dt);
        ally.state.motion = match swimming {
            // Mario has a swim clip and the ally animation reads the same
            // tables the player does, so this is the whole of making one swim.
            true => crate::player::Motion::Swim,
            false => crate::player::Motion::Run,
        };
        ally.state.speed = speed;
        ally.state.still_for = 0.0;
    }
}

/// Puts an ally at the height it belongs at: floating if it is swimming,
/// standing on the ground if it is not.
///
/// Its own function because three places in [`move_allies`] need it -- walking,
/// arriving and resting -- and an ally that is only re-seated on the ticks it
/// moves is one that sinks to the bottom of the moat the moment it stops.
///
/// The float is a pull rather than a snap, so breaking the surface is a body
/// rising through it rather than a body teleporting to it, and it is the
/// player's own `SWIM_FLOAT_DEPTH` so the two ride the water at the same height.
///
/// **Going down is paced and going up is not**, which is the same asymmetry
/// [`crate::enemy`] settles a slime with. A rise is a slope the Mario is walking
/// up and it is already limited by the walk: the floor query only ever offers
/// ground it could have stepped onto, and a stride is a fifth of a metre. A
/// *fall* has no such limit -- a Mario that walks off the castle wall is thirty
/// metres above the moat, and taking that in one assignment is not a fall, it is
/// the Mario ceasing to be up there and starting to be down here. [`steer`] is
/// what tries to stop this happening at all; this is what it looks like when it
/// happens anyway.
fn settle(
    level: &LevelData,
    transform: &mut Transform,
    depth: Option<f32>,
    swimming: bool,
    dt: f32,
) {
    if swimming {
        if let Some(depth) = depth {
            let rise = depth - crate::player::SWIM_FLOAT_DEPTH;
            transform.translation.y += rise * (SWIM_RISE * dt).min(1.0);
        }
        return;
    }
    if let Some(height) = level.floor_height(transform.translation + Vec3::Y * PLAYER_HEIGHT) {
        let drop = FALL_SPEED * dt;
        transform.translation.y += (height - transform.translation.y).max(-drop);
    }
}

/// How fast a Mario with nothing under it comes down, in metres a second.
///
/// The enemies' own falling speed, because a body dropping off the same wall
/// should reach the water at the same time whichever side it is on.
const FALL_SPEED: f32 = 14.0;

// -- gliding ----------------------------------------------------------------

/// How far an ally may have moved in one tick and still be drawn as having
/// travelled there.
///
/// A Mario walks four metres a second, so a third of a metre is a busy tick and
/// six metres is not a walk at all -- it is a warp pipe, a level swapping under
/// it, or a body being put somewhere. Interpolating one of those draws the ally
/// sliding across the map over the next thirty-third of a second, through
/// everything in between. `player::sync_visual` has no equivalent because the
/// player is never moved that way with the visual still attached.
const GLIDE_JUMP: f32 = 6.0;

/// Where an ally stood at the end of each of the last two ticks.
///
/// **The simulation runs at thirty steps a second and the game is drawn at
/// whatever the monitor does.** Luna has been drawn between two of those steps
/// since the beginning -- that is [`crate::player::RenderPose`], and it is the
/// only reason she does not judder -- but a Mario was drawn *at* them: the
/// same pose held for two or three frames and then jumped a whole tick's stride
/// at once. Beside a leader who glides, that reads as the squad stuttering
/// along behind her, which is exactly what it is.
///
/// The fix cannot be to interpolate the ally's `Transform` in place, because
/// that transform is not a picture -- it is where the Mario *is*. Forty systems
/// read it: the fight measures reach against it, the planner scores errands off
/// it, the walk steps from it. Smoothing it in place would feed a drawn
/// half-step back into the next tick's arithmetic, and a simulation whose input
/// depends on the frame rate is not a simulation.
///
/// So the pose is banked instead. [`bank`] writes down where the tick left the
/// ally, [`steady`] puts that exact pose back before the next tick reads it,
/// and [`glide`] draws whatever is in between. Every system in the fixed step
/// sees the same numbers it always saw; the drawn frames in between get the
/// smoothing, and nothing that runs on the fixed step can tell the difference.
///
/// One component rather than a field on [`Ally`], because it is nothing to do
/// with being an ally -- it is a body being drawn between two steps, which is
/// what anything simulated on a fixed step and drawn on a variable one needs.
#[derive(Component, Clone, Copy, Default)]
pub struct Glide {
    /// The tick before last, and the last tick. `None` until a tick has
    /// actually happened: an ally spawned this frame has one pose and nothing
    /// to interpolate it against.
    was: Option<(Vec3, Quat)>,
    now: Option<(Vec3, Quat)>,
}

impl Glide {
    /// Where to draw it, `alpha` of the way through the step after the one that
    /// has been simulated.
    ///
    /// A teleport is not interpolated -- see [`GLIDE_JUMP`] -- it is simply
    /// where it now is.
    pub fn between(&self, alpha: f32) -> Option<(Vec3, Quat)> {
        let (was, now) = (self.was?, self.now?);
        if was.0.distance(now.0) > GLIDE_JUMP {
            return Some(now);
        }
        Some((was.0.lerp(now.0, alpha), was.1.slerp(now.1, alpha)))
    }
}

/// Puts every ally back on its simulated pose, before the tick that reads it.
///
/// First in the fixed step, and that is the whole of its correctness: the
/// frames since the last tick have been drawing the ally somewhere between two
/// poses, and the tick about to run must start from the second of them rather
/// than from wherever the last drawn frame happened to leave it. See [`Glide`].
pub fn steady(mut allies: Query<(&Glide, &mut Transform), With<Ally>>) {
    for (glide, mut at) in &mut allies {
        let Some((translation, rotation)) = glide.now else {
            continue;
        };
        at.translation = translation;
        at.rotation = rotation;
    }
}

/// Writes down where this tick left every ally.
///
/// Last in the fixed step, after everything that could have moved one: the walk,
/// the fight, the warp pipe's arc. See [`Glide`].
pub fn bank(mut allies: Query<(&mut Glide, &Transform), With<Ally>>) {
    for (mut glide, at) in &mut allies {
        let pose = (at.translation, at.rotation);
        glide.was = glide.now.or(Some(pose));
        glide.now = Some(pose);
    }
}

/// Draws every ally between the last two ticks, once per frame.
///
/// The same job [`crate::player::sync_visual`] does for Luna, and off the same
/// clock: `overstep_fraction` is how far through the current step the wall
/// clock has got, so an ally is drawn exactly as far along its stride. See
/// [`Glide`].
pub fn glide(
    fixed_time: Res<Time<Fixed>>,
    mut allies: Query<(&Glide, &mut Transform), With<Ally>>,
) {
    let alpha = fixed_time.overstep_fraction().clamp(0.0, 1.0);
    for (glide, mut at) in &mut allies {
        let Some((translation, rotation)) = glide.between(alpha) else {
            continue;
        };
        at.translation = translation;
        at.rotation = rotation;
    }
}

/// Plays each ally's own clip, off the same tables the player uses.
pub fn animate_allies(
    animations: Res<CharacterAnimations>,
    allies: Query<(&Ally, &ActiveCharacter)>,
    mut players: Query<(
        &AllyAnimationRoot,
        &mut AnimationPlayer,
        &mut AnimationTransitions,
    )>,
) {
    for (root, mut player, mut transitions) in &mut players {
        let Ok((ally, character)) = allies.get(root.0) else {
            continue;
        };
        // Asked per ally rather than once for the system: a field can hold
        // Marios and Lunas at the same time, and one glTF finishing loading
        // before the other must not hold up the half that is ready.
        if !animations.ready(*character) {
            continue;
        }
        let (name, rate) = crate::animation::resolve(*character, &ally.state);
        let Some(clip) = animations.named(*character, name) else {
            continue;
        };
        // Through the shared applier rather than played here, so which clips
        // cycle and which hold their last pose is decided in one place for
        // the allies and the player alike.
        crate::animation::apply(&mut player, &mut transitions, clip, name, rate, false);
    }
}

/// The ring drawn on the ground while the whistle is open.
#[derive(Component)]
pub struct WhistleCircle;

/// The ring left on the ground where an order landed.
#[derive(Component)]
pub struct OrderCircle;

/// The ring's own transform.
///
/// Every exclusion here is load-bearing. Bevy proves two queries disjoint from
/// their `Without` filters alone -- `With<WhistleCircle>` and `With<Ally>`
/// describe different entities to a reader, but nothing to the scheduler -- so
/// a write to `Transform` that does not name every other `Transform` query in
/// the same system is rejected when the system is initialised, which in a
/// windowed build is a game that opens and shuts without a word.
type CircleQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (
        With<WhistleCircle>,
        Without<Player>,
        Without<Camera3d>,
        Without<Ally>,
    ),
>;

/// A flat annulus of unit outer radius, scaled to the circle's size when
/// drawn. Built here rather than from a torus so the ring stays flat on the
/// ground and its thickness stays proportional to how wide it has grown.
///
/// Shared with [`crate::stellarator`], which draws a machine's footprint with
/// it. Two rings a player is asked to read while holding two different buttons
/// should be visibly the same kind of mark, and one mesh is how that stays
/// true.
pub fn ring_mesh() -> Mesh {
    const SEGMENTS: usize = 64;
    const INNER: f32 = 0.94;
    let mut positions = Vec::with_capacity(SEGMENTS * 2);
    let mut indices = Vec::with_capacity(SEGMENTS * 6);
    for step in 0..SEGMENTS {
        let angle = step as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        positions.push([sin, 0.0, cos]);
        positions.push([sin * INNER, 0.0, cos * INNER]);
        let outer = (step * 2) as u32;
        let inner = outer + 1;
        let next_outer = ((step + 1) % SEGMENTS * 2) as u32;
        let next_inner = next_outer + 1;
        indices.extend_from_slice(&[outer, inner, next_inner, outer, next_inner, next_outer]);
    }
    let count = positions.len();
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; count]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; count]);
    mesh.insert_indices(bevy::mesh::Indices::U32(indices));
    mesh
}

/// Spawns the (initially hidden) whistle and order rings. Called from startup.
///
/// Two entities off one mesh. They outlive a level for the reason the build
/// preview does -- what you command with must not go away when you change
/// level -- and they are told apart by colour: the whistle's ring is the warm
/// one you are holding open, the order's is the cool one it left behind.
pub fn spawn_circle(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let ring = meshes.add(ring_mesh());
    commands.spawn((
        OrderCircle,
        bevy::light::NotShadowCaster,
        Mesh3d(ring.clone()),
        // Its own material handle rather than a share of the whistle's,
        // because `order_ring` fades this one by writing its alpha every
        // frame and a shared handle would drag the whistle's ring with it.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.45, 0.90, 1.0, RING_ALPHA),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        })),
        Visibility::Hidden,
    ));
    commands.spawn((
        WhistleCircle,
        bevy::light::NotShadowCaster,
        Mesh3d(ring),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.94, 0.45, RING_ALPHA),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        })),
        Visibility::Hidden,
    ));
}

/// The whistle button: held opens a circle, released gives the order.
///
/// Runs at the render rate rather than on the fixed step, because the circle
/// grows with wall-clock time and is drawn every frame; the orders it produces
/// are one-shot writes onto the squad, which the fixed step then acts on.
#[allow(clippy::too_many_arguments)]
pub fn whistle(
    time: Res<Time>,
    mut input: ResMut<crate::input::InputState>,
    level: Res<LevelData>,
    // Where the order may actually be *put*, which is a different question
    // from where it was pointed. See [`Squad::send`].
    field: Res<crate::flow::FlowField>,
    mut whistle: ResMut<Whistle>,
    mut mark: ResMut<OrderMark>,
    mut squad: ResMut<Squad>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
    player: Query<&Transform, With<Player>>,
    allies: Query<(Entity, &Transform), With<Ally>>,
    mut circle: CircleQuery,
) {
    let (Ok(camera), Ok(leader)) = (camera.single(), player.single()) else {
        return;
    };
    let released = crate::input::InputState::take(&mut input.squad_released);
    if input.squad || released {
        // The aim is refreshed on the press as well as on the hold, so a tap
        // too short to have grown a circle still sends the squad somewhere.
        whistle.aim = aim_point(
            &level,
            camera.translation,
            Vec3::from(camera.forward()),
            leader.translation,
            // As far as the level goes. An order has no range. See
            // [`order_reach`].
            order_reach(&level),
        );
    }
    if input.squad {
        let held = whistle.held_for.unwrap_or(0.0) + time.delta_secs();
        whistle.held_for = Some(held);
        whistle.radius = circle_radius(held);
    }
    if released {
        let held = whistle.held_for.take().unwrap_or(0.0);
        if held < TAP_SECONDS {
            // A tap is an order to the squad it already has, and the ring it
            // leaves is the only report the player gets that it went anywhere
            // -- the whistle's own circle was never drawn for a press this
            // short. Marked from the count `send` returns, so an order to
            // nobody still draws the smallest ring rather than nothing.
            mark.left_at(squad.send(&field, whistle.aim));
        } else {
            let inside: Vec<_> = allies
                .iter()
                .filter(|(_, transform)| {
                    in_circle(transform.translation, whistle.aim, whistle.radius)
                })
                .map(|(entity, _)| entity)
                .collect();
            squad.recruit(&inside);
        }
    }
    if let Ok((mut transform, mut visibility)) = circle.single_mut() {
        let showing = whistle.showing();
        *visibility = if showing {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        if showing {
            // Just clear of the ground, so the ring is not half-buried in the
            // slope it is drawn on.
            transform.translation = whistle.aim + Vec3::Y * 0.05;
            transform.scale = Vec3::new(whistle.radius, 1.0, whistle.radius);
        }
    }
}

/// Draws the mark left where the last order landed, and ages it out.
///
/// Its own system rather than a tail on [`whistle`] because that one already
/// writes the whistle circle's `Transform`, and a second `Transform` query in
/// the same system has to name every exclusion the first one does -- see
/// [`CircleQuery`] for what that costs. Chained after it in `presentation`, so
/// a mark left this frame is drawn this frame rather than a frame stale.
pub fn order_ring(
    time: Res<Time>,
    mut mark: ResMut<OrderMark>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ring: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<StandardMaterial>,
        ),
        With<OrderCircle>,
    >,
) {
    if let Some(age) = mark.age.as_mut() {
        *age += time.delta_secs();
    }
    let fade = mark.fade();
    let Ok((mut transform, mut visibility, material)) = ring.single_mut() else {
        return;
    };
    if fade <= 0.0 {
        *visibility = Visibility::Hidden;
        // Spent, so stop ageing a number nothing reads again.
        mark.age = None;
        return;
    }
    *visibility = Visibility::Visible;
    let radius = mark.drawn_radius();
    // Just clear of the ground, so the ring is not half-buried in the slope it
    // is drawn on -- the same 5cm the whistle's ring and the build preview's
    // footprint are lifted by.
    transform.translation = mark.at + Vec3::Y * 0.05;
    transform.scale = Vec3::new(radius, 1.0, radius);
    if let Some(mut material) = materials.get_mut(&material.0) {
        material.base_color.set_alpha(RING_ALPHA * fade);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drawn pose is somewhere on the stride, never past either end of it.
    ///
    /// The whole safety argument for [`Glide`] is that it only ever draws
    /// *between* two poses the simulation actually produced. Anything that ran
    /// ahead of the second one would be a Mario drawn where the fight has not
    /// put it yet, which is the extrapolation this deliberately is not.
    #[test]
    fn the_drawn_pose_stays_between_the_two_ticks_it_is_between() {
        let (was, now) = (Vec3::ZERO, Vec3::new(0.12, 0.0, 0.0));
        let glide = Glide {
            was: Some((was, Quat::IDENTITY)),
            now: Some((now, Quat::from_rotation_y(1.0))),
        };
        for step in 0..=10 {
            let alpha = step as f32 / 10.0;
            let (at, _) = glide.between(alpha).expect("a banked pose was not drawn");
            let along = (at - was).dot(now - was) / (now - was).length_squared();
            assert!(
                (-1e-5..=1.0 + 1e-5).contains(&along),
                "drawn {along} of the way along a stride at alpha {alpha}"
            );
        }
        assert_eq!(glide.between(0.0).unwrap().0, was);
        assert_eq!(glide.between(1.0).unwrap().0, now);
    }

    /// A Mario put somewhere is put there, rather than sliding across the map.
    ///
    /// A warp pipe, a respawn and a level swap all move a body outright, and
    /// interpolating one of those draws it travelling through everything in
    /// between over the next thirty-third of a second. See [`GLIDE_JUMP`].
    #[test]
    fn a_mario_that_was_put_somewhere_does_not_glide_there() {
        let there = Vec3::new(GLIDE_JUMP * 3.0, 0.0, 0.0);
        let glide = Glide {
            was: Some((Vec3::ZERO, Quat::IDENTITY)),
            now: Some((there, Quat::IDENTITY)),
        };
        assert_eq!(
            glide.between(0.5).unwrap().0,
            there,
            "a teleport was drawn as a walk"
        );
    }

    /// Nothing to interpolate is drawn as nothing, not as the origin.
    ///
    /// An ally spawned this frame has one pose and no history, and a `Glide`
    /// that answered `Vec3::ZERO` to that would put every new Mario at the
    /// middle of the map for one frame.
    #[test]
    fn a_mario_with_no_history_is_left_where_it_stands() {
        assert!(Glide::default().between(0.5).is_none());
        let banked = Glide {
            was: None,
            now: Some((Vec3::X, Quat::IDENTITY)),
        };
        assert!(banked.between(0.5).is_none());
    }

    #[test]
    fn a_cluster_spreads_without_stacking_or_lining_up() {
        let places: Vec<_> = (0..24).map(|index| slot(index, FOLLOW_SPACING)).collect();
        for (i, a) in places.iter().enumerate() {
            for (j, b) in places.iter().enumerate() {
                if i != j {
                    assert!(
                        a.distance(*b) > 0.4,
                        "slots {i} and {j} are on top of each other"
                    );
                }
            }
        }
        // The golden angle exists to stop spokes forming: no two consecutive
        // members should share a bearing.
        for pair in places.windows(2).skip(1) {
            let (a, b) = (pair[0], pair[1]);
            assert!(
                a.normalize().dot(b.normalize()) < 0.99,
                "the cluster lined up into a spoke"
            );
        }
        // And it grows outward rather than piling into one ring.
        assert!(places[23].length() > places[1].length());
    }

    #[test]
    fn the_circle_grows_from_a_tap_to_its_cap() {
        assert_eq!(circle_radius(0.0), CIRCLE_MIN_RADIUS);
        assert_eq!(circle_radius(TAP_SECONDS), CIRCLE_MIN_RADIUS);
        let half = circle_radius(TAP_SECONDS + CIRCLE_GROW_SECONDS * 0.5);
        assert!(
            half > CIRCLE_MIN_RADIUS && half < CIRCLE_MAX_RADIUS,
            "{half}"
        );
        let grown = circle_radius(TAP_SECONDS + CIRCLE_GROW_SECONDS);
        assert!((grown - CIRCLE_MAX_RADIUS).abs() < 1e-3, "{grown}");
        // Holding it forever does not grow it past the cap.
        assert!(circle_radius(60.0) <= CIRCLE_MAX_RADIUS + 1e-3);
    }

    #[test]
    fn the_circle_reaches_across_but_not_up() {
        let centre = Vec3::new(0.0, 0.0, 0.0);
        assert!(in_circle(Vec3::new(3.0, 0.0, 0.0), centre, 4.0));
        assert!(!in_circle(Vec3::new(9.0, 0.0, 0.0), centre, 4.0));
        // Somebody on the castle roof is not in a circle drawn on the lawn.
        assert!(!in_circle(
            Vec3::new(0.0, RECRUIT_HEIGHT + 1.0, 0.0),
            centre,
            4.0
        ));
        assert!(in_circle(Vec3::new(0.0, 2.0, 0.0), centre, 4.0));
    }

    #[test]
    fn recruiting_calls_back_an_ally_already_sent_somewhere() {
        let field = crate::flow::FlowField::new(&lawn());
        let mut squad = Squad::default();
        let ally = Entity::from_raw_u32(7).unwrap();
        squad.members.push(ally);
        squad.send(&field, Vec3::new(5.0, 0.0, 5.0));
        assert_eq!(squad.sent.len(), 1);
        assert!(squad.members.is_empty());
        // Whistled again, he drops the order rather than walking on deaf.
        assert_eq!(squad.recruit(&[ally]), 1);
        assert!(squad.sent.is_empty());
        assert_eq!(squad.members, vec![ally]);
        // And whistling him twice does not enlist him twice.
        assert_eq!(squad.recruit(&[ally]), 0);
        assert_eq!(squad.members.len(), 1);
    }

    #[test]
    fn sending_spreads_the_squad_around_the_spot() {
        let mut squad = Squad::default();
        for raw in 0..5 {
            squad.members.push(Entity::from_raw_u32(raw).unwrap());
        }
        let field = crate::flow::FlowField::new(&lawn());
        let target = Vec2::new(10.0, -4.0);
        squad.send(&field, Vec3::new(target.x, 0.0, target.y));
        assert_eq!(squad.marching(), 5);
        let mut seen: Vec<Vec2> = Vec::new();
        for order in &squad.sent {
            assert!(order.at.distance(target) < SEND_SPACING * 3.0);
            assert!(
                seen.iter().all(|other| other.distance(order.at) > 0.5),
                "two allies were sent to the same place"
            );
            seen.push(order.at);
        }
    }

    /// Turning the camera does not send the squad running round the player.
    ///
    /// This is the formation jitter, and the numbers are the point: the anchor
    /// sat on a 3.3 m arm off the leader's back, so a half-turn on the spot --
    /// one flick of a mouse, moving the player nowhere -- threw it 6.6 m across
    /// and every Mario chased it. Walking, on the other hand, must still drag
    /// the group along behind.
    #[test]
    fn turning_on_the_spot_does_not_move_the_formation() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut squad = Squad::default();
        let allies: Vec<Entity> = (0..4)
            .map(|index| {
                world
                    .spawn((Ally::new(Vec3::ZERO, index as f32), Transform::default()))
                    .id()
            })
            .collect();
        squad.members.extend(allies.iter().copied());
        world.insert_resource(squad);
        // Nothing here has been sent anywhere, so the sweep is never consulted
        // -- but `update_goals` asks for it, and a system missing a resource is
        // a system that does not run at all.
        world.insert_resource(crate::flow::FlowField::new(&lawn()));
        let leader = world.spawn((Player, Transform::default())).id();

        let goals = |world: &mut World| -> Vec<Vec2> {
            world.run_system_once(update_goals).expect("no run");
            allies
                .iter()
                .map(|ally| {
                    world
                        .get::<Ally>(*ally)
                        .expect("gone")
                        .goal
                        .expect("no goal")
                        .0
                })
                .collect()
        };
        let facing_one_way = goals(&mut world);
        // A half turn, standing still.
        world.get_mut::<Transform>(leader).unwrap().rotation =
            Quat::from_rotation_y(std::f32::consts::PI);
        let facing_the_other = goals(&mut world);
        for (before, after) in facing_one_way.iter().zip(&facing_the_other) {
            assert!(
                before.distance(*after) < 1e-3,
                "turning on the spot moved a slot {:.2} m",
                before.distance(*after)
            );
        }

        // But walking does bring them along: ten metres forward and the group
        // is gathering ten metres further on, still trailing at arm's length.
        world.get_mut::<Transform>(leader).unwrap().translation = Vec3::new(0.0, 0.0, 10.0);
        let walked = goals(&mut world);
        for (before, after) in facing_one_way.iter().zip(&walked) {
            assert!(
                (after.y - before.y - 10.0).abs() < 1e-3,
                "the formation did not follow him: {before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn disbanding_clears_both_lists() {
        let mut squad = Squad::default();
        squad.members.push(Entity::from_raw_u32(1).unwrap());
        squad.sent.push(Sent {
            who: Entity::from_raw_u32(2).unwrap(),
            at: Vec2::ZERO,
            arrived: true,
            towards: Vec2::ZERO,
            closest: 0.0,
            stuck_for: 0.0,
            lost_for: 0.0,
        });
        assert_eq!(squad.disband(), 2);
        assert!(squad.members.is_empty() && squad.sent.is_empty());
    }

    #[test]
    fn the_aim_lands_on_the_castle_lawn_in_front_of_the_player() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        // A camera behind and above him, looking down at the ground ahead.
        let origin = player + Vec3::new(0.0, 6.0, 9.0);
        let direction = (Vec3::new(player.x, player.y, player.z - 8.0) - origin).normalize();
        let aim = aim_point(&level, origin, direction, player, order_reach(&level));
        let flat = Vec2::new(aim.x - player.x, aim.z - player.z).length();
        assert!(
            flat >= AIM_MIN_RANGE,
            "aimed {flat} away, inside his own boots"
        );
        // It is ahead of him, on the bearing he is looking down.
        assert!(aim.z < player.z, "the aim landed behind the player");
        // And it is on the ground rather than floating over it.
        let floor = level.floor_height(aim + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some_and(|height| (height - aim.y).abs() < 0.5),
            "the aim is not on the floor: {aim:?} over {floor:?}"
        );
    }

    #[test]
    fn aiming_over_the_moat_walks_the_target_back_to_solid_ground() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        // Nearly level with the horizon, so the ray runs out over the moat and
        // off the edge of the map without ever meeting ground.
        let origin = player + Vec3::new(0.0, 2.0, 6.0);
        let direction = Vec3::new(0.0, 0.02, -1.0).normalize();
        let aim = aim_point(&level, origin, direction, player, order_reach(&level));
        let floor = level.floor_height(aim + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some(),
            "the order was sent somewhere with no floor under it: {aim:?}"
        );
    }

    /// An order reaches as far as the player can see, with no cap on it.
    ///
    /// **The cap did not refuse a long order, it quietly moved it.** Anything
    /// beyond twenty-six metres was pulled back down the bearing to twenty-six,
    /// so pointing across the courtyard sent the squad to a spot a third of the
    /// way there -- an order that looks, from the player's chair, like it was
    /// misunderstood. Sixty metres of lawn, and the aim has to land on the far
    /// end of it.
    #[test]
    fn an_order_has_no_range() {
        let level = lawn();
        let player = Vec3::new(0.0, 0.0, 50.0);
        let target = Vec3::new(0.0, 0.0, -50.0);
        let origin = player + Vec3::new(0.0, 6.0, 6.0);
        let aim = aim_point(
            &level,
            origin,
            (target - origin).normalize(),
            player,
            order_reach(&level),
        );
        assert!(
            aim.distance(target) < 1.0,
            "a hundred-metre order landed at {aim:?} rather than at {target:?}"
        );
        // And the reach really is the level rather than a bigger number that
        // happens to cover this lawn.
        assert!(
            order_reach(&level) >= 120.0,
            "the reach is {} on a 120 m lawn",
            order_reach(&level)
        );
    }

    /// A lawn with a wall standing across it.
    ///
    /// The wall is the thing a ray march could not see: it is vertical, so
    /// there is no "floor" anywhere along it to fall under, and a level ray
    /// aimed at its face used to run straight through and land on the lawn
    /// beyond. See [`aim_point`].
    fn lawn_with_a_wall() -> LevelData {
        let vertices = vec![
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
            // The wall's face, standing on the lawn at z = -10.
            Vec3::new(-8., 0., -10.),
            Vec3::new(8., 0., -10.),
            Vec3::new(8., 6., -10.),
            Vec3::new(-8., 6., -10.),
        ];
        LevelData::new(
            vertices,
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            vec![],
        )
    }

    /// An order aimed at a wall lands on the wall, not on the lawn behind it.
    ///
    /// The whole of what the raycast bought. The march this replaced only ever
    /// asked "is this sample under the floor", and along a level ray the answer
    /// is no from one end to the other -- wall or no wall -- so the aim fell
    /// through to its out-of-range fallback and put the order twenty-six metres
    /// out, through the wall and well past it.
    #[test]
    fn an_aim_at_a_wall_stops_at_the_wall() {
        let level = lawn_with_a_wall();
        let player = Vec3::new(0.0, 0.0, 10.0);
        // Off his shoulder, which is where the camera always is.
        let origin = Vec3::new(3.0, 2.0, 16.0);
        let direction = Vec3::new(0.0, 1.0, -26.0).normalize();
        let aim = aim_point(&level, origin, direction, player, order_reach(&level));
        assert!(
            (aim.z + 10.0).abs() < 0.1,
            "the aim went through the wall to {aim:?}"
        );
        assert!(
            aim.y > 1.0,
            "the aim slid down the wall to the lawn: {aim:?}"
        );
    }

    /// And it lands where the crosshair is rather than on the player's bearing.
    ///
    /// The aim used to be handed back as a point in front of the *player*, on
    /// the bearing from him to the hit, which quietly slid every order sideways
    /// by however far the camera sits off his shoulder. With the ring now drawn
    /// where the order landed, that skew is visible: the mark would sit next to
    /// the thing the player was pointing at.
    #[test]
    fn an_order_lands_under_the_crosshair_rather_than_beside_it() {
        let level = lawn_with_a_wall();
        let player = Vec3::new(0.0, 0.0, 10.0);
        let origin = Vec3::new(3.0, 2.0, 16.0);
        let direction = Vec3::new(0.0, 1.0, -26.0).normalize();
        let aim = aim_point(&level, origin, direction, player, order_reach(&level));
        // The ray runs down x = 3, so that is where it meets the wall. On the
        // player's own bearing -- straight up -x = 0 -- it would be at zero.
        assert!(
            (aim.x - 3.0).abs() < 0.1,
            "the aim was pulled onto the player's bearing: {aim:?}"
        );
    }

    /// A mark is left where the order landed, and it fades out rather than
    /// staying up for ever.
    #[test]
    fn an_order_leaves_a_ring_that_fades() {
        let field = crate::flow::FlowField::new(&lawn());
        let mut squad = Squad::default();
        for raw in 0..6 {
            squad.members.push(Entity::from_raw_u32(raw).unwrap());
        }
        let mut mark = OrderMark::default();
        assert_eq!(mark.fade(), 0.0, "a ring was drawn before any order");

        let aim = Vec3::new(4.0, 0.0, -9.0);
        let landed = squad.send(&field, aim);
        mark.left_at(landed);
        assert_eq!(mark.at, landed.at);
        assert_eq!(mark.fade(), 1.0);
        // It pops open rather than arriving at full width.
        assert!(mark.drawn_radius() < mark.radius);
        mark.age = Some(ORDER_RING_POP_SECONDS);
        assert!((mark.drawn_radius() - mark.radius).abs() < 1e-5);

        // Half spent, half solid.
        mark.age = Some(ORDER_RING_SECONDS * 0.5);
        assert!((mark.fade() - 0.5).abs() < 1e-5, "{}", mark.fade());
        mark.age = Some(ORDER_RING_SECONDS);
        assert_eq!(mark.fade(), 0.0, "the ring never went away");

        // And it is drawn wide enough to hold every Mario it sent.
        let middle = Vec2::new(landed.at.x, landed.at.z);
        for order in &squad.sent {
            assert!(
                order.at.distance(middle) <= landed.radius,
                "a Mario was sent {:.2} m out of a {:.2} m ring",
                order.at.distance(middle),
                landed.radius
            );
        }
    }

    /// An order to nobody still lands somewhere and still leaves a ring.
    #[test]
    fn an_order_with_nobody_to_carry_it_out_still_lands() {
        let field = crate::flow::FlowField::new(&lawn());
        let mut squad = Squad::default();
        let landed = squad.send(&field, Vec3::new(4.0, 0.0, -9.0));
        assert!(squad.sent.is_empty(), "an order to nobody sent somebody");
        assert!(landed.radius > 0.0, "an order left no mark at all");
    }

    #[test]
    fn aiming_straight_down_puts_the_target_at_his_feet() {
        let (level, _) = crate::level::load();
        let player = Vec3::new(-13.28, 3.0, 46.64);
        let aim = aim_point(
            &level,
            player + Vec3::Y * 5.0,
            Vec3::NEG_Y,
            player,
            order_reach(&level),
        );
        assert_eq!(aim, player);
    }

    /// Sixty metres of flat ground in every direction, with nothing on it.
    ///
    /// The ground a cluster is *supposed* to land on, for the tests about what
    /// a cluster is rather than about what the ground does to one.
    fn lawn() -> LevelData {
        let corners = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        LevelData::new(corners.to_vec(), vec![[0, 1, 2], [0, 2, 3]], vec![])
    }

    /// A flat lawn with a pond cut into one half of it.
    ///
    /// Ground everywhere, water only where the box says, so a walker can stand
    /// on either side and the shoreline is a straight line at `z = 0`.
    fn lawn_with_a_pond() -> LevelData {
        let corners = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        LevelData::new(
            corners.to_vec(),
            vec![[0, 1, 2], [0, 2, 3]],
            vec![crate::level::WaterBox {
                min_x: -20.,
                min_z: 0.,
                max_x: 20.,
                max_z: 40.,
                // Above the floor, so anything inside the box is out of its
                // depth rather than paddling.
                surface_y: 3.0,
            }],
        )
    }

    /// A Mario minding the water as much as an ordinary errand does.
    fn wary() -> Care {
        Care {
            caution: 1.0,
            water: true,
        }
    }

    #[test]
    fn a_dry_heading_is_taken_as_it_is() {
        let level = lawn_with_a_pond();
        // Walking away from the pond, over open lawn: no deflection at all, and
        // that is the case that has to stay free -- it is every step the squad
        // takes that is not near a shore.
        let away = Vec2::new(0.0, -1.0);
        let kept = steer(&level, Vec3::new(0.0, 0.0, -5.0), away, 6.0, wary());
        assert!((kept - away).length() < 1e-6, "{kept:?}");
    }

    #[test]
    fn a_mario_walks_round_a_pond_rather_than_into_it() {
        let level = lawn_with_a_pond();
        // Standing just short of the shore, pointed straight at the water.
        let from = Vec3::new(0.0, 0.0, -2.0);
        let into = Vec2::new(0.0, 1.0);
        let bent = steer(&level, from, into, 8.0, wary());
        assert!(
            (bent - into).length() > 1e-3,
            "walked straight in: {bent:?}"
        );
        // And what it picked is genuinely dry, which is the whole point -- a
        // deflection that still ends in the pond is not avoidance.
        let ahead = from + Vec3::new(bent.x, 0.0, bent.y) * 8.0;
        assert!(
            level
                .water_depth(ahead)
                .is_none_or(|depth| depth <= SWIMMING_DEPTH),
            "the detour is wet too: {ahead:?}"
        );
        // Still roughly the way it wanted to go, rather than a right-about
        // turn: a detour is scored against what it throws away, so the smallest
        // one that works wins.
        assert!(bent.dot(into) > 0.0, "it turned round: {bent:?}");
    }

    #[test]
    fn something_ringed_by_water_goes_where_it_meant_to() {
        // **The give-up case, and it is the one that keeps this safe.** A Mario
        // already in the pond has no dry heading anywhere; refusing all of them
        // would pin it there for the rest of the session. It swims out instead.
        let level = lawn_with_a_pond();
        let middle = Vec3::new(0.0, 0.0, 20.0);
        let wanted = Vec2::new(0.0, 1.0);
        assert_eq!(steer(&level, middle, wanted, 4.0, wary()), wanted);
    }

    /// A lawn with a sheer eight-metre drop across the middle of it.
    ///
    /// Two flat slabs, the far one far below, with nothing but air where they
    /// meet. Nothing here is a *wall* -- there is no vertical face to cast
    /// against -- so what a walker notices is only what it would be standing on
    /// a stride further along, which is the question a ledge asks.
    fn lawn_with_a_ledge() -> LevelData {
        let top = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 0.),
            Vec3::new(-60., 0., 0.),
        ];
        let bottom = [
            Vec3::new(-60., -8., 0.),
            Vec3::new(60., -8., 0.),
            Vec3::new(60., -8., 60.),
            Vec3::new(-60., -8., 60.),
        ];
        LevelData::new(
            top.iter().chain(bottom.iter()).copied().collect(),
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            Vec::new(),
        )
    }

    #[test]
    fn a_mario_walks_along_a_ledge_rather_than_off_it() {
        let level = lawn_with_a_ledge();
        // Three metres back from the edge, walking straight at it.
        let from = Vec3::new(0.0, 0.0, -3.0);
        let over = Vec2::new(0.0, 1.0);
        let bent = steer(&level, from, over, 6.0, wary());
        assert!((bent - over).length() > 1e-3, "walked off: {bent:?}");
        // What it picked is ground at its own level rather than the bottom of
        // the drop -- a deflection that still ends in mid-air is not avoidance.
        let ahead = from + Vec3::new(bent.x, 0.0, bent.y) * 6.0;
        let footing = level.ground_at(ahead + Vec3::Y * steer_reach_up(6.0));
        assert!(
            footing.is_some_and(|(height, _)| height - from.y > -6.0 * WALKABLE_SLOPE),
            "the detour goes over the edge too: {footing:?}"
        );
        // Still broadly where it was going. A right-about turn is not steering.
        assert!(bent.dot(over) > 0.0, "it turned round: {bent:?}");
    }

    /// A hill is not a cliff, and a Mario that treats one as the other never
    /// leaves the lawn.
    ///
    /// The ramp here climbs a metre in a metre, which is as steep as ground is
    /// allowed to get before [`crate::level::GROUND_NORMAL_Y`] stops calling it
    /// ground at all -- so this is the exact case [`WALKABLE_SLOPE`] is set
    /// against, and it is a wall to any fixed idea of how far a stride may
    /// climb. Both directions, because the same probe is what tells a path down
    /// a hill from a drop off the castle.
    #[test]
    fn the_steepest_ground_the_level_allows_is_still_ground() {
        let ramp = [
            Vec3::new(-60., -60., -60.),
            Vec3::new(60., -60., -60.),
            Vec3::new(60., 60., 60.),
            Vec3::new(-60., 60., 60.),
        ];
        let level = LevelData::new(ramp.to_vec(), vec![[0, 1, 2], [0, 2, 3]], Vec::new());
        // Standing in the middle of it, which is where the slab crosses zero.
        let from = Vec3::new(0.0, 0.0, 0.0);
        let uphill = Vec2::new(0.0, 1.0);
        assert_eq!(steer(&level, from, uphill, 4.0, wary()), uphill);
        assert_eq!(steer(&level, from, -uphill, 4.0, wary()), -uphill);
    }

    /// The whole of "prefer not to, rather than never": the same shoreline,
    /// two Marios, and the one with an order from the player crosses it.
    #[test]
    fn an_order_wades_where_an_errand_would_go_round() {
        let level = lawn_with_a_pond();
        // Pointed at the water, with dry lawn a long way round to either side.
        let from = Vec3::new(0.0, 0.0, -2.0);
        let into = Vec2::new(0.0, 1.0);
        let errand = steer(
            &level,
            from,
            into,
            8.0,
            Care {
                caution: crate::goap::Goal::Fetch {
                    ball: Entity::from_raw_u32(1).unwrap(),
                    at: Vec2::ZERO,
                    arrive: 1.0,
                }
                .caution(),
                water: true,
            },
        );
        assert!(
            (errand - into).length() > 1e-3,
            "an errand waded in: {errand:?}"
        );
        // The same step, walked under an order the player gave. The pond is
        // twenty metres of detour and the order is worth more than that.
        let ordered = steer(
            &level,
            from,
            into,
            8.0,
            Care {
                caution: crate::goap::Goal::Obey {
                    at: Vec2::ZERO,
                    arrive: 1.0,
                }
                .caution(),
                water: true,
            },
        );
        assert!(
            ordered.dot(into) > errand.dot(into),
            "an order was no bolder than an errand: {ordered:?} against {errand:?}"
        );
    }

    /// An ally that walks into deep water swims in it rather than trudging
    /// along the bottom.
    #[test]
    fn an_ally_out_of_its_depth_swims_at_the_surface() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(lawn_with_a_pond());
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        // Dropped into the middle of the pond, at the bottom of it, with its
        // plan pointing across -- so it is walking rather than standing, which
        // is the case that used to drag it along the floor.
        let under = Vec3::new(0.0, 0.0, 20.0);
        let ally = world
            .spawn((
                {
                    let mut ally = Ally::new(under, 0.0);
                    ally.plan = crate::goap::Goal::Fetch {
                        ball: Entity::from_raw_u32(99).unwrap(),
                        at: Vec2::new(0.0, 38.0),
                        arrive: 1.0,
                    };
                    ally
                },
                Transform::from_translation(under),
            ))
            .id();
        for _ in 0..90 {
            // The plan is written by hand here rather than by `goap::plan`,
            // which would clear it -- there is no ball in this world. So the
            // walk step is run on its own.
            world
                .run_system_once(move_allies)
                .expect("move_allies could not run");
        }
        let state = world.get::<Ally>(ally).unwrap();
        assert!(state.swimming, "it never noticed it was in the water");
        assert_eq!(
            state.state.motion,
            crate::player::Motion::Swim,
            "it is playing a walk cycle underwater"
        );
        let at = world.get::<Transform>(ally).unwrap().translation;
        // Riding the surface rather than the floor: the pond is three metres
        // deep and it floats just under the top of it.
        let depth = 3.0 - at.y;
        assert!(
            (depth - crate::player::SWIM_FLOAT_DEPTH).abs() < 0.4,
            "floating {depth} m down rather than {}",
            crate::player::SWIM_FLOAT_DEPTH
        );
        // And it did swim somewhere rather than treading water on the spot.
        assert!(at.z > under.z + 1.0, "it went nowhere: {at:?}");
    }

    /// Sends the squad in a test world, which has to reach the field and the
    /// squad at once. See [`Squad::send`], which needs the survey to know where
    /// there is anything to stand on.
    fn order(world: &mut World, to: Vec2) -> Landed {
        let aim = Vec3::new(to.x, 0.0, to.y);
        world.resource_scope(|world, mut squad: Mut<Squad>| {
            squad.send(world.resource::<crate::flow::FlowField>(), aim)
        })
    }

    /// One fixed tick of the squad, in the order the game runs it.
    ///
    /// `goap::plan` sits between the two, because that is where it sits in the
    /// real schedule and because a test that ran only the walk step would be
    /// exercising a Mario with no plan -- which stands still, whatever else is
    /// true about it. Deciding is its own system now; see [`crate::goap`].
    fn tick(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        // First, as in the real schedule: everything downstream navigates by
        // the sweep, and `update_goals` now asks it whether an order can be
        // carried out at all. A tick that left it out would be deciding against
        // a field that had never been swept. See [`Swept`].
        world
            .run_system_once(crate::flow::rebuild)
            .expect("flow::rebuild could not run");
        world
            .run_system_once(update_goals)
            .expect("update_goals could not run");
        world
            .run_system_once(crate::goap::plan)
            .expect("goap::plan could not run");
        // Between the decision and the walk, where it sits in the real
        // schedule. Most ticks it does nothing at all -- a Mario that can see
        // where it is going is not routed -- but it is in the list here for the
        // same reason `goap::plan` is: a test that left it out would be
        // exercising a walk step nothing had told which way to go.
        world
            .run_system_once(crate::path::plan)
            .expect("path::plan could not run");
        world
            .run_system_once(move_allies)
            .expect("move_allies could not run");
    }

    /// Everything a squad tick reads that is not the level or the tuning.
    fn squad_resources(world: &mut World) {
        world.insert_resource(Squad::default());
        world.insert_resource(Time::<Fixed>::from_hz(30.0));
        // The wall clock `flow::rebuild` counts its own interval against.
        world.init_resource::<Time>();
        // The navigation grid, surveyed off whatever level the caller has
        // already put in. `path::plan` reads it, and a survey of a hand-built
        // lawn is a few thousand floor queries against four triangles.
        let field = crate::flow::FlowField::new(world.resource::<LevelData>());
        world.insert_resource(field);
        world.init_resource::<crate::path::Pathing>();
        // The plan asks what the network is lit with, and the walk step makes a
        // splash when somebody goes in the water.
        world.init_resource::<crate::pylon::Network>();
        world.insert_resource(SoundQueue::default());
    }

    /// The follow behaviour end to end: recruit an ally, run the two fixed-step
    /// systems, and watch it walk to its slot behind the leader.
    #[test]
    fn a_recruited_ally_walks_to_its_slot_behind_the_leader() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let leader = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(leader)));
        // Standing well off to the side, with nothing to do.
        let start = leader + Vec3::new(9.0, 0.0, 0.0);
        let ally = world
            .spawn((Ally::new(start, 0.0), Transform::from_translation(start)))
            .id();

        // Unrecruited, it stays near where it was left rather than walking to
        // the leader.
        for _ in 0..30 {
            tick(&mut world);
        }
        let wandered = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            wandered.distance(start) < WANDER_RADIUS * 2.5,
            "an ally with no orders wandered off: {wandered:?}"
        );

        world.resource_mut::<Squad>().recruit(&[ally]);
        for _ in 0..90 {
            tick(&mut world);
        }
        let arrived = world.get::<Transform>(ally).unwrap().translation;
        let flat = Vec2::new(arrived.x - leader.x, arrived.z - leader.z).length();
        assert!(
            flat < FOLLOW_DISTANCE + FOLLOW_SPACING + FOLLOW_ARRIVE,
            "the ally never caught up: {flat} away"
        );
        // And it is standing on the ground rather than hovering over it.
        let level = world.resource::<LevelData>();
        let floor = level.floor_height(arrived + Vec3::Y * PLAYER_HEIGHT);
        assert!(
            floor.is_some_and(|height| (height - arrived.y).abs() < 0.2),
            "the ally is off the floor at {arrived:?}"
        );
    }

    /// A Mario with nothing to do walks somewhere, stands about, and walks
    /// somewhere else.
    ///
    /// What it must not do is change its mind every few ticks. Every change of
    /// clip restarts it, so an ally alternating between walking and standing
    /// three times a second never plays more than the opening frames of a
    /// step: a field of Marios stuck mid-stride, which is exactly what the
    /// orbiting drift target this replaced produced. The count is what the
    /// test is really about -- eyes on the game see the symptom, and this is
    /// the number underneath it.
    #[test]
    fn an_idle_ally_ambles_rather_than_twitching() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let start = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(start)));
        let ally = world
            .spawn((Ally::new(start, 0.0), Transform::from_translation(start)))
            .id();

        // Ten seconds of having nowhere to be.
        let mut clips = Vec::new();
        let mut walked = 0;
        for _ in 0..300 {
            tick(&mut world);
            let ally = world.get::<Ally>(ally).unwrap();
            if ally.state.motion == crate::player::Motion::Run {
                walked += 1;
            }
            let (clip, _) = crate::animation::resolve(ActiveCharacter::Mario, &ally.state);
            if clips.last() != Some(&clip) {
                clips.push(clip);
            }
        }
        assert!(
            clips.len() <= 8,
            "the clip changed {} times in ten seconds, so the walk never plays: {clips:?}",
            clips.len()
        );
        // And it is an amble rather than a statue: it does spend time walking,
        // and time standing, and neither swallows the other.
        assert!(
            (30..270).contains(&walked),
            "walked on {walked} of 300 ticks, which is not an amble"
        );
    }

    /// A lawn with a ten-metre moat cut across it, eight metres deep.
    ///
    /// **The bed matters more than the gap.** A hole with nothing in it is easy
    /// to refuse -- there is no ground, so every check refuses it. A moat has a
    /// floor, so a query dropped from the sky over the middle of it comes back
    /// with an answer, and every "is there ground here" test in the game says
    /// yes. That is the shape the castle actually has and the shape that got
    /// through. See [`cluster`].
    fn lawn_with_a_moat() -> LevelData {
        let vertices = vec![
            // The near bank.
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 0.),
            Vec3::new(-60., 0., 0.),
            // The bed of the moat, eight metres down.
            Vec3::new(-60., -8., 0.),
            Vec3::new(60., -8., 0.),
            Vec3::new(60., -8., 10.),
            Vec3::new(-60., -8., 10.),
            // And the far bank.
            Vec3::new(-60., 0., 10.),
            Vec3::new(60., 0., 10.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        LevelData::new(
            vertices,
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

    /// An order on the lip of a drop puts every Mario on ground it can stand on.
    ///
    /// **The bug this is about is the one that made the whole cluster a lie.**
    /// Eight slots were laid out by arithmetic alone -- the target plus a
    /// spiral offset -- so an order given on the edge of the moat sent four
    /// Marios to spots over open water. Each of those walked to the lip, leaned
    /// on it for six seconds and was written off, which on screen is half a
    /// squad obeying and half of it milling about on a bank.
    ///
    /// The test asserts the fix and, first, that the *obvious* fix would not
    /// have worked: the cells out over the moat are all "walkable" as the
    /// survey means it, because there is a bed under them. Filtering on that
    /// alone -- which is what the first attempt at this did -- leaves every one
    /// of those spots exactly where it was.
    #[test]
    fn a_cluster_on_the_lip_of_a_drop_keeps_every_mario_on_the_ground() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(lawn_with_a_moat());
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        // The leader on the near bank, which is what the sweep runs from and so
        // what decides which side of the moat counts as reachable.
        world.spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, 0.0, -6.0)),
        ));
        world
            .run_system_once(crate::flow::rebuild)
            .expect("flow::rebuild could not run");
        for raw in 0..8 {
            world
                .resource_mut::<Squad>()
                .members
                .push(Entity::from_raw_u32(raw).unwrap());
        }

        // A metre back from the lip -- near enough that the bare spiral would
        // throw most of the cluster over it.
        let aimed = Vec2::new(0.0, -1.0);
        let over: Vec<Vec2> = (0..8)
            .map(|index| aimed + slot(index, SEND_SPACING))
            .filter(|spot| spot.y > 0.0)
            .collect();
        assert!(
            !over.is_empty(),
            "this spot does not test anything: the plain spiral fits on the bank"
        );
        {
            let field = world.resource::<crate::flow::FlowField>();
            for spot in &over {
                assert!(
                    field
                        .survey_of(field.cell_at(Vec3::new(spot.x, 0.0, spot.y)))
                        .walkable,
                    "this level does not reproduce the bug: {spot:?} over the moat \
                     is not walkable, so the weak check would have caught it"
                );
            }
        }

        let landed = order(&mut world, aimed);
        assert_eq!(
            world.resource::<Squad>().sent.len(),
            8,
            "the order did not go out"
        );
        assert!(landed.at.z < 0.0, "the order landed at {:?}", landed.at);

        let field = world.resource::<crate::flow::FlowField>();
        for order in &world.resource::<Squad>().sent {
            let at = Vec3::new(order.at.x, 0.0, order.at.y);
            let cell = field.cell_at(at);
            assert!(
                field.survey_of(cell).walkable,
                "a Mario was sent to stand on nothing at {:?}",
                order.at
            );
            // The one that matters: the ground under the spot is the bank the
            // order was given on, not the bed of the moat eight metres down.
            assert!(
                field.centre_of(cell).y > -1.0,
                "a Mario was sent to stand {:.1} m down in the moat, at {:?}",
                -field.centre_of(cell).y,
                order.at
            );
            assert!(order.at.y < 0.5, "over the lip at {:?}", order.at);
        }
    }

    /// A second order redirects the squad without whistling it up again.
    ///
    /// **A tap is the only thing an order needs to be.** `send` used to drain
    /// the followers into the sent list and read nothing else, so the first tap
    /// emptied the squad and every tap after it commanded nobody -- to redirect
    /// a group already on the march the player had to hold the button, gather
    /// them all back up, and only then point somewhere new. Marching is not
    /// leaving the squad; only a whistle and a disband move anybody in or out.
    #[test]
    fn a_second_order_redirects_the_squad_it_already_sent() {
        let field = crate::flow::FlowField::new(&lawn());
        let mut squad = Squad::default();
        let marios: Vec<Entity> = (0..5)
            .map(|raw| Entity::from_raw_u32(raw).unwrap())
            .collect();
        squad.members.extend(marios.iter().copied());

        squad.send(&field, Vec3::new(10.0, 0.0, 10.0));
        assert_eq!(squad.sent.len(), 5);
        // One of them has already arrived and is standing its post, which must
        // not exempt it from being sent somewhere else.
        squad.sent[0].arrived = true;

        let again = Vec3::new(-14.0, 0.0, -6.0);
        let landed = squad.send(&field, again);
        assert_eq!(squad.sent.len(), 5, "the second order lost Marios");
        assert!(squad.members.is_empty());
        let mut who: Vec<Entity> = squad.sent.iter().map(|order| order.who).collect();
        let mut expected = marios.clone();
        who.sort();
        expected.sort();
        assert_eq!(who, expected, "the second order went to a different squad");
        for order in &squad.sent {
            assert!(!order.arrived, "an order arrived before it was given");
            assert!(
                order.at.distance(Vec2::new(again.x, again.z)) <= landed.radius,
                "a Mario was left at the old spot: {:?}",
                order.at
            );
        }
    }

    /// A Mario sent somewhere it cannot get to stops treating that as the only
    /// thing in the world.
    ///
    /// **This is the other half of "they get there and just stand about", and
    /// it is not a scoring bug -- the Mario really has not arrived.** A cluster
    /// is spread over several metres of whatever ground is under it, so some of
    /// its slots land behind railings, inside the castle wall, or on the far
    /// bank. An order is discharged by arrival and by nothing else, so those
    /// Marios lean on the fence at full [`crate::goap::Goal::Obey`] for the rest
    /// of the session -- deaf to the slime beside them, because obeying at a
    /// few metres' range outbids a fight at any range worth mentioning.
    ///
    /// Giving up is what makes it a squad again: the order retires to a post,
    /// and a post is weak enough that anything real outbids it.
    #[test]
    fn an_order_that_cannot_be_carried_out_is_eventually_given_up_on() {
        let mut world = World::new();
        // A lawn with a wall straight across it, and no way round: the far side
        // is reachable only by walking through five metres of stone.
        let lawn = [
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
            Vec3::new(-60., 0., 0.),
            Vec3::new(60., 0., 0.),
            Vec3::new(60., 5., 0.),
            Vec3::new(-60., 5., 0.),
        ];
        world.insert_resource(LevelData::new(
            lawn.to_vec(),
            vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
            Vec::new(),
        ));
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let leader = Vec3::new(0.0, 0.0, -10.0);
        world.spawn((Player, Transform::from_translation(leader)));
        let ally = world
            .spawn((Ally::new(leader, 0.0), Transform::from_translation(leader)))
            .id();
        world.resource_mut::<Squad>().recruit(&[ally]);
        // Sent to the far side of the wall.
        order(&mut world, Vec2::new(0.0, 10.0));

        // **Long enough to give up on it, and no longer.** The stall clock is
        // six seconds and this budget is under two, so passing this means the
        // order was retired by the pathfinder having refused it outright --
        // [`LOST_SECONDS`] -- rather than by a Mario leaning on the wall until
        // [`STUCK_SECONDS`] ran out. See [`update_goals`].
        let ticks = ((LOST_SECONDS + 0.6) * 30.0) as usize;
        assert!(
            ticks < (STUCK_SECONDS * 30.0) as usize,
            "the budget is long enough for the stall clock to be what retired it"
        );
        for _ in 0..ticks {
            tick(&mut world);
        }
        let here = world.get::<Transform>(ally).unwrap().translation;
        assert!(here.z < 0.0, "it walked through the wall: {here:?}");
        let squad = world.resource::<Squad>();
        assert_eq!(
            squad.marching(),
            0,
            "still marching at a wall it cannot pass"
        );
        // And what it is doing now is standing a post, which is a thing a
        // fight, a ball or a whistle can all take it away from.
        assert!(
            matches!(
                world.get::<Ally>(ally).unwrap().plan,
                crate::goap::Goal::Hold { .. }
            ),
            "the order never retired: {:?}",
            world.get::<Ally>(ally).unwrap().plan
        );
    }

    /// A lawn with a three-sided pocket cut into it, opening *away* from
    /// everything.
    ///
    /// Six metres of wall on three sides and a mouth at the back. Standing
    /// inside it, the way out is a hundred and eighty degrees from wherever you
    /// are going -- which is the one thing no steering rule in this game can
    /// ever produce, because [`steer`] deflects to a right angle and stops. It
    /// is the shape of every "it walked into a corner and stayed there": a
    /// courtyard, the inside of a castle wall, a bay of the moat.
    fn lawn_with_a_pocket() -> LevelData {
        let mut vertices = vec![
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 60.),
            Vec3::new(-60., 0., 60.),
        ];
        let mut indices = vec![[0u32, 1, 2], [0, 2, 3]];
        {
            let mut wall = |a: Vec3, b: Vec3| {
                let base = vertices.len() as u32;
                vertices.extend([a, b, b + Vec3::Y * 6.0, a + Vec3::Y * 6.0]);
                indices.push([base, base + 1, base + 2]);
                indices.push([base, base + 2, base + 3]);
            };
            // The back of the pocket, standing between the Mario and the whole
            // of the rest of the lawn.
            wall(Vec3::new(-16., 0., -10.), Vec3::new(16., 0., -10.));
            // And its two sides.
            wall(Vec3::new(-16., 0., -32.), Vec3::new(-16., 0., -10.));
            wall(Vec3::new(16., 0., -32.), Vec3::new(16., 0., -10.));
        }
        LevelData::new(vertices, indices, Vec::new())
    }

    /// A lawn in two levels with a knee-high lip between them, and one gentle
    /// ramp off to one side.
    ///
    /// The lip is seventy centimetres over forty -- sixty degrees, steeper than
    /// [`crate::level::GROUND_NORMAL_Y`] allows ground to be, so `resolve_walls`
    /// holds a body out of it. **And it is invisible to every test that looks at
    /// the two ends of a step.** The cells either side are good flat lawn, the
    /// rise between their middles is well inside the grid's own climb limit, and
    /// a knee-height ray following the ground sails clean over the top of it.
    /// Which is the shape of the whole complaint: ordered across it, a Mario
    /// walks up to the lip and stays there.
    fn lawn_with_a_bank() -> LevelData {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut slab = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let base = vertices.len() as u32;
            vertices.extend([a, b, c, d]);
            indices.push([base, base + 1, base + 2]);
            indices.push([base, base + 2, base + 3]);
        };
        // The lower lawn, all of it.
        slab(
            Vec3::new(-60., 0., -60.),
            Vec3::new(60., 0., -60.),
            Vec3::new(60., 0., 0.),
            Vec3::new(-60., 0., 0.),
        );
        // The lip, across everything but the eastern strip.
        slab(
            Vec3::new(-60., 0., 0.),
            Vec3::new(20., 0., 0.),
            Vec3::new(20., 0.7, 0.4),
            Vec3::new(-60., 0.7, 0.4),
        );
        // And the ramp that takes its place there: the same climb over ten
        // times the run.
        slab(
            Vec3::new(20., 0., 0.),
            Vec3::new(60., 0., 0.),
            Vec3::new(60., 0.7, 4.),
            Vec3::new(20., 0.7, 4.),
        );
        // The upper lawn, in two pieces because the ramp reaches further into
        // it than the lip does.
        slab(
            Vec3::new(-60., 0.7, 0.4),
            Vec3::new(20., 0.7, 0.4),
            Vec3::new(20., 0.7, 60.),
            Vec3::new(-60., 0.7, 60.),
        );
        slab(
            Vec3::new(20., 0.7, 4.),
            Vec3::new(60., 0.7, 4.),
            Vec3::new(60., 0.7, 60.),
            Vec3::new(20., 0.7, 60.),
        );
        LevelData::new(vertices, indices, Vec::new())
    }

    /// **The complaint, staged.** Ordered somewhere on the far side of a bank a
    /// Mario cannot climb, it walks round by the ramp instead of standing at the
    /// foot of the bank.
    ///
    /// Both tiers had to learn the same thing for this to work, and either one
    /// alone leaves the Mario stuck. The survey has to know the lip is there or
    /// the route leads straight over it; the walk step has to know or it steers
    /// into it anyway on the last stride. Neither could see it while they
    /// measured the two ends of a step and called the average a slope -- see
    /// [`LevelData::climbable`], which is that question asked of the ground in
    /// between.
    #[test]
    fn a_mario_ordered_over_a_bank_it_cannot_climb_walks_round_by_the_ramp() {
        let mut world = World::new();
        world.insert_resource(lawn_with_a_bank());
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let here = Vec3::new(0.0, 0.0, -10.0);
        world.spawn((Player, Transform::from_translation(here)));
        let ally = world
            .spawn((Ally::new(here, 0.0), Transform::from_translation(here)))
            .id();
        world.resource_mut::<Squad>().recruit(&[ally]);
        // Straight over the lip, twenty metres away, with the only way up
        // twenty metres off to the east.
        let to = Vec2::new(0.0, 10.0);
        order(&mut world, to);
        for _ in 0..900 {
            tick(&mut world);
        }
        let at = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(at.x, at.z).distance(to) <= SEND_ARRIVE + 1.0,
            "it stopped {:.1} m short, at {at:?}",
            Vec2::new(at.x, at.z).distance(to)
        );
        // And it is up on the top lawn rather than having found some way
        // through the bank itself.
        assert!(
            (at.y - 0.7).abs() < 0.2,
            "it is at {at:?}, not on the top lawn"
        );
    }

    /// **The whole case for routing, staged as a pair of runs.** The same
    /// Mario, the same order, the same lawn: with the search allowed it walks
    /// out of the pocket and round to the spot, and with the search switched
    /// off at the console it walks into the back wall and stays there.
    ///
    /// The two halves are one test rather than two because either alone proves
    /// nothing. "It arrived" is satisfied by an order that was never obstructed;
    /// "it did not arrive" is satisfied by a Mario that cannot walk at all. Run
    /// against the same staging, what is left between them is the routing.
    ///
    /// Staged on a hand-built lawn rather than on the castle, and that is not
    /// convenience. A route over the castle crosses ground the two tiers
    /// disagree about -- the navigation grid refuses a steeper step than the
    /// walk step does, and its cells are wider than a body -- so a castle
    /// staging tests the survey's fidelity as much as the search, and fails for
    /// reasons that have nothing to do with what it is asking. Here the walls
    /// are walls to both tiers by construction.
    #[test]
    fn a_mario_sent_out_of_a_pocket_walks_the_wrong_way_first() {
        let from = Vec3::new(0.0, 0.0, -20.0);
        let to = Vec2::new(0.0, 40.0);

        // Sent there twice: once with the routing on, once with it off.
        let walked = |budget: f32| -> (f32, f32) {
            let mut world = World::new();
            world.insert_resource(lawn_with_a_pocket());
            world.insert_resource(GameTuning::default());
            squad_resources(&mut world);
            world.resource_mut::<GameTuning>().path_budget = budget;
            world.spawn((Player, Transform::from_translation(from)));
            let ally = world
                .spawn((Ally::new(from, 0.0), Transform::from_translation(from)))
                .id();
            world.resource_mut::<Squad>().recruit(&[ally]);
            order(&mut world, to);
            // Long enough to walk out of the pocket, round the wall and back up
            // the lawn at the squad's marching pace, twice over.
            let mut behind = 0.0_f32;
            for t in 0..1200 {
                tick(&mut world);
                if t % 200 == 0 {
                    let at = world.get::<Transform>(ally).unwrap().translation;
                    let r = world.get::<crate::path::Route>(ally).unwrap();
                    println!(
                        "  b{budget} t{t} {at:?} legs {:?} lost {}",
                        r.legs(),
                        r.lost()
                    );
                }
                let at = world.get::<Transform>(ally).unwrap().translation;
                // How far *back* into the pocket it ever went, which is the
                // thing being tested: leaving means going the wrong way first.
                behind = behind.max(from.z - at.z);
            }
            let here = world.get::<Transform>(ally).unwrap().translation;
            (Vec2::new(here.x, here.z).distance(to), behind)
        };

        let (routed, backwards) = walked(4.0);
        let (beeline, _) = walked(0.0);
        assert!(
            routed <= SEND_ARRIVE + 1.0,
            "routed, it still stopped {routed:.1} m short of the spot"
        );
        assert!(
            backwards > 8.0,
            "it never walked away from the spot, so it never left the pocket"
        );
        assert!(
            beeline > 20.0,
            "the straight line got within {beeline:.1} m of a spot it has no way \
             of reaching, so the pocket is not a pocket"
        );
    }

    /// **The screenshot this was written for.** A ball over the fence, and one
    /// twice as far away on the same lawn: the squad goes for the one it can
    /// walk to.
    ///
    /// Scored on how far off a thing is, the near one wins and the Marios walk
    /// up to the railings and press against them -- which is the right answer to
    /// that question, because the far side of a fence really is eight metres
    /// away. It is not *closer*. See [`crate::goap::Option_::blocked`], and this
    /// is that scoring wired to a real grid rather than to a hand-made struct.
    #[test]
    fn a_mario_fetches_the_ball_it_can_walk_to_rather_than_the_near_one_over_a_fence() {
        let mut world = World::new();
        // A lawn cut in two by a fence, with a pond on the far side of it.
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
                Vec3::new(-60., 0., 6.),
                Vec3::new(60., 0., 6.),
                Vec3::new(60., 6., 6.),
                Vec3::new(-60., 6., 6.),
            ]);
            indices.push([base, base + 1, base + 2]);
            indices.push([base, base + 2, base + 3]);
        }
        world.insert_resource(LevelData::new(
            vertices,
            indices,
            // Well past the ball below, so that what separates the two is
            // which side of the fence they are on and nothing else. A wet ball
            // is already discounted by `WET_PENALTY` and would answer this test
            // without `blocked` doing anything at all.
            vec![crate::level::WaterBox {
                min_x: -60.,
                min_z: 20.,
                max_x: 60.,
                max_z: 60.,
                surface_y: 3.0,
            }],
        ));
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let here = Vec3::new(0.0, 0.0, -2.0);
        world.spawn((Player, Transform::from_translation(here)));
        let ally = world
            .spawn((Ally::new(here, 0.0), Transform::from_translation(here)))
            .id();
        // One ten metres off on dry ground over the fence, one twenty metres off
        // on the Mario's own side. Both perfectly good balls; only one of them
        // is a walk.
        let fenced = world
            .spawn((
                crate::nuclonium::Nuclonium {
                    held: crate::nuclonium::Held::Loose { claimed: None },
                },
                Transform::from_translation(Vec3::new(0.0, 0.0, 8.0)),
            ))
            .id();
        let dry = world
            .spawn((
                crate::nuclonium::Nuclonium {
                    held: crate::nuclonium::Held::Loose { claimed: None },
                },
                Transform::from_translation(Vec3::new(0.0, 0.0, -22.0)),
            ))
            .id();
        tick(&mut world);
        assert_eq!(
            world.get::<Ally>(ally).unwrap().plan.claim(),
            Some(dry),
            "it went for the one over the fence: {:?}",
            world.get::<Ally>(ally).unwrap().plan
        );
        // And it walks away from the fence rather than into it.
        for _ in 0..150 {
            tick(&mut world);
        }
        let at = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            at.z < here.z - 5.0,
            "it is still on the fence at {at:?}, {fenced:?} won"
        );
    }

    /// Sent somewhere, an ally holds the spot rather than drifting off it.
    #[test]
    fn a_sent_ally_arrives_and_holds_its_ground() {
        let mut world = World::new();
        let (collision, _) = crate::level::load();
        world.insert_resource(collision);
        world.insert_resource(GameTuning::default());
        squad_resources(&mut world);
        let leader = Vec3::new(-13.28, 3.0, 46.64);
        world.spawn((Player, Transform::from_translation(leader)));
        let ally = world
            .spawn((Ally::new(leader, 0.0), Transform::from_translation(leader)))
            .id();
        world.resource_mut::<Squad>().recruit(&[ally]);
        // Somewhere across the lawn, still on the castle grounds.
        let target = Vec2::new(leader.x + 8.0, leader.z - 6.0);
        order(&mut world, target);

        for _ in 0..150 {
            tick(&mut world);
        }
        let squad = world.resource::<Squad>();
        assert_eq!(squad.marching(), 0, "never reported arriving");
        let here = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(here.x, here.z).distance(target) <= SEND_ARRIVE + 0.2,
            "stopped {:?} short of the spot",
            Vec2::new(here.x, here.z).distance(target)
        );
        // Held: an order is not a suggestion, so it does not wander home.
        for _ in 0..90 {
            tick(&mut world);
        }
        let later = world.get::<Transform>(ally).unwrap().translation;
        assert!(
            Vec2::new(later.x, later.z).distance(target) <= SEND_ARRIVE + 0.2,
            "the ally wandered off the spot it was sent to"
        );
    }
}
