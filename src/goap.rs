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
//! [`crate::squad::skirt`].
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
            | Goal::Fight { at, arrive }
            | Goal::Fetch { at, arrive, .. }
            | Goal::Deliver { at, arrive } => Some((at, arrive)),
        }
    }

    /// Whether this is worth walking at the squad's marching pace rather than
    /// strolling. Everything that is a decision is; ambling is not.
    pub fn urgent(self) -> bool {
        !matches!(self, Goal::Idle)
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

/// What one option scores, water and all.
fn score(option: Option_, base: f32, scale: f32) -> f32 {
    let worth = appeal(base, option.range, scale);
    match option.wet {
        true => worth * WET_PENALTY,
        false => worth,
    }
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
/// order out-reaches a distant fight and yields to a near one.
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
            score(quarry, FIGHT_APPEAL, FIGHT_SCALE),
            Goal::Fight {
                at: quarry.at,
                arrive: quarry.arrive,
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
    network: Res<Network>,
    tuning: Res<crate::console::GameTuning>,
    squad: Res<crate::squad::Squad>,
    mut allies: Query<(Entity, &mut Ally, &Transform, Option<&crate::enemy::Aggro>)>,
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
        let mut all: Vec<Entity> = allies.iter().map(|(entity, _, _, _)| entity).collect();
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

    for (mario, mut ally, transform, aggro) in &mut allies {
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
        // Neither is discounted for being wet: an order is where the player
        // pointed, and second-guessing that is not this module's job.
        let spot = |at: Vec2, arrive: f32| Option_ {
            at,
            arrive,
            range: flat.distance(at),
            wet: false,
        };
        match ordered_spot(&squad, mario) {
            Some((at, arrive)) => options.ordered = Some(spot(at, arrive)),
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
            });
        }
        if carrying.binary_search(&mario).is_ok() {
            options.mast = nearest(&masts, here).map(|(at, range)| Option_ {
                at: Vec2::new(at.x, at.z),
                arrive: nuclonium::DELIVER_RANGE * 0.75,
                range,
                wet: wet(at),
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
            let mut best: Option<(Entity, Vec3, f32)> = None;
            for (ball, held, at) in &balls {
                if !held.available(mario, is_alive) {
                    continue;
                }
                let range = flat.distance(Vec2::new(at.translation.x, at.translation.z));
                if range > FETCH_RANGE {
                    continue;
                }
                if best.is_none_or(|(_, _, best)| range < best) {
                    best = Some((ball, at.translation, range));
                }
            }
            options.ball = best.map(|(ball, at, range)| {
                (
                    ball,
                    Option_ {
                        at: Vec2::new(at.x, at.z),
                        arrive: nuclonium::PICKUP_RANGE * 0.75,
                        range,
                        wet: wet(at),
                    },
                )
            });
        }

        ally.plan = choose(&options);
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

/// Where the player *sent* this Mario, if anywhere.
///
/// Only the whistle's sends. A formation slot used to come out of here too, and
/// scoring the two the same is the whole of the bug [`FOLLOW_APPEAL`] describes:
/// a squad that is following the player is always standing next to its slot, so
/// an order-strength follow is an order-strength job that never goes away.
fn ordered_spot(squad: &crate::squad::Squad, mario: Entity) -> Option<(Vec2, f32)> {
    squad
        .sent
        .iter()
        .find(|(sent, _, _)| *sent == mario)
        .map(|(_, at, _)| (*at, crate::squad::SEND_ARRIVE))
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

    fn spot(range: f32) -> Option_ {
        Option_ {
            at: Vec2::new(range, 0.0),
            arrive: 1.0,
            range,
            wet: false,
        }
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
        let options = Options {
            quarry: Some(spot(SIGHT - 1.0)),
            ordered: Some(spot(20.0)),
            engage: SIGHT,
            ..Options::default()
        };
        assert!(matches!(choose(&options), Goal::Obey { .. }), "{options:?}");
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
