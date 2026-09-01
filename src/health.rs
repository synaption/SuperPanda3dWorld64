//! Hit points: what an actor can take, what a blow costs it, and the two ways
//! either is shown on the screen.
//!
//! Before this module a fight was decided by contact alone -- a swing, a stomp
//! or a bullet despawned whatever it reached, and the player carried three
//! hearts that a touch took one of. That makes every creature identical to
//! every other one and every weapon identical to every other weapon: the only
//! question a fight could ask was whether you connected.
//!
//! So a blow now spends a *number* against a pool, and the two tables below are
//! the whole of the design. Read across them and the shape of every fight in
//! the game falls out:
//!
//! | | health | hits to kill it |
//! |---|---|---|
//! | slime | 5 | one of anything |
//! | ant | 10 | one of the player's, two of a Mario's |
//! | Mario | 20 | ten slime touches, seven ant ones |
//! | player | 100 | fifty slime touches, thirty-four ant ones |
//!
//! The asymmetry is deliberate and it is what makes a squad worth having. A
//! Mario cannot kill an ant on its own swing and has to land two, which is long
//! enough for the ant to be hitting back -- so a lone Mario sent at a nest
//! loses, and four of them do not. The player one-shots everything, and what
//! threatens him is arithmetic rather than any single enemy: nothing on the
//! field takes more than three points off him, and a crowd takes thirty.
//!
//! **Health lives in a component rather than in each actor's own controller.**
//! It was a `u8` on [`crate::player::Controller`] and nothing else in the game
//! had any, which meant the player's health and an enemy's death were two
//! unrelated mechanisms and a health bar could only ever be drawn over one of
//! them. One component on all three kinds of actor is what lets [`draw_unit_bars`]
//! be a single system that does not know or care what it is drawing over.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{display::DisplaySettings, enemy::Enemy, player::Player, squad::Ally};

/// What each actor can take before it goes down.
pub const PLAYER_HEALTH: i32 = 100;

/// What a red ball puts back.
///
/// A quarter of Luna's pool, which is worth going out of your way for and is
/// not a full heal: three of them undo a bad fight and one of them does not.
/// A Mario's whole pool is smaller than this, so one always mends a Mario
/// completely -- which is the right shape for a unit you have eight of and no
/// way to heal individually.
pub const MEDKIT_HEAL: i32 = 25;

/// How near somebody who needs it a red ball starts drifting towards them, how
/// near it has to get to be absorbed, how far up the body it aims, and how hard
/// it is pulled.
///
/// **A red ball is no longer spent the instant somebody is within reach of
/// it**, and the difference is a moment. It used to vanish at two metres: the
/// ball was there, and then the number went up, with nothing on the screen
/// joining the two. Now it notices, drifts in, and is absorbed on contact --
/// which is half a second of the thing visibly coming to you, and is also what
/// makes one dropped in the middle of a scrap readable as an arrival rather
/// than as something that quietly disappeared.
///
/// The lure is generous next to [`crate::nuclonium::PICKUP_RANGE`] and on
/// purpose: nobody is ever *sent* for one of these, so it has to be had by
/// fighting near it. The touch is small, because it is meant to be the moment
/// the ball reaches the body rather than a second reach in disguise.
///
/// [`MEDKIT_HEIGHT`] is roughly chest height. A ball that aimed at a body's
/// origin would dive into the grass on its way in, since that origin is between
/// the feet.
///
/// The pull is stiffer than [`crate::nuclonium`]'s escort spring: a train
/// following Luna should lag and swing, and a pickup should not -- something
/// that dawdled on its way in would read as bait rather than as a medkit.
pub const MEDKIT_LURE: f32 = 4.5;
pub const MEDKIT_TOUCH: f32 = 0.75;
pub const MEDKIT_HEIGHT: f32 = 1.0;
pub const MEDKIT_PULL: f32 = 7.0;
pub const MARIO_HEALTH: i32 = 20;
pub const ANT_HEALTH: i32 = 10;
pub const SLIME_HEALTH: i32 = 5;

/// What each actor's blow costs whatever it lands on.
///
/// The player's is one number rather than one per weapon: the sword, the stomp
/// and the pistol all spend ten. That is a decision about the *game* rather
/// than an omission -- a gun that outdamaged the sword would make the sword a
/// thing you swap away from and never back to, and this port has two weapons.
/// See [`crate::weapon::Spec`] for where a per-weapon figure would go if one
/// is ever wanted.
pub const PLAYER_DAMAGE: i32 = 10;
pub const MARIO_DAMAGE: i32 = 5;
pub const ANT_DAMAGE: i32 = 3;
pub const SLIME_DAMAGE: i32 = 2;

/// What the things that stand still can take.
///
/// A structure is not an actor and its number does not come off the same
/// reasoning. What decides it is *how long it takes to lose one while you are
/// somewhere else*, because that is the only thing a destructible building
/// changes about the game: a pylon is a thing you planted and walked away from,
/// and the question the crowd is asking of it is whether you get back in time.
///
/// The two are attacked by different things at different rates, so they are two
/// unrelated numbers rather than a scale.
///
/// **A pylon is only ever attacked by the crowd**, which takes turns -- one blow
/// every [`crate::structure::RECOVERY`], however many of them arrived. Forty ant
/// blows is about fourteen seconds of being stood on: long enough to hear it
/// happening and get back across the map on the jetpack, short enough that
/// ignoring it costs you the mast and the beams through it. Forty was five
/// seconds, and five seconds is not a warning, it is an outcome.
///
/// **A warp pipe is only ever attacked by discrete blows** -- the sword, a
/// Mario's fist, a bullet -- which all land in full. Six of the player's own
/// swings, so clearing a nest is a thing you commit to rather than something
/// that happens in passing, and about a dozen of a squad's punches, so sending
/// four Marios at one is a plan.
pub const PYLON_HEALTH: i32 = 120;
pub const WARP_PIPE_HEALTH: i32 = 60;

/// The relationships between the two tables that the fights are built on.
///
/// A compile error rather than a test, because these are constants and the
/// thing worth catching is somebody *editing one of them* -- which is a change
/// to how every fight in the game goes, and ought to stop the build rather than
/// wait for a test run to say so. Each line is a sentence about play, and
/// changing a number is fine as long as the sentence is still true.
const _: () = {
    // A Mario cannot delete an ant. It lands, the ant is still there, and the
    // ant is hitting back in the gap -- which is what makes a squad worth more
    // than the sum of its Marios and a lone one worth sending nowhere.
    assert!(MARIO_DAMAGE < ANT_HEALTH);
    assert!(MARIO_DAMAGE * 2 >= ANT_HEALTH);
    // But it does pop a slime, so a squad clears the weak half of a field
    // without the player.
    assert!(MARIO_DAMAGE >= SLIME_HEALTH);
    // The player one-shots everything the game places. What threatens him is
    // arithmetic across a crowd rather than any single creature.
    assert!(PLAYER_DAMAGE >= ANT_HEALTH && PLAYER_DAMAGE >= SLIME_HEALTH);
    // And that crowd has to be a real crowd: no enemy is worth more than a few
    // percent of him, which is the difference between a hundred points and the
    // three hearts this replaced.
    assert!(PLAYER_HEALTH / SLIME_DAMAGE > 10);
    assert!(PLAYER_HEALTH / ANT_DAMAGE > 10);
    // Neither building falls to one blow of anything, or a mast planted in
    // front of a pipe would be gone before its beams were drawn.
    assert!(PYLON_HEALTH > PLAYER_DAMAGE && WARP_PIPE_HEALTH > PLAYER_DAMAGE);
    // A mast is defensible: dozens of the crowd's blows, and the crowd takes
    // turns, so what it survives is measured in seconds rather than in how many
    // of them turned up.
    assert!(PYLON_HEALTH / ANT_DAMAGE >= 30);
    // A nest is an objective: several swings and not one, so knocking one down
    // is a thing you decide to do.
    assert!(WARP_PIPE_HEALTH / PLAYER_DAMAGE >= 5);
};

/// An actor's pool of hit points, and what it started with.
///
/// The maximum travels with the current value because everything that draws one
/// of these wants the *fraction*, and a bar that knows only the number left has
/// no width to draw. Keeping both here rather than looking the maximum back up
/// from the actor's kind is also what lets [`draw_unit_bars`] treat a slime, a Mario
/// and the player as the same thing.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    /// Never negative and never above [`Self::max`]: [`Self::hurt`] and
    /// [`Self::refill`] are the only things that write it, and neither can
    /// leave it outside that range.
    pub current: i32,
    pub max: i32,
}

impl Health {
    /// A full pool of `max` points.
    pub fn new(max: i32) -> Self {
        Self { current: max, max }
    }

    /// Spends `amount` against the pool and reports whether that finished it.
    ///
    /// The return value is what every caller actually wants -- "do I despawn
    /// this now" -- and having it here rather than a separate [`Self::dead`]
    /// call at each site is what stops a hit that kills something being
    /// resolved as a hit that did not.
    pub fn hurt(&mut self, amount: i32) -> bool {
        self.current = (self.current - amount).max(0);
        self.dead()
    }

    /// Back to full, which is what respawning is.
    pub fn refill(&mut self) {
        self.current = self.max;
    }

    /// Puts `amount` back, and reports whether any of it was wanted.
    ///
    /// The report is the useful half. A medkit is spent by whoever picks it up,
    /// and something at full health picking one up spends it on nothing -- so
    /// the thing that decides *whether* to pick one up asks this, and a body
    /// that needs no mending walks past it and leaves it for one that does.
    pub fn mend(&mut self, amount: i32) -> bool {
        if self.current >= self.max {
            return false;
        }
        self.current = (self.current + amount).min(self.max);
        true
    }

    pub fn dead(&self) -> bool {
        self.current <= 0
    }

    /// How full the pool is, from zero to one, for whatever is drawing it.
    ///
    /// Guards the maximum rather than trusting it: a `Health` built with a zero
    /// maximum is a bug somewhere else, and a division by it here would be a
    /// `NaN` width on a UI node, which Bevy lays out as a node that is never
    /// seen again.
    pub fn fraction(&self) -> f32 {
        if self.max <= 0 {
            return 0.0;
        }
        (self.current as f32 / self.max as f32).clamp(0.0, 1.0)
    }
}

/// Lands `amount` on a target that may or may not have any hit points, and
/// reports whether it should now be despawned.
///
/// **A target with no [`Health`] at all dies to the first thing that touches
/// it.** That is exactly what every fight in this game did before this module
/// existed, and it is what an actor assembled by hand -- in a test, or at a
/// spawn site that has not been taught about hit points yet -- still gets.
/// Failing closed the other way would be worse in both places: a test would
/// hang on an immortal slime, and a missing component in the game would be a
/// creature nothing could kill.
pub fn strike(health: Option<&mut Health>, amount: i32) -> bool {
    match health {
        Some(health) => health.hurt(amount),
        None => true,
    }
}

impl BarFades {
    /// Makes last frame's readings the ones to read from, and empties this
    /// frame's ready to be refilled.
    ///
    /// A swap rather than a fresh map, so the allocation goes round and round
    /// instead of being made and dropped on every rendered frame.
    fn turn_over(&mut self) {
        std::mem::swap(&mut self.now, &mut self.spare);
        self.now.clear();
    }
}

/// Marks the player's own bar, the one that is always on the screen.
#[derive(Component)]
pub struct PlayerBarFill;

/// The number written across the player's bar.
#[derive(Component)]
pub struct PlayerBarLabel;

/// One of the floating bars drawn over a unit's head.
///
/// Carries nothing: which unit a bar is showing is decided fresh every frame by
/// [`draw_unit_bars`], and what matters about these entities is that they are
/// *made once* -- see [`spawn`].
#[derive(Component)]
pub struct UnitBar;

/// The coloured part of a unit bar, which is a child of the [`UnitBar`] frame.
#[derive(Component)]
pub struct UnitBarFill;

/// How many unit bars exist.
///
/// A fixed pool rather than a bar spawned per creature, because the field this
/// draws over is thousands of enemies and a UI node per enemy is thousands of
/// nodes laid out every frame whether or not any of them is on the screen. The
/// pool is filled from the units nearest the camera and everything past it is
/// hidden, so the cost of the setting is constant rather than proportional to
/// how bad an idea turning it on was.
const UNIT_BARS: usize = 96;

/// How far from the camera a unit still gets a bar.
///
/// Past this the bar is a couple of pixels wide and reads as dirt on the
/// screen. It is also the cheap rejection that keeps the sort below over a
/// handful of candidates rather than over the whole field.
const BAR_RANGE: f32 = 45.0;

/// The player's bar, in window pixels.
pub const PLAYER_BAR: Vec2 = Vec2::new(260.0, 20.0);

/// How far the corner's stack of player bars sits from the window's edges, and
/// how much daylight there is between two of them.
///
/// Shared with [`crate::energy`] rather than written twice, because the two
/// bars are one stack: the energy bar is pinned to the margin and this one is
/// pushed up over it by exactly the height of the bar below plus the gap. Two
/// copies of the number is two chances for a redesign to leave them overlapping
/// or floating apart.
pub const HUD_MARGIN: f32 = 16.0;
pub const BAR_GAP: f32 = 6.0;

/// What a unit bar is drawn in at full strength: the dark ground behind the
/// fill, and the hard edge around it.
///
/// Named rather than written at the spawn site because the fade has to scale
/// them every frame, and a fade that scaled a *faded* colour would dim the bar
/// towards nothing over a few seconds instead of settling.
const BAR_GROUND: Color = Color::srgba(0.02, 0.02, 0.04, 0.75);
const BAR_EDGE: Color = Color::srgba(0.0, 0.0, 0.0, 0.8);

/// A unit's bar, in window pixels. Small and wide: it has to read as a quantity
/// at a glance from across a lawn without covering the creature it belongs to.
const UNIT_BAR: Vec2 = Vec2::new(38.0, 5.0);

/// How long a bar takes to come up to full strength.
///
/// Bars appear in bulk -- turn the setting on, swing the camera across the
/// lawn, walk into range of a nest -- and appearing instantly reads as the
/// screen glitching rather than as information arriving. A second is long
/// enough to be seen as a fade and short enough that a bar you turned to look
/// at is legible by the time your eye reaches it.
const FADE_SECONDS: f32 = 1.0;

/// How far above an actor's head its bar floats, in world units.
const BAR_CLEARANCE: f32 = 0.2;

/// How tall a Mario is *drawn*, which is not [`crate::player::PLAYER_HEIGHT`].
///
/// That constant is the collision capsule -- what the walls push out of and
/// what the shadow is projected for -- and it is a good deal taller than the
/// model standing inside it, because Mario is a stumpy character and a capsule
/// that fitted him would let his hat through a ceiling. Hang a bar off the
/// capsule and it floats a whole body-height over his head with a gap of lawn
/// between.
///
/// So this is measured off the screen rather than taken from the game's own
/// numbers, and it cannot come from the glTF either: the bind-pose bounds in
/// `mario.glb` are in skin space under a rotated armature, so its longest axis
/// is the one running through the character sideways. Enemies have no such
/// problem and do not use this -- [`crate::enemy::Kind::body`] measures each of
/// them properly, and a bar over a slime is hung off that.
const MARIO_HEAD: f32 = 1.25;

/// The colour a pool of this fullness is drawn in.
///
/// Stepped rather than interpolated: three colours is what a console-era bar
/// had, and a continuous ramp spends most of its range on shades nobody can
/// name. The steps also mean the bar *changes* at a threshold, which is the
/// moment worth noticing.
///
/// **All three are red now.** It used to run green through amber to red, which
/// is the arcade convention and was fine while there was one bar on the screen.
/// There are two: [`crate::energy`] sits underneath and it is the green one, so
/// a health bar that started green would have the corner showing two green bars
/// at full and asking the player which was which in the half-second he has to
/// look. Health owns red, energy owns green, and neither ever wears the other's
/// colour at any fullness.
///
/// The ramp survives the move -- light red at full, deepening twice on the way
/// down -- because the thresholds are the useful part and losing them would
/// leave the width as the only thing the bar says.
fn fill_colour(fraction: f32) -> Color {
    if fraction > 0.55 {
        Color::srgb(0.98, 0.55, 0.52)
    } else if fraction > 0.25 {
        Color::srgb(0.93, 0.32, 0.28)
    } else {
        Color::srgb(0.80, 0.10, 0.10)
    }
}

/// Builds the player's bar and the whole unit-bar pool.
///
/// Called from the game's startup alongside the rest of the UI. The pool is
/// built here, hidden, and never grows: [`draw_unit_bars`] only ever moves these
/// nodes and changes their visibility.
pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(HUD_MARGIN),
                // Above the energy bar rather than at the margin itself: see
                // [`HUD_MARGIN`], and [`crate::energy::spawn`] for the bar that
                // is down there instead.
                bottom: Val::Px(HUD_MARGIN + crate::energy::ENERGY_BAR.y + BAR_GAP),
                width: Val::Px(PLAYER_BAR.x),
                height: Val::Px(PLAYER_BAR.y),
                border: UiRect::all(Val::Px(2.0)),
                // Clipped, so the fill inside it cannot draw over the border
                // when it is full.
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.04, 0.72)),
            BorderColor::all(Color::srgba(0.85, 0.88, 0.95, 0.65)),
            GlobalZIndex(15),
        ))
        .with_children(|bar| {
            bar.spawn((
                PlayerBarFill,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(fill_colour(1.0)),
            ));
            // Over the fill rather than beside the bar: the number and the
            // quantity it describes want to be one thing to look at.
            bar.spawn((
                PlayerBarLabel,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(0.05, 0.05, 0.08, 0.95)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(8.0),
                    top: Val::Px(1.0),
                    ..default()
                },
            ));
        });
    for _ in 0..UNIT_BARS {
        commands
            .spawn((
                UnitBar,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(UNIT_BAR.x),
                    height: Val::Px(UNIT_BAR.y),
                    border: UiRect::all(Val::Px(1.0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                // Faded right out to begin with. A bar's first frame on the
                // screen is the first frame of its fade, and these are spawned
                // long before any of them is shown.
                BackgroundColor(BAR_GROUND.with_alpha(0.0)),
                BorderColor::all(BAR_EDGE.with_alpha(0.0)),
                GlobalZIndex(12),
                Visibility::Hidden,
            ))
            .with_children(|bar| {
                bar.spawn((
                    UnitBarFill,
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(fill_colour(1.0).with_alpha(0.0)),
                ));
            });
    }
}

/// Writes the player's own bar every frame.
pub fn draw_player_bar(
    player: Query<&Health, With<Player>>,
    mut fill: Query<(&mut Node, &mut BackgroundColor), With<PlayerBarFill>>,
    mut label: Query<&mut Text, With<PlayerBarLabel>>,
) {
    let Ok(health) = player.single() else {
        return;
    };
    if let Ok((mut node, mut colour)) = fill.single_mut() {
        node.width = Val::Percent(health.fraction() * 100.0);
        *colour = BackgroundColor(fill_colour(health.fraction()));
    }
    if let Ok(mut text) = label.single_mut() {
        **text = format!("{} / {}", health.current, health.max);
    }
}

/// One unit that has earned a bar this frame.
struct Candidate {
    /// Which unit it is, which is the key its fade is remembered under.
    unit: Entity,
    /// Where its bar goes, in the window's own pixels: projected and stretched
    /// already, because whether it is on the screen at all is part of whether
    /// it is a candidate.
    at: Vec2,
    /// How far off it is, which is what the pool is handed out by.
    away: f32,
    fraction: f32,
    /// How far through its fade-in it is, from nothing to full strength.
    fade: f32,
}

/// How far through its fade each unit currently on the screen is.
///
/// **Kept per unit rather than per bar, which is the whole reason this is a
/// resource and not a field on [`UnitBar`].** The bars are a pool handed out
/// fresh every frame in distance order, so the bar entity showing a given
/// creature changes whenever two creatures swap places in that order -- which,
/// in a crowd, is constantly. A fade stored on the bar would therefore belong
/// to the slot rather than to the creature: one unit walking past another would
/// restart both their fades, and a unit arriving would fade in a bar somewhere
/// else on the screen while its own popped in. Keyed by the creature, the fade
/// follows the creature and the pool stays free to shuffle.
///
/// A unit that drops off the list loses its entry and fades in again if it
/// comes back. That is right for the case it exists for -- something walking
/// back into range is arriving -- and the cost is that a creature hovering
/// exactly on the screen's edge re-fades as it crosses.
#[derive(Resource, Default)]
pub struct BarFades {
    /// This frame's, by unit.
    now: HashMap<Entity, f32>,
    /// Last frame's. Kept only so its allocation can be reused rather than
    /// dropped and made again: this runs every rendered frame.
    spare: HashMap<Entity, f32>,
}

/// Puts the floating bars over the units nearest the camera, or hides every one
/// of them when the setting is off.
///
/// **The projection has two steps rather than one, and the second is the
/// non-obvious half.** The world is not drawn to the window: it is drawn to an
/// image of the player's chosen size and stretched over the window afterwards
/// -- see [`crate::display`]. So the world camera's `world_to_viewport` answers
/// in *that image's* pixels, and the UI this positions is laid out in the
/// window's. At a render scale of four they differ by four, which is a bar
/// sitting in the top-left quarter of the screen while the creature it belongs
/// to walks about in the middle. The ratio is uniform because the image always
/// follows the window's aspect exactly, so one scalar puts it right.
///
/// **It runs in `PostUpdate`, between transform propagation and the UI layout,
/// and both halves of that are load-bearing.** A projection is only as current
/// as the camera it projects through, and `GlobalTransform` is written by
/// `TransformSystems::Propagate` -- so run in `Update`, as this first was, it
/// aims every bar through *last* frame's camera while placing it over this
/// frame's creature. Standing still that is invisible; moving, every bar on the
/// screen sits a consistent shove away from the head it belongs to, in the
/// direction the camera is travelling. The other end is `UiSystems::Layout`,
/// which reads the `left` and `top` written here: after it, they would not be
/// laid out until the following frame, which is the same lag again.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn draw_unit_bars(
    settings: Res<DisplaySettings>,
    time: Res<Time>,
    mut fades: ResMut<BarFades>,
    camera: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    // `GlobalTransform` rather than `Transform` for the same reason the camera
    // is: this is the pose the frame is being drawn from, and a bar has to be
    // placed against what is on the screen rather than against what the
    // simulation last wrote.
    enemies: Query<(Entity, &GlobalTransform, &Health, &Enemy)>,
    allies: Query<(Entity, &GlobalTransform, &Health), With<Ally>>,
    // The masts and the nests. A building being worn down is the one thing in
    // this game that happens somewhere the player is not, so a bar over it is
    // not decoration -- it is how you find out a pylon is going before it goes.
    buildings: Query<(
        Entity,
        &GlobalTransform,
        &Health,
        &crate::structure::Structure,
    )>,
    mut bars: Query<
        (
            &mut Node,
            &mut Visibility,
            &mut BackgroundColor,
            &mut BorderColor,
            &Children,
        ),
        With<UnitBar>,
    >,
    mut fills: Query<(&mut Node, &mut BackgroundColor), (With<UnitBarFill>, Without<UnitBar>)>,
) {
    let mut hide_everything = !settings.unit_health_bars;
    let Ok((camera, eye)) = camera.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    // The image the world was drawn into, which is what the projection below
    // answers in.
    let internal = camera.logical_viewport_size().unwrap_or(Vec2::ZERO);
    if internal.x <= 0.0 || internal.y <= 0.0 {
        hide_everything = true;
    }
    let mut wanted: Vec<Candidate> = Vec::new();
    if !hide_everything {
        // Window pixels per image pixel. See the note above.
        let stretch = window.width() / internal.x;
        let screen = window.size();
        let from = eye.translation();
        // **Two points rather than one, and the second is what keeps a bar
        // looking attached.** A perspective camera does not project the world's
        // vertical onto the screen's: pitch it downwards, as this game's camera
        // is most of the time, and world-vertical lines fan out from a
        // vanishing point below the screen. Project only the head and the bar
        // is *correct* -- it is exactly above the creature in the world -- and
        // reads as belonging to nothing, because on a creature near the corner
        // of the screen it sits the better part of a body-width to one side.
        //
        // So the height comes from the head and the column comes from the feet:
        // the bar is over the creature on the screen, which is the question a
        // player is asking of it. The feet fall back to the head on the one
        // frame they cannot be projected -- a creature straddling the near
        // plane, which is a creature the camera is inside.
        let mut consider = |unit: Entity, base: Vec3, head: Vec3, away: f32, fraction: f32| {
            // Behind the camera and past the far plane are both an `Err` from
            // Bevy, so what is left to reject here is a head that projects to a
            // real point off the sides of the window.
            let Ok(top) = camera.world_to_viewport(eye, head) else {
                return;
            };
            let column = camera
                .world_to_viewport(eye, base)
                .map(|at| at.x)
                .unwrap_or(top.x);
            let at = Vec2::new(column, top.y) * stretch;
            if at.x < -UNIT_BAR.x
                || at.y < -UNIT_BAR.y
                || at.x > screen.x + UNIT_BAR.x
                || at.y > screen.y + UNIT_BAR.y
            {
                return;
            }
            wanted.push(Candidate {
                unit,
                at,
                away,
                fraction,
                // Filled in below, once the list has been cut down to the units
                // that actually get a bar.
                fade: 0.0,
            });
        };
        for (unit, at, health, enemy) in &enemies {
            let foot = at.translation();
            let away = foot.distance(from);
            if away > BAR_RANGE {
                continue;
            }
            // Up the creature's own axis rather than up the world's: a crawler
            // stuck to a ceiling is upside down, and its head is the end of it
            // its model is standing on.
            let up = at.rotation() * Vec3::Y;
            consider(
                unit,
                foot,
                foot + up * (enemy.kind.body().1 + BAR_CLEARANCE),
                away,
                health.fraction(),
            );
        }
        for (unit, at, health) in &allies {
            let foot = at.translation();
            let away = foot.distance(from);
            if away > BAR_RANGE {
                continue;
            }
            consider(
                unit,
                foot,
                foot + Vec3::Y * (MARIO_HEAD + BAR_CLEARANCE),
                away,
                health.fraction(),
            );
        }
        for (unit, at, health, structure) in &buildings {
            // A building at full health has nothing to say. The pool is a fixed
            // pool that only ever falls, unlike a creature's -- creatures are
            // spawned, fight and die inside the bar's own fade -- so without
            // this every mast on the map wears a full green bar forever and the
            // pool of ninety-six is spent on scenery.
            if health.current >= health.max {
                continue;
            }
            let foot = at.translation();
            let away = foot.distance(from);
            if away > BAR_RANGE {
                continue;
            }
            // Over the top of it, off the size it was placed with rather than a
            // height written here. See [`crate::structure::head`].
            consider(
                unit,
                foot,
                crate::structure::head(foot, structure),
                away,
                health.fraction(),
            );
        }
        // Nearest first, so a pool smaller than the crowd spends itself on the
        // creatures whose bars can actually be read. Without the sort, which
        // units got one would be query order -- which is stable enough to look
        // deliberate and arbitrary enough to be wrong.
        wanted.sort_by(|a, b| a.away.total_cmp(&b.away));
        wanted.truncate(UNIT_BARS);
    }
    // Each surviving unit picks its fade up from where it left it last frame and
    // carries it a frame further. Done after the truncation, so a unit crowded
    // out of the pool is one that gets no bar and banks no progress towards one.
    //
    // The two maps are swapped rather than one of them rebuilt: last frame's
    // becomes the thing this frame reads from, this frame's is refilled into the
    // allocation last frame gave back, and a unit that has gone simply never
    // gets copied across. That is the pruning, and it costs nothing.
    fades.turn_over();
    let step = time.delta_secs() / FADE_SECONDS;
    for candidate in &mut wanted {
        candidate.fade = advanced(fades.spare.get(&candidate.unit).copied(), step);
        fades.now.insert(candidate.unit, candidate.fade);
    }
    let mut next = wanted.iter();
    for (mut node, mut visibility, mut ground, mut edge, children) in &mut bars {
        let Some(candidate) = next.next() else {
            *visibility = Visibility::Hidden;
            continue;
        };
        *visibility = Visibility::Visible;
        // Centred on the head rather than hung off its left corner, and sat
        // just above it rather than across it.
        node.left = Val::Px(candidate.at.x - UNIT_BAR.x * 0.5);
        node.top = Val::Px(candidate.at.y - UNIT_BAR.y);
        // All three of a bar's colours fade together. Fading only the fill
        // would bring the bar up as a black box that fills in, which is a
        // different and worse animation.
        *ground = BackgroundColor(BAR_GROUND.with_alpha(BAR_GROUND.alpha() * candidate.fade));
        *edge = BorderColor::all(BAR_EDGE.with_alpha(BAR_EDGE.alpha() * candidate.fade));
        for child in children.iter() {
            if let Ok((mut fill, mut colour)) = fills.get_mut(child) {
                fill.width = Val::Percent(candidate.fraction * 100.0);
                *colour =
                    BackgroundColor(fill_colour(candidate.fraction).with_alpha(candidate.fade));
            }
        }
    }
}

/// A fade one step further on, from wherever it had got to.
///
/// `None` is a unit that was not on the screen last frame, and starts from
/// nothing. Its own function because it is the whole of the animation and the
/// only arithmetic in it worth a test.
fn advanced(previous: Option<f32>, step: f32) -> f32 {
    (previous.unwrap_or(0.0) + step).clamp(0.0, 1.0)
}

/// A red ball lying about: hit points for whoever runs over it.
///
/// A marker and nothing else. Everything about *being* a ball -- the sphere,
/// the glow, the float, the wake -- belongs to [`crate::nuclonium::Orb`], which
/// draws the green ones the same way. What this component says is only what
/// happens when somebody touches it, which is the entire difference between the
/// two kinds of drop.
#[derive(Component)]
pub struct Medkit;

/// A red ball on its way to somebody, and who that is.
///
/// The medkit's half of [`crate::nuclonium::Held::Following`], and it exists
/// for the same reason that does: the thing that *decides* a ball is coming to
/// you runs on the fixed step, and the thing that moves it has to run per drawn
/// frame or the ball judders in behind a player who does not. So this is the
/// decision, written down where [`crate::nuclonium::swim`] can act on it every
/// frame until [`mend`] takes it away again.
#[derive(Component)]
pub struct Drawn {
    pub toward: Entity,
}

/// Everything a medkit can be picked up by: Luna, and her Marios.
///
/// A named type for clippy's sake. The `Without` is load-bearing as well as
/// tidy -- a medkit has a `Transform` too, and two `Transform` queries in one
/// system have to name each other or the schedule refuses to build.
type Bodies<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Transform, &'static mut Health),
    (Without<Medkit>, Or<(With<Player>, With<Ally>)>),
>;

/// Whether a red ball at `kit` is near enough `body` to start drifting to it.
///
/// Flat, with a band above and below, exactly as
/// [`crate::nuclonium::within_reach`] is and for the same reason: a ball
/// floats, and a rule measured straight through the air is a rule you can stand
/// underneath and not satisfy.
pub fn lured(kit: Vec3, body: Vec3) -> bool {
    let apart = kit - body;
    Vec3::new(apart.x, 0.0, apart.z).length() <= MEDKIT_LURE
        && apart.y <= crate::player::PLAYER_HEIGHT + 1.0
        && apart.y >= -1.6
}

/// Hands out the red balls: notices, draws in, absorbs.
///
/// Luna and her Marios alike, because a squad that cannot be healed is a squad
/// that is spent after one bad fight, and there is nothing else in the game
/// that puts a Mario's hit points back. A kit picks the nearest body that any
/// of it would do some good to, so one dropped between a hurt Mario and a
/// healthy Luna goes to the Mario.
///
/// **Something at full health leaves one alone**, and now does so without ever
/// having touched it: a body already full is not a candidate, so it is not
/// something a kit drifts towards and then declines. A red ball on the grass
/// with nobody hurt near it is inert, and is still there after the fight that
/// makes you want it.
///
/// Only the decisions are here. The drifting itself is
/// [`crate::nuclonium::swim`] -- see there for why a follow that is solved on
/// the fixed step is a follow the player can see stepping.
pub fn mend(
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut kits: Query<
        (
            Entity,
            &Transform,
            &mut crate::nuclonium::Orb,
            Option<&Drawn>,
        ),
        With<Medkit>,
    >,
    mut bodies: Bodies,
) {
    for (kit, at, mut orb, drawn) in &mut kits {
        // The nearest body this would be worth anything to. Measured to where
        // the ball is aimed rather than to the body's feet, so "nearest" means
        // the same thing here as the absorb below means by it.
        let wanted = bodies
            .iter()
            .filter(|(_, body, health)| {
                health.current < health.max && lured(at.translation, body.translation)
            })
            .min_by(|a, b| {
                let reach = |body: &Transform| {
                    (body.translation + Vec3::Y * MEDKIT_HEIGHT).distance_squared(at.translation)
                };
                reach(a.1).total_cmp(&reach(b.1))
            })
            .map(|(who, body, _)| (who, body.translation + Vec3::Y * MEDKIT_HEIGHT));
        let Some((who, reach)) = wanted else {
            // Nobody near it needs it any more -- healed by another kit, or
            // walked off, or killed. It stops chasing and floats where it got
            // to, which is where it bobs from now on: `Orb::settle` exists so
            // that handover is a single call rather than a field poked from
            // another module.
            if drawn.is_some() {
                commands.entity(kit).remove::<Drawn>();
                orb.settle(at.translation.y);
            }
            continue;
        };
        if at.translation.distance(reach) <= MEDKIT_TOUCH {
            if let Ok((_, _, mut health)) = bodies.get_mut(who) {
                health.mend(MEDKIT_HEAL);
            }
            commands.entity(kit).despawn();
            // The weapon-swap sound, borrowed: it is the one noise in the set
            // that already means "you now have this", and a pickup with no
            // sound at all reads as a pickup that did not happen.
            sounds.push_at(crate::audio::Sfx::Draw, at.translation);
            continue;
        }
        // Re-pointed rather than left on its first choice, so a kit halfway to
        // Luna turns for the Mario that stumbled in front of it bleeding.
        if drawn.is_none_or(|drawn| drawn.toward != who) {
            commands.entity(kit).insert(Drawn { toward: who });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_clamps_at_both_ends() {
        let mut health = Health::new(10);
        assert!(!health.hurt(3));
        assert_eq!(health.current, 7);
        health.refill();
        assert_eq!(health.current, 10, "a refill is a whole pool");
        assert!(health.hurt(999), "an overkill is still a kill");
        assert_eq!(health.current, 0, "and does not go negative");
        assert_eq!(health.fraction(), 0.0);
    }

    #[test]
    fn a_target_without_hit_points_dies_to_the_first_hit() {
        assert!(strike(None, 1), "the old behaviour is the fallback");
        let mut health = Health::new(SLIME_HEALTH);
        assert!(
            strike(Some(&mut health), PLAYER_DAMAGE),
            "the player's blow finishes a slime"
        );
    }

    #[test]
    fn a_fade_starts_from_nothing_and_settles_at_full() {
        // A unit that was not on the screen last frame starts from nothing
        // rather than from whatever the bar it happens to be handed was showing.
        assert_eq!(advanced(None, 0.0), 0.0);
        // A second's worth of steps arrives at full strength, whatever size the
        // steps were: this is what makes the fade a *duration* rather than a
        // rate that a fast machine runs through in a tenth of a second.
        for frames in [4usize, 30, 60, 240] {
            let step = 1.0 / frames as f32;
            let mut fade = 0.0;
            for _ in 0..frames {
                fade = advanced(Some(fade), step);
            }
            assert!(
                (fade - 1.0).abs() < 1e-3,
                "{frames} frames of a second left the fade at {fade}"
            );
        }
        // And it stops there rather than climbing, so a bar that has been on
        // the screen for a minute is drawn exactly as one a second old is.
        assert_eq!(advanced(Some(1.0), 0.5), 1.0);
        // A frame long enough to finish the whole fade does not overshoot --
        // which is the first frame after a level load, every time.
        assert_eq!(advanced(None, 9.0), 1.0);
    }

    #[test]
    fn the_fill_steps_through_three_colours() {
        assert_ne!(fill_colour(1.0), fill_colour(0.4));
        assert_ne!(fill_colour(0.4), fill_colour(0.1));
        assert_eq!(fill_colour(1.0), fill_colour(0.9));
    }
}
