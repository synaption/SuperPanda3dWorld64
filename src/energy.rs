//! The player's second pool: what the jetpack burns and what the gun taxes.
//!
//! Health answers "how much more can I take". This answers "how much more can
//! I do", and it is the one number standing between a jetpack that is a tool
//! and a jetpack that is a second pair of legs. Before it, holding the booster
//! in the air was free and unbounded -- the Hero could leave the ground at the
//! castle and arrive anywhere without ever touching it again, which quietly
//! deletes the level's geometry as a thing the player has to solve.
//!
//! So flight is now rationed, and the ration is small enough to be felt:
//!
//! | | what it costs | what it does to the refill |
//! |---|---|---|
//! | thrust, airborne | the whole bar in [`THRUST_SECONDS`] | holds it [`THRUST_DELAY`] after the last burn |
//! | thrust, grounded | nothing | holds it while the key is down |
//! | a shot | [`SHOT_COST`] of the bar | holds it [`SHOT_DELAY`] |
//! | nothing | -- | full again in [`FILL_SECONDS`] |
//!
//! And running it to nothing costs more than the bar: it *drains*, which puts
//! the booster and the gun both out of service until the bar is full again,
//! flashing all the way up. See [`Energy::drained`]. That rule is what makes
//! the last tenth of the bar worth watching -- without it, a player flies until
//! the booster coughs, coasts a second, and flies again on the dregs, and the
//! bar is a stutter rather than a resource. With it, hitting zero is a mistake
//! with a five-second price, so flying to nine tenths and stopping is a skill.
//!
//! Everything inside the lockout is written so that the lockout can end.
//! Nothing that happens while it is on may hold the refill -- a held booster
//! stamps no delay, the skate's stall is ignored, and the gun that would park
//! it for a second is not firing in the first place. A lockout whose only exit
//! is a full bar must not contain a way of stopping the bar filling.
//!
//! **The airborne burn and the refill are the same rate**, and that is the
//! whole feel of the thing: five seconds of flight bought with five seconds of
//! not flying, plus a second of dead air on top that a player learns to plan
//! around. The skate is deliberately not on the same footing -- it costs
//! nothing, because it is a way of *crossing the ground* rather than a way of
//! ignoring it, and charging for it would only teach the player to walk.
//!
//! The gun's tax is a hundredth of the bar, which is nearly nothing per shot
//! and is not the point. [`SHOT_DELAY`] is the point: a shot parks the refill
//! for a second, so a firefight is a second in which no flight is being banked.
//! That is what puts the two systems on one bar rather than on two, and it is
//! why a player who empties the magazine on the way up does not get back down
//! the way he planned.
//!
//! Nothing else in the game has one of these. Enemies and Marios carry
//! [`crate::health::Health`] and no `Energy`, and every system here is written
//! to fail open on an actor without one -- see [`crate::player::movement`],
//! where a player assembled without this component flies the way he did before
//! the bar existed rather than not flying at all.

use bevy::prelude::*;

use crate::health;

/// How long the bar takes to come back from empty, with nothing spending it.
pub const FILL_SECONDS: f32 = 5.0;

/// How long a full bar holds the booster up.
///
/// Equal to [`FILL_SECONDS`] on purpose: see the table above.
pub const THRUST_SECONDS: f32 = 5.0;

/// How long the refill is held after the last airborne burn.
pub const THRUST_DELAY: f32 = 1.0;

/// How long the refill is held by a shot, and what fraction of the bar the
/// shot itself spends.
pub const SHOT_DELAY: f32 = 1.0;
pub const SHOT_COST: f32 = 0.01;

/// The pool, from empty to full, and how long until it starts filling again.
///
/// A fraction rather than a count of points, unlike [`health::Health`], because
/// nothing about it is ever counted: no blow spends a whole number of it, no
/// second pool has to be compared against it, and the only reader is a bar that
/// wants a width. Storing what the one reader wants is what keeps the maximum
/// out of the struct entirely.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Energy {
    /// Zero to one. [`Self::advance`], [`Self::thrust`] and [`Self::shoot`] are
    /// the only things that write it and none of them can leave that range.
    pub level: f32,
    /// Seconds of refill still owed before the bar starts climbing. Counted
    /// down by [`Self::advance`] instead of being a deadline compared against a
    /// clock, so a paused game or a console left open does not silently pay it.
    pub delay: f32,
    /// Set the moment the bar reaches zero and cleared the moment it reaches
    /// one, and nowhere in between. Everything that spends the bar refuses
    /// while it is set: see [`Self::drained`].
    ///
    /// A flag rather than the `level == 0.0` test it could be derived from,
    /// because the two are only the same for one step. The whole point of the
    /// lockout is that it outlives the emptiness that started it, all the way
    /// up the bar.
    drained: bool,
}

impl Default for Energy {
    fn default() -> Self {
        Self::new()
    }
}

impl Energy {
    /// A full bar, spending nothing.
    pub fn new() -> Self {
        Self {
            level: 1.0,
            delay: 0.0,
            drained: false,
        }
    }

    /// Whether the bar has been run to nothing and has not yet come back full.
    ///
    /// The one question every spender asks first. It is deliberately *not*
    /// "is the bar empty": a player who drained it at the bottom of a shaft
    /// and has climbed a third of the way back is still locked out, and the
    /// bar flashing at him is saying so.
    pub fn drained(&self) -> bool {
        self.drained
    }

    /// Takes `amount` off the bar and trips the lockout if that finished it.
    ///
    /// The single place `level` is spent, so there is one place the lockout can
    /// be missed from rather than one per spender. It also drops the hold on
    /// the way down: from here the only thing that clears the lockout is the
    /// bar reaching full, and a delay left standing would be a second of a
    /// five-second sentence served for nothing.
    fn spend(&mut self, amount: f32) {
        self.level = (self.level - amount).max(0.0);
        if self.level <= 0.0 {
            self.drained = true;
            self.delay = 0.0;
        }
    }

    /// Runs the clock one fixed step: pays down the hold, or fills if there is
    /// none left to pay.
    ///
    /// **Call this once at the top of a step, before anything spends any.**
    /// The order is what makes the delays land on the durations they are named
    /// for: a burn at the end of step *N* sets a full second, and the first
    /// step that takes any of it back is *N + 1*. Advancing afterwards instead
    /// would hand a step of it back before the burn had even been drawn.
    pub fn advance(&mut self, dt: f32) {
        if self.delay > 0.0 {
            self.delay = (self.delay - dt).max(0.0);
            return;
        }
        self.level = (self.level + dt / FILL_SECONDS).min(1.0);
        if self.level >= 1.0 {
            // The only way out of the lockout, and it is the same line whether
            // the bar spent five seconds climbing or was never drained at all.
            self.drained = false;
        }
    }

    /// Spends a step of airborne thrust, and reports whether there was any to
    /// spend.
    ///
    /// A `false` here is the booster cutting out mid-air, which is the whole
    /// mechanism: the caller falls through to gravity on the same step. The
    /// hold is stamped on every burning step rather than on the last one,
    /// because there is no way to know which step is the last until it is over.
    ///
    /// **A drained bar refuses and stamps nothing**, however long the key is
    /// held over it. Holding the booster down through a lockout must not be a
    /// way of extending the lockout, and a hold stamped every step by a burn
    /// that never happens is exactly that: the refill would never start and the
    /// player would be waiting on a bar that had stopped moving.
    pub fn thrust(&mut self, dt: f32) -> bool {
        if self.drained || self.level <= 0.0 {
            self.drained = true;
            return false;
        }
        self.delay = THRUST_DELAY;
        self.spend(dt / THRUST_SECONDS);
        true
    }

    /// Holds the refill for this step and no longer: the grounded skate.
    ///
    /// One step's worth rather than [`THRUST_DELAY`], so letting go of the key
    /// on the ground has the bar climbing again immediately. Costing nothing
    /// but still stopping the fill is what stops a skate across a lawn doubling
    /// as a way of charging for the flight at the end of it.
    ///
    /// Ignored while the bar is drained. The skate itself still works -- it
    /// spends nothing, so the lockout has no quarrel with it -- but a player
    /// who lands from a drained flight is usually still holding the key, and a
    /// stall that counted there would leave him skating across a lawn with a
    /// bar that never comes back and no way of knowing why.
    pub fn stall(&mut self, dt: f32) {
        if self.drained {
            return;
        }
        self.delay = self.delay.max(dt);
    }

    /// Spends a shot: a hundredth of the bar, and a second of held refill.
    ///
    /// `max` rather than an assignment, so a shot fired while the booster is
    /// still burning cannot *shorten* the hold that burn has already stamped.
    /// A shot fired while the bar was climbing stops it, which is the case the
    /// number was chosen for.
    ///
    /// Callers check [`Self::drained`] first and do not fire at all inside a
    /// lockout, so the guard here is belt to that braces: a shot that slipped
    /// through would park the refill for a second of a sentence whose only
    /// remission is the bar filling.
    pub fn shoot(&mut self) {
        if self.drained {
            return;
        }
        self.spend(SHOT_COST);
        self.delay = self.delay.max(SHOT_DELAY);
    }

    /// Whether the bar is being held rather than climbing, which is the one
    /// thing about it worth colouring differently.
    pub fn held(&self) -> bool {
        self.delay > 0.0
    }
}

/// Marks the bar's frame, whose border is half of the drained flash.
#[derive(Component)]
pub struct EnergyBarFrame;

/// Marks the coloured part of the player's energy bar.
#[derive(Component)]
pub struct EnergyBarFill;

/// The percentage written across it.
#[derive(Component)]
pub struct EnergyBarLabel;

/// The bar, in window pixels. As wide as the health bar it sits under and
/// shorter than it, because the two are not equals: one of them is whether you
/// are alive.
pub const ENERGY_BAR: Vec2 = Vec2::new(health::PLAYER_BAR.x, 14.0);

/// What the bar is drawn in while it is climbing, and while it is held.
///
/// The same green the health bar used to be. The two bars do not need two
/// colours to be told apart -- they are one above the other in the same corner
/// and one of them is two thirds the height of the other -- and they do need to
/// read as the same *kind* of thing, which is a quantity of yours that goes
/// down when something spends it. Health is the red one now: see
/// `health::fill_colour`.
///
/// The held colour is that green with the life taken out of it. A bar that is
/// *not coming back yet* is the fact the player is waiting on, and it should be
/// legible without reading the number written across it.
const FILL_LIVE: Color = Color::srgb(0.30, 0.85, 0.35);
const FILL_HELD: Color = Color::srgb(0.19, 0.40, 0.22);

/// What a drained bar flashes to, and how many times a second.
///
/// Amber rather than red: red is the health bar's whole family now, and a
/// player glancing at something flashing red in that corner would read it as
/// dying rather than as grounded. Four a second is fast enough to catch the eye
/// in a fight and slow enough not to strobe over the five seconds it runs for.
const FLASH: Color = Color::srgb(1.0, 0.80, 0.25);
const FLASH_HZ: f32 = 4.0;

/// The bar's ordinary border, named because the flash has to put it back.
const EDGE: Color = Color::srgba(0.85, 0.88, 0.95, 0.65);

/// Builds the bar, under the health bar in the same corner.
///
/// Called from the game's startup right after [`health::spawn`], which is what
/// puts the two of them in one stack: this one is pinned to the margin and the
/// health bar is pushed up over it by exactly this bar's height plus the gap.
pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(health::HUD_MARGIN),
                bottom: Val::Px(health::HUD_MARGIN),
                width: Val::Px(ENERGY_BAR.x),
                height: Val::Px(ENERGY_BAR.y),
                border: UiRect::all(Val::Px(2.0)),
                // Clipped, so a full fill cannot draw over its own border.
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.04, 0.72)),
            BorderColor::all(EDGE),
            GlobalZIndex(15),
            EnergyBarFrame,
        ))
        .with_children(|bar| {
            bar.spawn((
                EnergyBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(FILL_LIVE),
            ));
            bar.spawn((
                EnergyBarLabel,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..default()
                },
                TextColor(Color::srgba(0.05, 0.05, 0.08, 0.95)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(8.0),
                    top: Val::Px(0.0),
                    ..default()
                },
            ));
        });
}

/// Writes the bar every frame, alongside [`health::draw_player_bar`].
///
/// Reads the wall clock rather than counting its own frames, so the flash is
/// four a second on any machine rather than speeding up with the frame rate.
pub fn draw_player_bar(
    time: Res<Time>,
    player: Query<&Energy, With<crate::player::Player>>,
    mut frame: Query<&mut BorderColor, With<EnergyBarFrame>>,
    mut fill: Query<(&mut Node, &mut BackgroundColor), With<EnergyBarFill>>,
    mut label: Query<&mut Text, With<EnergyBarLabel>>,
) {
    let Ok(energy) = player.single() else {
        return;
    };
    // A square wave rather than a sine: a bar that eases in and out of a colour
    // reads as a bar breathing, and this one is meant to read as an alarm.
    let flashing = energy.drained() && (time.elapsed_secs() * FLASH_HZ).fract() < 0.5;
    if let Ok(mut border) = frame.single_mut() {
        // The border and not only the fill, because a bar that has just started
        // refilling is three pixels of colour, and a flash inside it would be
        // invisible for exactly the seconds it matters most.
        *border = BorderColor::all(if flashing { FLASH } else { EDGE });
    }
    if let Ok((mut node, mut colour)) = fill.single_mut() {
        node.width = Val::Percent(energy.level.clamp(0.0, 1.0) * 100.0);
        *colour = BackgroundColor(match (flashing, energy.held() || energy.drained()) {
            (true, _) => FLASH,
            (false, true) => FILL_HELD,
            (false, false) => FILL_LIVE,
        });
    }
    if let Ok(mut text) = label.single_mut() {
        // Rounded down rather than to nearest, so the bar only says "100%" when
        // there really is a whole one to spend.
        **text = format!("{}%", (energy.level.clamp(0.0, 1.0) * 100.0).floor());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::FIXED_DT;

    /// A bar sitting at `level`, holding nothing and not locked out: the state
    /// a test wants when what it is about starts partway up.
    fn part_full(level: f32) -> Energy {
        Energy {
            level,
            ..Energy::new()
        }
    }

    /// Runs `seconds` of steps, spending nothing.
    fn coast(energy: &mut Energy, seconds: f32) {
        for _ in 0..(seconds / FIXED_DT).round() as usize {
            energy.advance(FIXED_DT);
        }
    }

    #[test]
    fn a_full_bar_is_five_seconds_of_flight_and_then_nothing() {
        let mut energy = Energy::new();
        let mut burnt = 0.0;
        while energy.thrust(FIXED_DT) {
            energy.advance(FIXED_DT);
            burnt += FIXED_DT;
            assert!(burnt < THRUST_SECONDS + 1.0, "the booster never cut out");
        }
        assert!(
            (burnt - THRUST_SECONDS).abs() < 0.05,
            "five seconds of thrust, not {burnt}"
        );
        // Not `level == 0.0`: emptying the bar drops the hold with it, so the
        // step after the last burn has already put a sliver back. What is left
        // over is the lockout, and that is what stops him flying on the sliver.
        assert!(energy.drained(), "it ran dry without locking out");
    }

    #[test]
    fn the_refill_waits_a_second_after_the_last_burn() {
        let mut energy = Energy::new();
        energy.thrust(FIXED_DT);
        let spent = energy.level;
        // The whole of the hold and not a step more.
        coast(&mut energy, THRUST_DELAY);
        assert_eq!(energy.level, spent, "it climbed while it was held");
        coast(&mut energy, 1.0);
        assert!(
            energy.level > spent,
            "and it never started again afterwards"
        );
    }

    #[test]
    fn an_empty_bar_fills_in_five_seconds() {
        let mut energy = part_full(0.0);
        coast(&mut energy, FILL_SECONDS);
        assert!(
            (energy.level - 1.0).abs() < 1e-3,
            "a full bar after {FILL_SECONDS}s, not {}",
            energy.level
        );
    }

    #[test]
    fn running_it_to_nothing_locks_it_out_until_it_is_full() {
        let mut energy = Energy::new();
        while energy.thrust(FIXED_DT) {
            energy.advance(FIXED_DT);
        }
        assert!(energy.drained(), "an emptied bar did not lock out");
        // Not at a tenth, not at nine tenths: the lockout is a *full bar*, and
        // a booster that came back partway up would make hitting zero cheap.
        for _ in 0..(FILL_SECONDS / FIXED_DT) as usize - 1 {
            energy.advance(FIXED_DT);
            assert!(!energy.thrust(FIXED_DT), "it flew inside the lockout");
            assert!(energy.drained(), "it came back before it was full");
        }
        coast(&mut energy, 1.0);
        assert_eq!(energy.level, 1.0);
        assert!(!energy.drained(), "a full bar is still locked out");
        assert!(energy.thrust(FIXED_DT), "and it never flew again");
    }

    #[test]
    fn nothing_inside_the_lockout_can_stop_it_ending() {
        // The three ways the bar is normally held, all applied every step of a
        // lockout: the booster held down, the skate held down, and the trigger.
        // A lockout whose only exit is a full bar must contain no way of
        // stopping the bar filling, or the player waits for ever.
        let mut energy = part_full(0.0);
        energy.drained = true;
        for _ in 0..(FILL_SECONDS / FIXED_DT).ceil() as usize + 2 {
            energy.advance(FIXED_DT);
            energy.thrust(FIXED_DT);
            energy.stall(FIXED_DT);
            energy.shoot();
        }
        assert!(
            !energy.drained(),
            "the bar was held at {} and never came back",
            energy.level
        );
    }

    #[test]
    fn the_grounded_skate_holds_the_bar_without_emptying_it() {
        let mut energy = part_full(0.5);
        // The key was already down when the step began, which is the state
        // every step of a held skate after the first one is in.
        energy.stall(FIXED_DT);
        for _ in 0..90 {
            energy.advance(FIXED_DT);
            energy.stall(FIXED_DT);
        }
        assert_eq!(energy.level, 0.5, "three seconds of skating cost nothing");
        // One step to pay off the last stall, and then it is climbing again.
        energy.advance(FIXED_DT);
        energy.advance(FIXED_DT);
        assert!(energy.level > 0.5, "and letting go starts it immediately");
    }

    #[test]
    fn a_shot_costs_a_hundredth_and_a_second() {
        let mut energy = part_full(0.5);
        energy.shoot();
        assert!((energy.level - 0.49).abs() < 1e-6);
        coast(&mut energy, SHOT_DELAY);
        assert!((energy.level - 0.49).abs() < 1e-6, "it climbed while held");
        coast(&mut energy, 1.0);
        assert!(energy.level > 0.49);
    }

    #[test]
    fn a_shot_cannot_shorten_the_hold_a_burn_stamped() {
        let mut energy = Energy::new();
        energy.thrust(FIXED_DT);
        energy.advance(FIXED_DT);
        let held = energy.delay;
        energy.shoot();
        assert!(energy.delay >= held, "the shot cut the hold short");
    }
}
