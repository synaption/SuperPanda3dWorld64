//! What a Mario decides to do with itself.
//!
//! Every ally in the field wakes up each tick with four or five things it could
//! be doing -- obey the order it was given, close on what it has noticed, walk
//! over to a resource ball, carry the one it is holding to a mast, or amble --
//! and this module is the part that picks one. `next.md` has asked for goal
//! oriented action planning for a while; this is the shape of it that the game
//! actually needs today, which is the *choosing* rather than a multi-step
//! planner.
//!
//! **It replaces a priority chain, and the chain is worth describing because
//! its failure is the reason this exists.** The rule was: a fight beats an
//! order, an order beats an errand, an errand beats standing about. Each line
//! reads sensibly and the whole thing does not work, because
//! [`crate::enemy::alert`] never lets a creature *stop* having a target -- aggro
//! is given up only by the target dying. So in any level with enemies standing
//! about, every Mario is permanently in the top bracket and nothing below it is
//! ever reached. A squad that would happily fetch a ball on an empty lawn
//! fetched nothing at all on a populated one, and no amount of reordering the
//! chain fixes that: the problem is that a chain compares *kinds* of thing,
//! when what actually decides this is how far away each one is.
//!
//! So every option is scored instead, and the score is one function of two
//! numbers -- what the option is worth at arm's length, and how far off it is.
//! See [`appeal`]. That gives the behaviour the chain was reaching for and the
//! behaviour it could not express, out of the same three lines:
//!
//!   * A slime standing on a Mario outranks everything. Fighting is worth the
//!     most up close and falls away fastest, so anything actually threatening
//!     it wins.
//!   * A slime forty metres away outranks nothing. The same fight, scored at
//!     range, loses to a ball at the Mario's feet -- and to the order it was
//!     given, which is what makes an order an order.
//!   * A slime *between* a Mario and where it was sent is fought, whatever the
//!     numbers above say about forty metres, because the distance a fight is
//!     scored at is how far out of the way it is rather than how far off. A
//!     squad marching across the map deals with what it walks into and ignores
//!     what it walks past. See [`detour`], which is one subtraction and is the
//!     whole of that behaviour.
//!   * A Mario with nothing else to do will cross a lawn for a ball, because
//!     ambling scores a flat and very small amount and any real option beats
//!     it at any distance.
//!
//! **One rule sits above the scoring: a Mario in a fight does not haul.** Not
//! "fighting scores higher" -- fetching and delivering are struck off the list
//! outright while something it can see is coming for it. A score, however
//! large, is still a distance at which a ball outbids a slime, and a Mario that
//! turns its back on the thing hitting it to go and pick something up is wrong
//! at every one of those distances. The line between a fight a Mario is *in*
//! and a grudge it is merely carrying is drawn at sight range; see [`engaged`],
//! which is also the reason this can be a hard rule without re-creating the
//! chain's failure.
//!
//! **Water is a cost, not a wall.** A ball that rolled into the moat is still
//! worth having; it is just worth less than the same ball on grass, by
//! [`WET_PENALTY`]. That is the whole of "Marios should avoid water" at this
//! layer -- they prefer dry work when there is dry work -- and the steering half
//! of it, going *round* a pond on the way to something past it, is
//! [`crate::squad::steer`], which asks [`Goal::caution`] how much this
//! particular job minds getting wet.
//!
//! Everything here is arithmetic over a small struct, so the decisions can be
//! exercised without a level, a renderer or an ECS -- see the tests, which are
//! the specification.

use bevy::prelude::*;

use crate::{level::LevelData, nuclonium, pylon::Network, squad::Ally};

/// What a Mario settled on this tick.
///
/// Carries where to walk rather than what to walk to, because the walk step has
/// no business asking what a ball is: by the time this is written the decision
/// is made and all that is left is a point on the ground.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Goal {
    /// Nothing worth doing. Amble around where it was left.
    #[default]
    Idle,
    /// Walk to where the player sent it.
    Obey { at: Vec2, arrive: f32 },
    /// Stand about the spot it was sent to, having got there.
    ///
    /// A different thing from [`Goal::Obey`] and not a shade of it: an order is
    /// a journey and this is what is left when the journey is over. See
    /// [`HOLD_APPEAL`] for why the two had to stop being scored as one, which
    /// is the whole of "a squad sent to attack something arrives and then
    /// stands there".
    Hold { at: Vec2, arrive: f32 },
    /// Close on what it has noticed, and stop at that thing's edge.
    Fight { at: Vec2, arrive: f32 },
    /// Go and pick a ball up.
    Fetch { ball: Entity, at: Vec2, arrive: f32 },
    /// Carry the one it is holding to a mast.
    Deliver { at: Vec2, arrive: f32 },
}

impl Goal {
    /// Where to walk and how near counts as arrived, or `None` for ambling.
    pub fn destination(self) -> Option<(Vec2, f32)> {
        match self {
            Goal::Idle => None,
            Goal::Obey { at, arrive }
            | Goal::Hold { at, arrive }
            | Goal::Fight { at, arrive }
            | Goal::Fetch { at, arrive, .. }
            | Goal::Deliver { at, arrive } => Some((at, arrive)),
        }
    }

    /// Whether this is worth walking at the squad's marching pace rather than
    /// strolling. Everything the Mario is on its way to do is; standing about
    /// somewhere, whether that is the spot it was sent to or the patch of lawn
    /// it was left on, is not.
    pub fn urgent(self) -> bool {
        !matches!(self, Goal::Idle | Goal::Hold { .. })
    }

    /// How much this Mario minds what is between it and where it is going.
    ///
    /// The multiplier [`crate::squad::steer`] puts on every hazard it finds --
    /// deep water, a drop, a wall -- against the cost of not walking straight
    /// at the thing. It is what makes the rule *"prefer not to, rather than
    /// never"*: nothing here is zero, so a Mario always takes the dry way round
    /// when there is one, and nothing here is infinite, so a Mario with an
    /// order and no dry way round wades in.
    ///
    /// The order is the design. **An order is the most daring thing a Mario
    /// does** -- told to be somewhere, it swims the moat rather than pacing the
    /// bank, because a squad that would not is a squad that cannot be sent
    /// anywhere across water. A fight it has chosen for itself is next, then an
    /// errand, and a Mario with nothing to do is the most careful of all: there
    /// is no reason on earth for an ambling Mario to paddle anywhere, and an
    /// idle crowd wandering into the sea is what the caution reads as from
    /// outside.
    /// The scale is set against the detour it is being weighed against, which
    /// is `1 - cos(turn)` and so runs from nothing to one over a right angle.
    /// Read the numbers as "how far out of its way this Mario will go": an
    /// order swings about seventy degrees to keep out of the moat and nearly a
    /// right angle to stay off a cliff, and past that it wades or climbs down,
    /// because a squad that will not is a squad that cannot be sent across
    /// water. An ambling Mario, at nearly three times an order's caution, never
    /// gets its feet wet while there is any dry way at all.
    pub fn caution(self) -> f32 {
        match self {
            Goal::Obey { .. } => 0.7,
            Goal::Fight { .. } => 0.9,
            Goal::Deliver { .. } => 1.0,
            Goal::Fetch { .. } => 1.2,
            Goal::Hold { .. } => 1.5,
            Goal::Idle => 2.0,
        }
    }

    /// Which ball this Mario is coming for, if any, so [`crate::nuclonium`] can hold
    /// it for them.
    pub fn claim(self) -> Option<Entity> {
        match self {
            Goal::Fetch { ball, .. } => Some(ball),
            _ => None,
        }
    }
}

/// One thing a Mario could do, as the score needs to see it.
#[derive(Clone, Copy, Debug)]
pub struct Option_ {
    pub at: Vec2,
    pub arrive: f32,
    /// How far off it is, in metres.
    pub range: f32,
    /// Whether getting there means being in deep water.
    pub wet: bool,
    /// Whether there is something in the way of the straight line to it.
    ///
    /// **[`Self::range`] is how far off a thing is as the crow flies, and a
    /// Mario is not a crow.** That gap is a whole class of behaviour: a ball
    /// eight metres away through a fence and across the moat outscores one
    /// twenty metres away on the same lawn, so the squad walks up to the
    /// railings and presses against them -- which is exactly what it should do,
    /// given a score that says the thing behind the fence is nearer. It is
    /// nearer. It is not *closer*.
    ///
    /// Answered by [`crate::flow::FlowField::clear`], which is three array
    /// reads a sample and is the same question [`crate::path`] asks before it
    /// decides whether a route is worth working out. A real path length would be
    /// better and is not affordable per option per body per tick; what this
    /// costs is one grid walk for the candidates that could still win.
    pub blocked: bool,
}

/// Everything one Mario knows about what it could be doing.
///
/// A plain struct rather than a set of queries, so [`choose`] is a function of
/// its arguments and can be written down in a test as a sentence about play.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Where the player *sent* it, with the whistle. An order.
    pub ordered: Option<Option_>,
    /// Where its slot in the marching formation is. Not an order -- see
    /// [`FOLLOW_APPEAL`], and the paragraph above it, which is the difference
    /// between a squad that collects things and one that never has.
    pub following: Option<Option_>,
    /// The spot it was sent to and has already reached. Not an order either,
    /// for the same reason and with worse symptoms -- see [`HOLD_APPEAL`].
    pub holding: Option<Option_>,
    /// What it has noticed, and the edge of it.
    pub quarry: Option<Option_>,
    /// The nearest ball it is allowed to go for, and which one.
    pub ball: Option<(Entity, Option_)>,
    /// The nearest live mast, offered only while it is carrying something.
    pub mast: Option<Option_>,
    /// How near [`Self::quarry`] has to be for the Mario to count as *in* the
    /// fight rather than merely aware of one. See [`choose`].
    ///
    /// Zero by default, which is a Mario that is never engaged and therefore
    /// scores every option against every other -- the behaviour this module
    /// shipped with. [`plan`] fills it from `enemy_sight`.
    pub engage: f32,
}

/// What each kind of job is worth to a Mario standing on top of it, and over
/// what distance that is worth halves.
///
/// The pairs are the entire design and every one of them is a sentence:
///
///   * **Fighting** is worth the most and reaches the least far. A creature in
///     front of a Mario is the most pressing thing in the world; the same
///     creature across the lawn is somebody else's problem. The short scale is
///     what stops a permanent aggro target -- which is what every Mario has,
///     see this module's preamble -- from monopolising the whole squad forever.
///     For a Mario under orders, read "away" as [`detour`] does: away from the
///     line it is walking, not away from the Mario.
///   * **An order** is worth almost as much and reaches furthest of anything.
///     That is what makes it an order: told to stand somewhere fifty metres
///     off, a Mario goes, and only something with its hands on it stops them.
///   * **Keeping formation** is worth a fraction of an order, and that gap is
///     load-bearing. See [`FOLLOW_APPEAL`].
///   * **Delivering** is worth as much as fighting and reaches nearly as far as
///     an order. A Mario carrying something takes it home; it is already
///     holding the thing, and a squad that dropped the job every time a slime
///     wandered past would never bank anything.
///   * **Fetching** is worth less than a fight or an order and reaches a very
///     long way -- further than anything else here except an order. Collecting
///     is the squad's standing job rather than an interruption to it, and the
///     scale is what makes a Mario cross most of a field for a ball instead of
///     trailing the player past one.
///   * **Ambling** is a flat floor with no distance in it at all, set below
///     what any real option scores at any range this game contains. A Mario
///     with something to do never stands about.
const FIGHT_APPEAL: f32 = 1.00;
const FIGHT_SCALE: f32 = 7.0;
const OBEY_APPEAL: f32 = 0.90;
const OBEY_SCALE: f32 = 60.0;
const DELIVER_APPEAL: f32 = 1.00;
const DELIVER_SCALE: f32 = 45.0;
const FETCH_APPEAL: f32 = 0.75;
const FETCH_SCALE: f32 = 40.0;
const IDLE_APPEAL: f32 = 0.08;

/// What walking back to your slot in the formation is worth.
///
/// **A formation slot is not an order, and scoring it as one is what made the
/// squad look like it would only pick up what was lying next to a pylon.** An
/// ally following the player is by definition standing near its slot, so an
/// order-strength follow scores about 0.89 every tick, for ever -- and fetching
/// tops out at [`FETCH_APPEAL`] with the Mario stood on the ball. A Mario
/// holding formation could therefore never fetch anything at any range, and the
/// only ones that ever did were the ones a fight had knocked far enough out of
/// position for the follow to have decayed. Which looks, from outside, exactly
/// like "they only collect near the places the fighting happens".
///
/// So following is a *weak, long* pull: barely a third of an order up close, so
/// nearly any real job outbids it, but falling away slowly enough that a Mario
/// left forty metres behind still comes rather than ambling where it stands.
/// An order the player actually gave keeps the full [`OBEY_APPEAL`]; the two
/// are different things and are now scored as different things.
const FOLLOW_APPEAL: f32 = 0.35;
const FOLLOW_SCALE: f32 = 60.0;

/// What standing on the spot you were already sent to is worth.
///
/// **An order that has been carried out is not still an order, and scoring it
/// as one is why a squad sent to attack something walked over and then stood
/// there.** Work it through with [`OBEY_APPEAL`]: a Mario on its ordered spot
/// is at zero range from it, so obeying scores the full 0.90 every tick, for
/// ever. A slime three metres away scores `FIGHT_APPEAL * FIGHT_SCALE / (FIGHT_SCALE + 3)`,
/// which is 0.70. The order wins. It goes on winning against anything further
/// off than about eighty centimetres -- which is *inside* the reach the punch
/// already has, so the only Marios that ever swung were the ones whose slot
/// happened to land on top of something. The rest stood on their spots in the
/// middle of a fight, looking broken, because they were doing exactly as they
/// were told.
///
/// So arriving retires the order into a station: the same point, wanted the
/// same way a formation slot is wanted. Weak enough that anything real outbids
/// it -- a slime within about thirteen metres, which is [`crate::console::GameTuning::enemy_sight`]
/// and therefore everything the Mario can see -- and long enough that a Mario
/// shoved out of a fight walks back to its post rather than ambling off from
/// wherever the fight left it. The player's order still decides *where* the
/// squad is; it stops deciding what they may do once they are there.
///
/// The same numbers as [`FOLLOW_APPEAL`], because it is the same sentence about
/// a different point. Kept as its own pair anyway: holding a line and trailing
/// a leader are different orders from the player and will want to be tuned
/// apart.
const HOLD_APPEAL: f32 = 0.35;
const HOLD_SCALE: f32 = 60.0;

/// Whether a Mario is *in* a fight, as opposed to holding a grudge against
/// something across the valley.
///
/// The distinction is the whole of "aggro takes precedence", and it exists
/// because [`crate::enemy::alert`] never lets a creature stop having a target.
/// Without a line drawn somewhere, "fighting always wins" and "the squad never
/// collects anything again" are the same sentence -- that is exactly what the
/// priority chain this module replaced did.
///
/// So the line is drawn at sight: a quarry near enough to be *seen* is a fight
/// the Mario is in, and a quarry further off than it could have noticed one in
/// the first place is a memory of where something was. [`plan`] passes
/// `enemy_sight` in rather than a constant here, so the two agree by
/// construction and the slider moves both.
fn engaged(options: &Options) -> bool {
    options
        .quarry
        .is_some_and(|quarry| quarry.range <= options.engage)
}

/// What being in deep water does to the appeal of a job.
///
/// A discount rather than a refusal, which is the difference between "Marios
/// should avoid water" and "Marios cannot swim". Given two balls the squad takes
/// the dry one; given one ball in the moat and nothing else to do, somebody
/// swims out for it.
///
/// A quarter rather than the third it was, because it is tied to
/// [`FETCH_SCALE`] and that got much longer. The number that matters is where a
/// wet ball falls below [`IDLE_APPEAL`] -- past that distance nobody swims for
/// it at all -- and with the longer reach a third put that crossover outside
/// [`FETCH_RANGE`] entirely, which is a squad that would cross the whole map
/// underwater for one ball. At a quarter it lands around fifty metres, well
/// inside the sweep, and the test below pins it.
pub const WET_PENALTY: f32 = 0.25;

/// What being round the far side of something does to the appeal of a job.
///
/// The same shape as [`WET_PENALTY`] and for the same reason: a discount rather
/// than a refusal. A ball behind a fence is still worth having -- somebody walks
/// round for it once there is nothing better on this side -- it is just worth a
/// quarter of the same ball in plain sight, because the walk is the long way
/// round and the score has no other way of knowing that.
///
/// A quarter is not a measurement of anything. What it is set against is the
/// case in the screenshot this exists for: a ball across the moat at eight
/// metres against a ball on the lawn at twenty. Fetching is hyperbolic in range,
/// so the near one scores 0.63 and the far one 0.50 -- and a quarter of 0.63 is
/// 0.16, which loses comfortably. Anything much gentler leaves the squad
/// walking into fences; anything much harsher and a Mario will not step round a
/// tree for something.
pub const BLOCKED_PENALTY: f32 = 0.25;

/// How far a Mario will consider going for a ball at all.
///
/// Deliberately further than the range at which fetching actually wins anything,
/// so that **the score decides and this does not**. It is a sweep limit -- it
/// stops a Mario setting off across a planet for something it will never reach,
/// and stops the per-ally loop looking at every ball on the map -- and if it
/// ever becomes the thing that decides whether a ball gets collected, it is set
/// too low. That was the old design's mistake twice over.
pub const FETCH_RANGE: f32 = 90.0;

/// What an option that far away is worth.
///
/// Hyperbolic rather than linear or a threshold, and each of those matters. It
/// is `base` at zero range and half of `base` at `scale`, falls away smoothly
/// for ever and never reaches zero -- so a distant thing is always worth
/// *something*, which is what lets an idle Mario walk a long way for a ball, and
/// there is no cliff for a target to jitter across.
pub fn appeal(base: f32, range: f32, scale: f32) -> f32 {
    base * scale / (scale + range.max(0.0))
}

/// How far a fight is out of a Mario's way, rather than how far off it is.
///
/// **A squad marching across the map should deal with what it walks into.**
/// That was not what it did, and the reason is worth writing down because the
/// numbers say it plainly. An order is worth [`OBEY_APPEAL`] over
/// [`OBEY_SCALE`], which is 0.60 at thirty metres out; a fight is worth
/// [`FIGHT_APPEAL`] over [`FIGHT_SCALE`], which is 0.60 at *four and a half*.
/// So a Mario with somewhere to be threw a punch only at something already
/// close enough to hit it, and walked past everything else -- including the
/// slime standing squarely between it and the spot it was sent to, which it
/// then squeezed round.
///
/// The mistake is measuring the fight the same way from every direction. What
/// a fight actually costs a Mario that is going somewhere is not the walk to
/// the creature, it is the walk to the creature *and on to where it was going*
/// less the walk it was making anyway -- the extra metres, which is what
/// "out of its way" means and is one subtraction:
///
/// ```text
/// detour = |here -> foe| + |foe -> spot| - |here -> spot|
/// ```
///
/// Every case falls out of that one line, with no corridor width, no cone and
/// no special case for "on the route":
///
///   * Something **on the line** costs nothing. The detour is zero, the fight
///     scores the full [`FIGHT_APPEAL`], and the Mario stops and deals with it
///     -- at any distance, because walking at it *is* walking to the spot.
///   * Something **beside the line** costs the width of the sidestep, and the
///     longer the march the less that is: five metres off a thirty-metre order
///     is a metre and a half of detour and gets fought, fifteen metres off the
///     same order is twelve and does not.
///   * Something **behind** costs double -- there and back -- so a grudge a
///     Mario is walking away from stays walked away from. This is the case
///     that made the whole thing safe to do: [`crate::enemy::alert`] never
///     lets go of a target, so most Marios are carrying one, and raw distance
///     cannot tell "in front of me" from "behind me" at all.
///   * Something **past the spot** costs the overshoot twice, so a squad sent
///     to a spot stops at the spot rather than running on at the first thing
///     it can see beyond it. It arrives, the order retires into a post, and
///     [`HOLD_APPEAL`] lets it have the fight from there.
///
/// Only against an order. A Mario ambling, holding a post or trailing the
/// player is not on its way anywhere it minds about, so a fight is scored at
/// what it is: how far off it is.
fn detour(quarry: Option_, ordered: Option<Option_>) -> f32 {
    let Some(ordered) = ordered else {
        return quarry.range;
    };
    // Non-negative by the triangle inequality; clamped because it is three
    // subtractions of floats and a hair below zero would be an arrival bonus.
    (quarry.range + quarry.at.distance(ordered.at) - ordered.range).max(0.0)
}

/// What one option scores, water and walls and all.
///
/// **The harshest penalty that applies, rather than all of them multiplied
/// together.** Both of them are saying the same thing from different angles --
/// that the walk is not the simple one [`Option_::range`] describes -- and
/// stacking them is double-counting a single fact. It also has a cliff in it: a
/// ball that is both in the moat and round the far side of a fence scores a
/// sixteenth, which lands under [`IDLE_APPEAL`], and a Mario that stands about
/// rather than fetch the only thing on the lawn is a worse bug than one that
/// takes a long walk for it.
fn score(option: Option_, base: f32, scale: f32) -> f32 {
    let mut discount = 1.0_f32;
    if option.wet {
        discount = discount.min(WET_PENALTY);
    }
    if option.blocked {
        discount = discount.min(BLOCKED_PENALTY);
    }
    appeal(base, option.range, scale) * discount
}

/// Picks what a Mario does this tick.
///
/// Pure, and deliberately so: this is the whole of the squad's behaviour and it
/// is a function from a struct to an enum, which means every decision in the
/// game can be written down as an assertion. See the tests.
///
/// **Scored, with one hard rule on top of the scoring: a Mario in a fight does
/// not haul.** Fetching and delivering are simply not on the list while
/// something it can see is coming for it -- see [`engaged`]. That is a gate
/// rather than a very large number because it has to be *absolute*: any
/// finite score for fighting is a distance at which a ball outbids it, and
/// "the Mario turned its back on the slime it was fighting to pick something
/// up" is a bug at every one of those distances, not only at the far ones.
///
/// An order is deliberately **not** gated. A whistle is the player speaking,
/// and a squad that ignores it because a slime wandered into view is worse than
/// one that leaves a fight when told to; that contest stays scored, where an
/// order out-reaches a fight it would have to leave the line for and yields to
/// one standing on that line. See [`detour`] for what "distant" means to a
/// Mario that has somewhere to be.
pub fn choose(options: &Options) -> Goal {
    let mut best = (IDLE_APPEAL, Goal::Idle);
    let hauling = !engaged(options);
    let mut offer = |worth: f32, goal: Goal| {
        // Strictly better, so an earlier option holds a tie. The order below is
        // therefore the tie-break, and it runs from the most specific job to the
        // least: carrying something beats going to get something.
        if worth > best.0 {
            best = (worth, goal);
        }
    };
    if let Some(mast) = options.mast.filter(|_| hauling) {
        offer(
            score(mast, DELIVER_APPEAL, DELIVER_SCALE),
            Goal::Deliver {
                at: mast.at,
                arrive: mast.arrive,
            },
        );
    }
    if let Some((ball, option)) = options.ball.filter(|_| hauling) {
        offer(
            score(option, FETCH_APPEAL, FETCH_SCALE),
            Goal::Fetch {
                ball,
                at: option.at,
                arrive: option.arrive,
            },
        );
    }
    if let Some(quarry) = options.quarry {
        offer(
            score(
                Option_ {
                    // Scored by how far out of the Mario's way it is rather
                    // than by how far off it is, which is the whole of
                    // fighting things on the way. See [`detour`].
                    range: detour(quarry, options.ordered),
                    // **And not discounted for having something in the way**,
                    // which is the one place [`BLOCKED_PENALTY`] does not
                    // belong. Work it through: a fight is the shortest-scaled
                    // option there is, so a quarter of it is under
                    // [`IDLE_APPEAL`] by fifteen metres and under a formation
                    // slot by three -- and [`crate::flow::FlowField::clear`] is
                    // a coarse question, false for a kerb, a hummock or the
                    // corner of a wall anywhere along a line drawn over cells
                    // nearly two metres across. Across a lawn at ten metres it
                    // is false more often than not. So the squad stood about
                    // while things walked up to it, and the further off the
                    // thing was the likelier that was: this is "Marios have a
                    // hard time punching things that are farther away".
                    //
                    // The penalty is right for a *ball*, which is the case it
                    // was written for: one behind a fence is worth less than
                    // one on the lawn because they are alternatives and the
                    // Mario picks. A quarry is not an alternative to anything
                    // -- there is one, it noticed the Mario, and it is walking
                    // over -- and the walk round is [`crate::path`]'s job,
                    // which a Mario has and a ball's score cannot see.
                    // `wet` still applies, so nobody swims a moat for a fight.
                    blocked: false,
                    ..quarry
                },
                FIGHT_APPEAL,
                FIGHT_SCALE,
            ),
            Goal::Fight {
                at: quarry.at,
                arrive: quarry.arrive,
            },
        );
    }
    if let Some(holding) = options.holding {
        offer(
            score(holding, HOLD_APPEAL, HOLD_SCALE),
            Goal::Hold {
                at: holding.at,
                arrive: holding.arrive,
            },
        );
    }
    if let Some(following) = options.following {
        offer(
            score(following, FOLLOW_APPEAL, FOLLOW_SCALE),
            Goal::Obey {
                at: following.at,
                arrive: following.arrive,
            },
        );
    }
    if let Some(ordered) = options.ordered {
        offer(
            score(ordered, OBEY_APPEAL, OBEY_SCALE),
            Goal::Obey {
                at: ordered.at,
                arrive: ordered.arrive,
            },
        );
    }
    best.1
}

/// Reads the field, scores everyone's options, and writes down what each Mario
/// decided.
///
/// Runs before [`crate::squad::move_allies`], which does nothing but walk
/// towards [`Ally::plan`]. The split is the point: deciding is a question about
/// the whole field and walking is a question about one body, and keeping them
/// in one system is how the old chain ended up unable to see that a fight forty
/// metres away is not a fight.
#[allow(clippy::type_complexity)]
pub fn plan(
    level: Res<LevelData>,
    // Only to ask whether there is anything between a Mario and the thing it is
    // thinking about. See [`Option_::blocked`].
    field: Res<crate::flow::FlowField>,
    network: Res<Network>,
    tuning: Res<crate::console::GameTuning>,
    squad: Res<crate::squad::Squad>,
    mut allies: Query<(
        Entity,
        &mut Ally,
        &Transform,
        Option<&crate::enemy::Aggro>,
        &mut crate::path::Route,
    )>,
    mut balls: Query<(Entity, &mut nuclonium::Nuclonium, &Transform), Without<Ally>>,
) {
    // Where a ball could be taken. Taken once rather than per Mario: this is a
    // handful of points and a sweep per ally would be a sweep per ally.
    let masts: Vec<Vec3> = network
        .nodes
        .iter()
        .filter(|node| node.hops.is_some())
        .map(|node| node.at)
        .collect();
    let carrying: Vec<Entity> = {
        let mut held: Vec<Entity> = balls
            .iter()
            .filter_map(|(_, ball, _)| match ball.held {
                nuclonium::Held::Carried(mario) => Some(mario),
                nuclonium::Held::Loose { .. } | nuclonium::Held::Following(_) => None,
            })
            .collect();
        held.sort_unstable();
        held
    };
    let alive: Vec<Entity> = {
        let mut all: Vec<Entity> = allies.iter().map(|(entity, ..)| entity).collect();
        all.sort_unstable();
        all
    };
    let is_alive = |who: Entity| alive.binary_search(&who).is_ok();
    let wet = |at: Vec3| level.water_depth(at).is_some_and(|depth| depth > 0.0);

    // **Every claim is torn up and re-made from scratch each tick.** A claim is
    // a Mario's current plan and nothing more, so it has to expire exactly when
    // the plan does. Written once and cleared only on death -- which is what
    // this did first -- a Mario that walked half way to a ball and then decided
    // to fight instead left it reserved for the rest of the session.
    for (_, mut ball, _) in &mut balls {
        if matches!(ball.held, nuclonium::Held::Loose { claimed: Some(_) }) {
            ball.held = nuclonium::Held::Loose { claimed: None };
        }
    }

    for (mario, mut ally, transform, aggro, mut route) in &mut allies {
        let here = transform.translation;
        let flat = Vec2::new(here.x, here.z);
        let mut options = Options {
            // What counts as being in a fight, off the same slider the enemies
            // notice each other with. See [`engaged`].
            engage: tuning.enemy_sight,
            ..Options::default()
        };

        // Where the squad wants it, and -- the distinction that matters -- on
        // which of the two grounds. Read off the squad rather than off
        // `Ally::goal`, so the record of what was asked for and the decision
        // about whether to do it right now stay separate things.
        //
        // Neither is discounted for being wet, nor for being round a corner:
        // an order is where the player pointed, and second-guessing that is not
        // this module's job. The *walk* there is [`crate::path`]'s business and
        // it goes round.
        let spot = |at: Vec2, arrive: f32| Option_ {
            at,
            arrive,
            range: flat.distance(at),
            wet: false,
            blocked: false,
        };
        match ordered_spot(&squad, mario) {
            // Reached, so the order is spent and what is left is a post. The
            // two are scored quite differently and that difference is the whole
            // of [`HOLD_APPEAL`].
            Some((at, arrive, true)) => options.holding = Some(spot(at, arrive)),
            Some((at, arrive, false)) => options.ordered = Some(spot(at, arrive)),
            None => {
                options.following = squad
                    .members
                    .iter()
                    .position(|member| *member == mario)
                    .and_then(|index| squad.follow_slot(index))
                    .map(|(at, arrive)| spot(at, arrive));
            }
        }
        // What it has noticed. `Aggro::at` is the last place it knew of, which
        // is what it would be walking to in any case.
        if let Some(aggro) = aggro.filter(|aggro| aggro.target.is_some()) {
            let at = Vec2::new(aggro.at.x, aggro.at.z);
            options.quarry = Some(Option_ {
                at,
                // The *edge* of it -- walking to a body's centre is walking to
                // a place `enemy::spread` will not allow, and the two then
                // argue about it for the length of the fight.
                arrive: aggro.room + crate::player::PLAYER_RADIUS + crate::squad::STRIKE_RANGE,
                range: flat.distance(at),
                wet: wet(aggro.at),
                // A slime it can see across the moat is a slime it would have to
                // walk to the bridge for. Discounted rather than struck off:
                // something that has come for a Mario is worth going round for
                // once it is the only thing on the list.
                blocked: !field.clear(here, aggro.at),
            });
        }
        if carrying.binary_search(&mario).is_ok() {
            options.mast = nearest(&masts, here).map(|(at, range)| Option_ {
                at: Vec2::new(at.x, at.z),
                arrive: nuclonium::DELIVER_RANGE * 0.75,
                range,
                wet: wet(at),
                blocked: !field.clear(here, at),
            });
        } else {
            // **Offered whether or not there is anywhere to take it.** Fetching
            // used to be struck off while the network was dark, on the grounds
            // that a ball is only worth having because there is somewhere for
            // it to go -- and that is one more way for the squad to look
            // broken. A player who has not linked their masts up yet, or whose
            // last mast has just been knocked over, sees Marios walking past
            // the takings of a fight they just won. A Mario that picks one up
            // and holds it is doing something useful: the moment a mast lights,
            // `mast` is offered and it delivers.
            //
            // **Picked by what it scores rather than by how far off it is**,
            // which is the difference between a squad that collects the lawn
            // and one that walks into a fence. Nearest is the right answer only
            // when every candidate is worth the same, and they are not: one in
            // the moat is worth a quarter, and one round the far side of
            // something is worth a quarter again. Scoring here also means this
            // loop and [`choose`] agree about which ball is the good one, which
            // they did not when one sorted by distance and the other by worth.
            let mut best: Option<(Entity, Option_, f32)> = None;
            for (ball, held, at) in &balls {
                if !held.available(mario, is_alive) {
                    continue;
                }
                let range = flat.distance(Vec2::new(at.translation.x, at.translation.z));
                if range > FETCH_RANGE {
                    continue;
                }
                // The most this could possibly be worth, before anything about
                // it has been looked up: no water, nothing in the way. If even
                // that loses to what is already in hand, the lookups are not
                // worth paying for -- which is what keeps the grid walk below
                // to the handful of balls that could still win.
                if best
                    .is_some_and(|(_, _, worth)| appeal(FETCH_APPEAL, range, FETCH_SCALE) <= worth)
                {
                    continue;
                }
                let option = Option_ {
                    at: Vec2::new(at.translation.x, at.translation.z),
                    arrive: nuclonium::PICKUP_RANGE * 0.75,
                    range,
                    wet: wet(at.translation),
                    blocked: !field.clear(here, at.translation),
                };
                let worth = score(option, FETCH_APPEAL, FETCH_SCALE);
                if best.is_none_or(|(_, _, had)| worth > had) {
                    best = Some((ball, option, worth));
                }
            }
            options.ball = best.map(|(ball, option, _)| (ball, option));
        }

        ally.plan = choose(&options);
        // And the way there, which is a separate question from what to do and
        // is answered by a separate module -- but *asked* here, because this is
        // the tick and the line on which the destination is decided. Restating
        // an unchanged want costs a compare; see [`crate::path::Route::want`].
        //
        // The water toll is this Mario's own caution against the console's
        // number, which is the same preference [`Goal::caution`] hands the
        // steering, one scale up: an order routes through the moat where an
        // errand walks to the bridge.
        match ally.plan.destination() {
            Some((at, _)) => route.want(
                at,
                crate::flow::Tolls {
                    wet: tuning.path_toll * ally.plan.caution(),
                    hug: tuning.path_clearance,
                },
            ),
            None => route.clear(),
        }
        // Whatever it settled on going to get is now spoken for, so the next
        // Mario in this same loop picks a different ball.
        if let Some(ball) = ally.plan.claim() {
            if let Ok((_, mut held, _)) = balls.get_mut(ball) {
                held.held = nuclonium::Held::Loose {
                    claimed: Some(mario),
                };
            }
        }
    }
}

/// Where the player *sent* this Mario, if anywhere, and whether it is there
/// yet.
///
/// Only the whistle's sends. A formation slot used to come out of here too, and
/// scoring the two the same is the whole of the bug [`FOLLOW_APPEAL`] describes:
/// a squad that is following the player is always standing next to its slot, so
/// an order-strength follow is an order-strength job that never goes away.
///
/// The flag is the same bug wearing the squad's own clothes. An order that has
/// been carried out is a place to stand rather than a journey to make, and
/// [`HOLD_APPEAL`] is what it is worth once it is.
fn ordered_spot(squad: &crate::squad::Squad, mario: Entity) -> Option<(Vec2, f32, bool)> {
    squad
        .sent
        .iter()
        .find(|order| order.who == mario)
        .map(|order| (order.at, crate::squad::SEND_ARRIVE, order.arrived))
}

/// The nearest of a handful of points, and how far off it is.
fn nearest(points: &[Vec3], from: Vec3) -> Option<(Vec3, f32)> {
    points
        .iter()
        .map(|at| (*at, Vec2::new(at.x - from.x, at.z - from.z).length()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something at a point on the ground, with the Mario at the origin.
    ///
    /// The range is worked out from the position rather than passed in beside
    /// it, because [`detour`] reads both and a test that let the two disagree
    /// would be describing a world that cannot exist.
    fn beside(x: f32, z: f32) -> Option_ {
        Option_ {
            at: Vec2::new(x, z),
            arrive: 1.0,
            range: Vec2::new(x, z).length(),
            wet: false,
            blocked: false,
        }
    }

    /// Something straight ahead of the Mario. Everything laid out with this is
    /// on one line, which is the arrangement [`detour`] charges nothing for.
    fn spot(range: f32) -> Option_ {
        beside(range, 0.0)
    }

    fn ball() -> Entity {
        Entity::from_raw_u32(7).unwrap()
    }

    #[test]
    fn nothing_to_do_is_ambling() {
        assert_eq!(choose(&Options::default()), Goal::Idle);
    }

    #[test]
    fn something_standing_on_a_mario_beats_everything_else_it_could_do() {
        // A slime at arm's length, an order fifty metres off and a ball three
        // metres away. The fight wins, which is the whole reason fighting is
        // scored highest at zero range.
        let options = Options {
            quarry: Some(spot(1.0)),
            ordered: Some(spot(50.0)),
            ball: Some((ball(), spot(3.0))),
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Fight { .. }));
    }

    #[test]
    fn the_same_fight_across_the_lawn_loses_to_the_ball_at_its_feet() {
        // **This is the case the priority chain could not express, and the
        // reason this module exists.** Aggro is never given up, so this Mario
        // will hold that distant target for the rest of the session; under the
        // chain it therefore never fetched anything again.
        let options = Options {
            quarry: Some(spot(40.0)),
            ball: Some((ball(), spot(3.0))),
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Fetch { .. }));
    }

    #[test]
    fn a_mario_with_nothing_else_on_will_cross_a_lawn_for_a_ball() {
        // No threshold anywhere: a ball at the far end of `FETCH_RANGE` still
        // outscores standing about, which is what makes a squad tidy up a field
        // it has finished fighting over.
        let options = Options {
            ball: Some((ball(), spot(FETCH_RANGE))),
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Fetch { .. }));
    }

    #[test]
    fn an_order_outreaches_a_distant_fight_and_yields_to_a_near_one() {
        let far = Options {
            ordered: Some(spot(20.0)),
            quarry: Some(spot(25.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&far), Goal::Obey { .. }), "{far:?}");
        let near = Options {
            ordered: Some(spot(20.0)),
            quarry: Some(spot(2.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&near), Goal::Fight { .. }), "{near:?}");
    }

    /// **The bug this whole `Hold` business exists for.** A squad tapped at a
    /// nest walks over, spreads out around it, and then stands there while
    /// slimes wander past its elbow -- because every one of them is standing on
    /// the spot it was told to stand on, and an order at zero range beats a
    /// fight at any range past about eighty centimetres.
    #[test]
    fn a_mario_that_has_reached_the_spot_it_was_sent_to_fights_what_it_finds_there() {
        let arrived = Options {
            holding: Some(spot(0.0)),
            quarry: Some(spot(4.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(
            matches!(choose(&arrived), Goal::Fight { .. }),
            "it stood on its spot in the middle of a fight: {arrived:?}"
        );
        // And the same Mario still on its way keeps walking past a fight it
        // would have to leave the line for, because an order it has not
        // carried out yet is still an order. Four metres *ahead* it would
        // stop for; twelve metres out to one side of a twenty-metre march it
        // does not. See [`detour`].
        let marching = Options {
            ordered: Some(spot(20.0)),
            quarry: Some(beside(2.0, 12.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(
            matches!(choose(&marching), Goal::Obey { .. }),
            "{marching:?}"
        );
    }

    /// **What a squad sent across a field full of slimes is expected to do.**
    /// It was not doing it: an order is worth 0.60 at thirty metres and a
    /// fight is worth 0.60 at four and a half, so a marching Mario swung only
    /// at what was already close enough to hit it and squeezed round
    /// everything else -- the slime standing squarely in the gateway included.
    ///
    /// Scoring the fight by [`detour`] rather than by distance is the whole of
    /// the fix, and the three cases here are the three the subtraction has to
    /// tell apart.
    #[test]
    fn a_marching_mario_fights_what_stands_in_its_way_and_walks_past_what_does_not() {
        // Dead on the line, half way along. Walking at it is walking to the
        // spot, so it costs nothing and is dealt with -- at eighteen metres,
        // which is well past anything raw distance would have stopped for.
        let ahead = Options {
            ordered: Some(spot(40.0)),
            quarry: Some(spot(18.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(matches!(choose(&ahead), Goal::Fight { .. }), "{ahead:?}");
        // The same creature, the same distance off, square to the march. Now
        // it is twenty-two metres of walking that the order does not want, and
        // the order wins.
        let aside = Options {
            ordered: Some(spot(40.0)),
            quarry: Some(beside(0.0, 18.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(matches!(choose(&aside), Goal::Obey { .. }), "{aside:?}");
        // And a grudge it is walking away from costs the walk twice, which is
        // what keeps the permanent aggro every Mario carries from turning the
        // squad round. Six metres behind, and it does not go back for it.
        let behind = Options {
            ordered: Some(spot(40.0)),
            quarry: Some(beside(-6.0, 0.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(matches!(choose(&behind), Goal::Obey { .. }), "{behind:?}");
    }

    /// A corridor, without a corridor width anywhere in the code: how far to
    /// one side counts as "on the way" falls out of how long the march is.
    #[test]
    fn how_far_off_the_line_a_mario_will_step_grows_with_the_march() {
        let past = |x: f32, z: f32| Options {
            ordered: Some(spot(30.0)),
            quarry: Some(beside(x, z)),
            engage: 14.0,
            ..Options::default()
        };
        // Five metres off a thirty-metre order is a metre and a half of extra
        // walking. Worth it.
        let near = past(15.0, 5.0);
        assert!(matches!(choose(&near), Goal::Fight { .. }), "{near:?}");
        // Fifteen metres off the same order is twelve. Not worth it.
        let wide = past(15.0, 15.0);
        assert!(matches!(choose(&wide), Goal::Obey { .. }), "{wide:?}");
    }

    /// The detour is only charged against an order, because only an order is
    /// somewhere the Mario minds about being. A Mario holding a post scores a
    /// fight at what it is: how far off it is.
    #[test]
    fn a_mario_with_no_order_scores_a_fight_at_its_own_range() {
        // Off to one side of nothing at all, so there is no line to be off.
        // Four metres from a Mario standing on its post is a fight.
        let posted = Options {
            holding: Some(spot(0.0)),
            quarry: Some(beside(0.0, 4.0)),
            engage: 14.0,
            ..Options::default()
        };
        assert!(matches!(choose(&posted), Goal::Fight { .. }), "{posted:?}");
    }

    /// Holding a spot is worth more than nothing, which is what brings a Mario
    /// back to it once the fight it left for is over.
    #[test]
    fn a_mario_with_the_field_to_itself_goes_back_to_its_post() {
        let options = Options {
            holding: Some(spot(9.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Hold { .. }), "{options:?}");
        // Standing on it, with nothing else on, it stays: the post outscores
        // ambling at every range a Mario could be shoved to.
        let standing = Options {
            holding: Some(spot(0.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&standing), Goal::Hold { .. }));
    }

    /// A post is a preference and an order is an order: the same Mario walks
    /// into water for one and round it for the other.
    #[test]
    fn a_post_is_walked_to_more_carefully_than_an_order() {
        assert!(
            Goal::Obey {
                at: Vec2::ZERO,
                arrive: 1.0
            }
            .caution()
                < Goal::Hold {
                    at: Vec2::ZERO,
                    arrive: 1.0
                }
                .caution()
        );
        // And nothing is fearless or paralysed: every job weighs a hazard, and
        // none of them weighs it infinitely. See [`crate::squad::steer`].
        for goal in [
            Goal::Idle,
            Goal::Obey {
                at: Vec2::ZERO,
                arrive: 1.0,
            },
            Goal::Hold {
                at: Vec2::ZERO,
                arrive: 1.0,
            },
            Goal::Fight {
                at: Vec2::ZERO,
                arrive: 1.0,
            },
            Goal::Deliver {
                at: Vec2::ZERO,
                arrive: 1.0,
            },
            Goal::Fetch {
                ball: ball(),
                at: Vec2::ZERO,
                arrive: 1.0,
            },
        ] {
            assert!(
                goal.caution() > 0.0 && goal.caution().is_finite(),
                "{goal:?}"
            );
        }
    }

    #[test]
    fn a_mario_carrying_something_takes_it_home_past_a_passing_slime() {
        // Carrying beats a fight it is not actually in. It does not beat one it
        // is: something with its hands on the Mario still wins.
        let passing = Options {
            mast: Some(spot(30.0)),
            quarry: Some(spot(6.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&passing), Goal::Deliver { .. }));
        let grappling = Options {
            mast: Some(spot(30.0)),
            quarry: Some(spot(1.0)),
            ..Options::default()
        };
        assert!(matches!(choose(&grappling), Goal::Fight { .. }));
    }

    /// **The screenshot this was written for.** A ball eight metres away
    /// through a fence and across the moat, and one twenty metres away on the
    /// same lawn. Scored on how far off they are, the squad walks up to the
    /// railings and presses against them -- and that is the right answer to the
    /// question, because the far side of a fence really is eight metres away.
    #[test]
    fn a_thing_round_the_far_side_of_something_loses_to_a_further_one_in_reach() {
        let near_but_round_a_corner = Option_ {
            blocked: true,
            wet: true,
            ..spot(8.0)
        };
        let far_but_on_this_side = spot(20.0);
        assert!(
            score(far_but_on_this_side, FETCH_APPEAL, FETCH_SCALE)
                > score(near_but_round_a_corner, FETCH_APPEAL, FETCH_SCALE),
            "it went for the one in the water"
        );
        // And it is a discount rather than a refusal: with nothing else on the
        // lawn, somebody does go round for it.
        let options = Options {
            ball: Some((ball(), near_but_round_a_corner)),
            ..Options::default()
        };
        assert!(
            matches!(choose(&options), Goal::Fetch { .. }),
            "{options:?}"
        );
    }

    /// **And the case that is not that one: a fight.** A kerb, a hummock or the
    /// corner of a wall anywhere along the line makes
    /// [`crate::flow::FlowField::clear`] say no, which over ten metres of lawn
    /// it does more often than not -- and a quarter of a fight is under a
    /// formation slot from three metres and under [`IDLE_APPEAL`] from fifteen.
    /// So the squad stood about while things walked up to it, and the further
    /// off the thing was the likelier that was.
    #[test]
    fn a_fight_is_not_talked_out_of_by_a_bump_in_the_lawn() {
        let over_a_kerb = Option_ {
            blocked: true,
            ..spot(9.0)
        };
        let holding_formation = Options {
            following: Some(spot(0.0)),
            quarry: Some(over_a_kerb),
            ..Options::default()
        };
        assert!(
            matches!(choose(&holding_formation), Goal::Fight { .. }),
            "it held formation while something walked up to it: {holding_formation:?}"
        );
        // Water still is a wall's worth of discouragement, because swimming a
        // moat for a fight is a different sentence from walking round a rock.
        let across_the_moat = Options {
            following: Some(spot(0.0)),
            quarry: Some(Option_ {
                wet: true,
                ..spot(9.0)
            }),
            ..Options::default()
        };
        assert!(
            matches!(choose(&across_the_moat), Goal::Obey { .. }),
            "it swam the moat for a slime: {across_the_moat:?}"
        );
    }

    #[test]
    fn water_is_a_cost_rather_than_a_wall() {
        // Given a dry ball and a wet one, the squad takes the dry one, even
        // though the wet one is nearer.
        let wet_ball = Option_ {
            wet: true,
            ..spot(4.0)
        };
        let choice = choose(&Options {
            ball: Some((ball(), wet_ball)),
            ..Options::default()
        });
        assert!(
            matches!(choice, Goal::Fetch { .. }),
            "nobody went in for it at all: {choice:?}"
        );
        // But a wet ball at your feet is worth less than a dry one across the
        // lawn, which is what makes the preference a preference rather than a
        // rounding error. Scored rather than chosen, because `choose` is only
        // ever offered one ball.
        assert!(
            score(wet_ball, FETCH_APPEAL, FETCH_SCALE)
                < score(spot(30.0), FETCH_APPEAL, FETCH_SCALE),
            "the squad would swim four metres rather than walk thirty"
        );
        // And far enough out the discount is what tips it below ambling, so
        // nobody swims the length of the moat for one. The crossover has to sit
        // *inside* the sweep limit or it is not a rule at all -- see
        // [`WET_PENALTY`], which is set against exactly this.
        let crossover = (1..FETCH_RANGE as u32)
            .map(|range| range as f32)
            .find(|&range| {
                score(
                    Option_ {
                        wet: true,
                        ..spot(range)
                    },
                    FETCH_APPEAL,
                    FETCH_SCALE,
                ) < IDLE_APPEAL
            });
        let Some(crossover) = crossover else {
            panic!("a wet ball outscores ambling at every range a Mario can see");
        };
        assert!(
            (30.0..70.0).contains(&crossover),
            "a Mario gives up on a ball in the water at {crossover} m"
        );
    }

    /// Sight range, which is what [`plan`] passes as `engage`.
    const SIGHT: f32 = 14.0;

    #[test]
    fn a_mario_in_a_fight_does_not_stop_to_pick_anything_up() {
        // **The rule the player asked for.** A ball under its feet and a slime
        // it can see: it fights, and it goes on fighting. Note how far apart
        // the two are -- a ball at arm's length and a quarry most of the way
        // out to the edge of sight -- because a *score* would have handed this
        // one to the ball, and did.
        let options = Options {
            quarry: Some(spot(SIGHT - 1.0)),
            ball: Some((ball(), spot(0.5))),
            engage: SIGHT,
            ..Options::default()
        };
        assert!(
            matches!(choose(&options), Goal::Fight { .. }),
            "{options:?}"
        );
        // And it does not run its takings home either. Delivering is finishing
        // a job rather than starting one, but it is still a Mario walking away
        // from something that is hitting it.
        let laden = Options {
            quarry: Some(spot(SIGHT - 1.0)),
            mast: Some(spot(3.0)),
            engage: SIGHT,
            ..Options::default()
        };
        assert!(matches!(choose(&laden), Goal::Fight { .. }), "{laden:?}");
    }

    #[test]
    fn a_grudge_across_the_valley_is_not_a_fight() {
        // The other half, and the reason the rule above can be absolute. Aggro
        // is never given up -- see this module's preamble -- so every Mario in
        // a populated level is carrying a target for ever. Past sight it stops
        // being a fight it is in, and the squad gets on with its work.
        let options = Options {
            quarry: Some(spot(SIGHT + 1.0)),
            ball: Some((ball(), spot(3.0))),
            engage: SIGHT,
            ..Options::default()
        };
        assert!(
            matches!(choose(&options), Goal::Fetch { .. }),
            "{options:?}"
        );
    }

    #[test]
    fn an_order_still_reaches_a_mario_that_is_in_a_fight() {
        // Orders are deliberately outside the rule: the whistle is the player
        // speaking, and a squad that cannot be called off a slime is worse than
        // one that leaves a fight when told to.
        //
        // The slime is put square to the march rather than along it, and that
        // is not incidental. Being *in* a fight is about how near the thing is
        // and nothing else, so this is still a Mario with a slime inside sight
        // -- but where the fight is decides whether the order outbids it, and
        // one standing in the gateway is a fight the Mario should stop for.
        // See [`detour`], and the marching cases above.
        let options = Options {
            quarry: Some(beside(0.0, SIGHT - 2.0)),
            ordered: Some(spot(20.0)),
            engage: SIGHT,
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Obey { .. }), "{options:?}");
        // What the rule is actually about: fetching and delivering are struck
        // off while it is engaged, and the order is not.
        assert!(engaged(&options), "{options:?}");
    }

    #[test]
    fn a_mario_holding_formation_still_crosses_a_field_for_a_ball() {
        // **The regression for "they only pick up what is next to a pylon".**
        // A squad following the player is by definition standing near its slot,
        // so this option is on the list every tick of every session. Scored as
        // an order it beat fetching at *every* range -- a ball underfoot
        // included -- and the only allies that ever collected anything were the
        // ones a fight had knocked out of position. Which reads, from outside,
        // as "they only collect where the fighting was".
        let holding = spot(1.0);
        for range in [1.0, 10.0, 25.0, 40.0] {
            let options = Options {
                following: Some(holding),
                ball: Some((ball(), spot(range))),
                ..Options::default()
            };
            assert!(
                matches!(choose(&options), Goal::Fetch { .. }),
                "a ball {range} m off lost to a formation slot: {options:?}"
            );
        }
        // It is a pull rather than nothing, though: with no ball on the field a
        // Mario left a long way behind comes back rather than ambling where it
        // stands.
        let trailing = Options {
            following: Some(spot(40.0)),
            ..Options::default()
        };
        assert!(
            matches!(choose(&trailing), Goal::Obey { .. }),
            "{trailing:?}"
        );
    }

    #[test]
    fn a_mario_the_player_actually_sent_somewhere_goes_there() {
        // The other side of the same split. An order is not a formation slot:
        // told to stand twenty metres off, a Mario goes, and a ball ten metres
        // away does not talk it out of it.
        let options = Options {
            ordered: Some(spot(20.0)),
            ball: Some((ball(), spot(10.0))),
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Obey { .. }), "{options:?}");
    }

    #[test]
    fn appeal_halves_at_its_own_scale_and_never_reaches_zero() {
        assert!((appeal(1.0, 0.0, 10.0) - 1.0).abs() < 1e-6);
        assert!((appeal(1.0, 10.0, 10.0) - 0.5).abs() < 1e-6);
        assert!(appeal(1.0, 10_000.0, 10.0) > 0.0);
        // Monotone, which is what stops a target jittering between two goals as
        // a Mario walks.
        let mut last = f32::INFINITY;
        for step in 0..100 {
            let here = appeal(1.0, step as f32, 12.0);
            assert!(here < last);
            last = here;
        }
    }
}
