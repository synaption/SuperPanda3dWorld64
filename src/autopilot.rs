//! The flight computer: pick a world, and it flies the crossing.
//!
//! Outer Wilds' autopilot is the model, and its shape survives intact: you
//! choose a destination, the ship kills the sideways drift, burns until the
//! halfway point of what its engines can undo, flips, and brakes -- and the
//! moment you touch the stick it is yours again. What is different here is
//! only the fitting: the "ship" is the jetpack's thrust, the destinations are
//! the two planets and the sun, and the controls are the ones this game
//! already has -- the **Tab picker** aims the X button at the autopilot
//! ([`crate::action::Mode::Autopilot`]), and X locks onto **whatever body the
//! crosshair rests on**: a ray from the camera, tested against each world's
//! disc with a little grace round the rim ([`aimed_at`]). Pressing at empty
//! sky lets go. Aiming, not a menu -- the same gesture every other use of
//! the crosshair in this game already is.
//!
//! The guidance is one rule rather than three phases: at any distance `d`
//! short of where it means to stop, the fastest closing speed the engines
//! can still cancel is `sqrt(2 a d)` -- planned at [`BRAKE_MARGIN`] of the
//! real burn, so the plan is always slightly pessimistic and converges
//! instead of overshooting. The thrust each tick simply pushes the velocity
//! towards `that speed, straight at the target`, which accelerates from
//! rest, kills lateral drift, and turns into the braking burn all by itself
//! the moment the allowed speed falls below the held one. What phase that
//! works out to is published on [`Autopilot::phase`] for the console and the
//! overlay, but nothing steers by it.
//!
//! At a planet the boundary is a handover, not an arrival: the crossing
//! carries a walking pace over the weightless line, and inside it the same
//! computer rides the descent -- the jet against the fall, held to
//! [`descent_limit`] -- until the feet are down, where it lets go for good.
//! Only the sun stops short, a hop over the surface, because the sun has no
//! gravity to hand over to and flying you into a wall is not arriving.
//!
//! Pressing again on the body already locked asks for a **parking orbit**
//! instead: a ring [`ORBIT_ALTITUDE`] above the stop radius, flown at
//! [`ORBIT_SPEED`], held until the next press -- the same body back to a
//! crossing, empty sky to let go.

use crate::{
    console::{ConsoleState, GameTuning},
    gravity::{Gravity, GRAVITY_FADE, GRAVITY_RANGE},
    input::InputState,
    level::{LevelData, Shape},
    orbit::{SolarSystem, SUN_CENTRE, SUN_RADIUS},
    player::{Controller, Player, RenderPose, INFINITE_BURN},
    world::LevelId,
};
use bevy::prelude::*;

/// How far outside the sun's surface the autopilot calls it a day. There is
/// no gravity there to finish the approach, so "arrived" has to leave the
/// last hop to the player rather than parking them touching the wall.
const SUN_ARRIVE: f32 = 40.0;

/// The fraction of the real burn the braking plan is drawn against. Planning
/// with the whole burn leaves no reserve for the plan being a tick stale --
/// the target is orbiting -- and an autopilot that overshoots by a metre a
/// tick is an autopilot oscillating through its destination.
const BRAKE_MARGIN: f32 = 0.85;

/// Corrections smaller than this are not worth a burn: the coast is already
/// the plan, near enough. In metres a second.
const CLOSE_ENOUGH: f32 = 0.5;

/// The pace the crossing carries over a planet's weightless boundary. A plan
/// that reaches the line at zero hangs there, weightless, forever; a walking
/// pace across it puts the body in the gravity that finishes the trip.
const HANDOVER: f32 = 5.0;

/// The descent speed touchdown is aimed at, in metres a second: the floor
/// under [`descent_limit`], so the last metres are a settle and not a hover
/// that never lands.
pub const TOUCHDOWN: f32 = 2.5;

/// How far above the stop radius a parking orbit's ring sits. Clear of the
/// weightless boundary, so the ring never trades the jet for gravity.
const ORBIT_ALTITUDE: f32 = 60.0;

/// The pace a parking orbit is flown at, in metres a second.
const ORBIT_SPEED: f32 = 40.0;

/// The grace round a body's rim the crosshair may miss by and still lock on,
/// in radians. About three and a half degrees: a distant planet subtends a
/// few degrees itself, and demanding a dead-centre hit on a moving target at
/// five kilometres is a test of mouse hardware, not of intent.
const PICK_SLOP: f32 = 0.06;

/// Somewhere the autopilot can be pointed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// A body index into [`SolarSystem::bodies`].
    Planet(usize),
    Sun,
    /// An index into [`crate::world::FIXTURES`]: one of the `test_world`
    /// diagnostic bodies, when the level was loaded with them. Standing
    /// still, which makes them the easiest crossings in the system.
    Fixture(usize),
}

impl Target {
    /// Everything the crosshair can lock, fixtures included -- [`aimed_at`]
    /// skips the ones the level was loaded without.
    pub const ALL: [Target; 6] = [
        Target::Planet(0),
        Target::Planet(1),
        Target::Sun,
        Target::Fixture(0),
        Target::Fixture(1),
        Target::Fixture(2),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Target::Planet(0) => "the first planet",
            Target::Planet(_) => "the second planet",
            Target::Sun => "the sun",
            Target::Fixture(index) => crate::world::FIXTURES[index.min(2)].name,
        }
    }

    /// The short form the lock-on tag wears on screen, where "the first
    /// planet" is a sentence and a HUD wants a callsign.
    pub fn tag(self) -> &'static str {
        match self {
            Target::Planet(0) => "PLANET I",
            Target::Planet(_) => "PLANET II",
            Target::Sun => "SUN",
            Target::Fixture(index) => crate::world::FIXTURES[index.min(2)].tag,
        }
    }

    /// Where the target is *now* -- asked every tick, because everything but
    /// the sun and the fixtures is moving.
    pub fn centre(self, system: &SolarSystem) -> Vec3 {
        match self {
            Target::Planet(index) => system.bodies[index.min(1)].centre,
            Target::Sun => SUN_CENTRE,
            Target::Fixture(index) => crate::world::FIXTURES[index.min(2)].stands_at,
        }
    }

    /// The body's own extent: the disc the crosshair aims at.
    pub fn body_radius(self, planet_radius: f32) -> f32 {
        match self {
            Target::Planet(_) => planet_radius,
            Target::Sun => SUN_RADIUS,
            Target::Fixture(index) => crate::world::FIXTURES[index.min(2)].radius,
        }
    }

    /// The distance from the target's centre the approach plans to be
    /// stationary at.
    pub fn stop_radius(self, planet_radius: f32) -> f32 {
        match self {
            // The weightless boundary: past here the body's own pull is
            // the better pilot, and the movement code hands over to it.
            Target::Planet(_) | Target::Fixture(_) => {
                self.body_radius(planet_radius) + GRAVITY_RANGE + GRAVITY_FADE
            }
            Target::Sun => SUN_RADIUS + SUN_ARRIVE,
        }
    }
}

/// What the burn is currently doing, for the console and the overlay.
/// Steering never reads it -- see the module doc.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Idle,
    Burn,
    Brake,
    /// Holding a parking orbit's ring.
    Orbit,
}

/// What the lock is *for*: flying there, or staying in a ring round it.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Mode {
    #[default]
    Approach,
    /// Hold a circle of radius `hold` round the target.
    Orbit { hold: f32 },
}

/// The autopilot's whole state: where it is pointed, if anywhere, and what
/// its burn is doing about it. `flown` remembers whether this lock has ever
/// burned -- the difference, on the ground, between a trip that is over and
/// one still being lined up: touchdown releases the first and keeps the
/// second.
#[derive(Resource, Default)]
pub struct Autopilot {
    pub target: Option<Target>,
    pub phase: Phase,
    pub mode: Mode,
    pub flown: bool,
}

impl Autopilot {
    /// A fresh lock, pointed at `target` for the crossing.
    pub fn approach(target: Target) -> Self {
        Self {
            target: Some(target),
            ..Self::default()
        }
    }

    pub fn engaged(&self) -> bool {
        self.target.is_some()
    }

    pub fn disengage(&mut self) {
        *self = Self::default();
    }
}

/// The body under the crosshair, if any: the ray from `from` along `along`,
/// tested against every body's disc with [`PICK_SLOP`] of grace round the
/// rim. Where discs overlap -- a planet crossing the sun -- the one the
/// crosshair sits deepest inside wins, which is the near, smaller-looking
/// body whenever the aim is actually on it.
pub fn aimed_at(
    system: &SolarSystem,
    planet_radius: f32,
    fixtures: usize,
    from: Vec3,
    along: Vec3,
) -> Option<Target> {
    let along = along.normalize_or(Vec3::Z);
    let mut best: Option<(f32, Target)> = None;
    for target in Target::ALL {
        // A fixture the level was loaded without is not in the sky to aim at.
        if let Target::Fixture(index) = target {
            if index >= fixtures {
                continue;
            }
        }
        let centre = target.centre(system);
        let radius = target.body_radius(planet_radius);
        let gap = centre - from;
        let distance = gap.length();
        // Standing inside a body is not aiming at it.
        if distance <= radius {
            continue;
        }
        let off = along.angle_between(gap / distance);
        // How wide the body looks from here, as the half-angle of its disc.
        let subtends = (radius / distance).min(1.0).asin();
        let margin = off - subtends;
        if margin < PICK_SLOP && best.is_none_or(|(held, _)| margin < held) {
            best = Some((margin, target));
        }
    }
    best.map(|(_, target)| target)
}

/// Locks the autopilot onto whatever the crosshair rests on when the routed
/// action button is pressed, and lets go on a press at empty sky.
/// Fixed-step, before the movement that flies it, so a press and its first
/// burn land on the same tick.
#[allow(clippy::too_many_arguments)]
pub fn select(
    id: Res<LevelId>,
    mut input: ResMut<InputState>,
    mut pilot: ResMut<Autopilot>,
    mut console: ResMut<ConsoleState>,
    system: Res<SolarSystem>,
    level: Res<LevelData>,
    camera: Query<&Transform, With<Camera3d>>,
) {
    let pressed = InputState::take(&mut input.autopilot_released);
    if *id != LevelId::PlanetOrbit {
        // Nowhere to fly to: the castle has no system over it. The edge was
        // still consumed, so a press here does not fire on arrival.
        if pilot.engaged() {
            pilot.disengage();
        }
        return;
    }
    if !pressed {
        return;
    }
    let Ok(eye) = camera.single() else {
        return;
    };
    let radius = planet_radius(&level);
    let fixtures = level.world_count().saturating_sub(1);
    match aimed_at(
        &system,
        radius,
        fixtures,
        eye.translation,
        eye.forward().into(),
    ) {
        // A second press on the body already locked asks for the parking
        // orbit instead of the crossing; a third asks the crossing back.
        Some(target) if pilot.target == Some(target) && matches!(pilot.mode, Mode::Approach) => {
            pilot.mode = Mode::Orbit {
                hold: target.stop_radius(radius) + ORBIT_ALTITUDE,
            };
            pilot.phase = Phase::Idle;
            console.report(format!("autopilot: parking orbit round {}", target.name()));
        }
        Some(target) => {
            *pilot = Autopilot::approach(target);
            console.report(format!("autopilot: locked on {}", target.name()));
        }
        None if pilot.engaged() => {
            pilot.disengage();
            console.report("autopilot: off".to_string());
        }
        None => console.report("autopilot: nothing under the crosshair".to_string()),
    }
}

/// One tick of guidance: the unit thrust direction the crossing wants, or
/// `None` for hands off -- either the coast is already the plan, or the
/// destination has been reached and the autopilot has let go.
///
/// A function rather than a system because the burn has to happen *inside*
/// [`crate::player::movement`]'s flight branch, under the same energy meter
/// and the same acceleration the player's own thumb gets: an autopilot with
/// its own physics is two flight models to keep honest.
pub fn steer(
    pilot: &mut Autopilot,
    system: &SolarSystem,
    planet_radius: f32,
    at: Vec3,
    velocity: Vec3,
    accel: f32,
) -> Option<Vec3> {
    let target = pilot.target?;
    let gap = target.centre(system) - at;
    let distance = gap.length();
    let stop_at = target.stop_radius(planet_radius);
    let toward = gap.normalize_or(Vec3::Z);
    let desired = match pilot.mode {
        Mode::Approach => {
            if distance <= stop_at {
                match target {
                    // The sun has no gravity to hand over to: arrival is
                    // release.
                    Target::Sun => {
                        pilot.disengage();
                        return None;
                    }
                    // A planet's boundary is a handover, not an arrival: the
                    // descent inside it belongs to the same computer riding
                    // the jet against the pull, in the movement code. Out
                    // here in the boundary's skin there is nothing to do.
                    // A fixture's pull hands over the same way.
                    Target::Planet(_) | Target::Fixture(_) => return None,
                }
            }
            // The fastest speed, straight at the target, that the remaining
            // distance can still absorb. Everything the guidance does falls
            // out of pushing the velocity towards this one vector -- floored
            // at the handover pace on a planet, so the plan crosses the
            // weightless line instead of parking on it.
            let allowed = (2.0 * accel * BRAKE_MARGIN * (distance - stop_at)).sqrt();
            let allowed = match target {
                Target::Planet(_) | Target::Fixture(_) => allowed.max(HANDOVER),
                Target::Sun => allowed,
            };
            toward * allowed
        }
        Mode::Orbit { hold } => {
            // Close on the ring at the speed the gap can absorb -- the same
            // braking rule the crossing flies, signed for whichever side of
            // the ring this is -- while carrying the ring's own pace round
            // the body, whichever way the flight already leans.
            let outward = distance - hold;
            let closing = (2.0 * accel * BRAKE_MARGIN * outward.abs())
                .sqrt()
                .copysign(outward);
            let sideways = velocity - toward * velocity.dot(toward);
            let round = sideways.normalize_or(
                toward
                    .cross(Vec3::Y)
                    .normalize_or(toward.cross(Vec3::X).normalize_or(Vec3::Z)),
            );
            toward * closing + round * ORBIT_SPEED
        }
    };
    let correction = desired - velocity;
    if correction.length() < CLOSE_ENOUGH {
        return None;
    }
    pilot.flown = true;
    pilot.phase = match pilot.mode {
        Mode::Orbit { .. } => Phase::Orbit,
        Mode::Approach => match correction.dot(velocity) < 0.0 {
            true => Phase::Brake,
            false => Phase::Burn,
        },
    };
    Some(correction / correction.length())
}

/// The fastest fall the height still under the body can absorb, given the
/// braking the jet has to spare over gravity -- the descent's own copy of
/// the crossing's one rule, floored at [`TOUCHDOWN`] so the last metres are
/// landed rather than hovered. In metres a second, positive down.
pub fn descent_limit(brake: f32, altitude: f32) -> f32 {
    (2.0 * brake.max(0.0) * BRAKE_MARGIN * altitude.max(0.0))
        .sqrt()
        .max(TOUCHDOWN)
}

/// How long the flight computer expects the crossing to take, in seconds:
/// the plan flown ahead at a coarse step, counting until it lets go. `None`
/// when it does not arrive inside ten minutes, which on this system means
/// the burn cannot get there -- out of the question rather than out of time.
///
/// Gravity is left out on purpose: the plan lives almost entirely in the
/// weightless middle, and an ETA is a briefing, not a simulation.
pub fn eta(
    target: Target,
    system: &SolarSystem,
    planet_radius: f32,
    at: Vec3,
    velocity: Vec3,
    accel: f32,
) -> Option<f32> {
    const STEP: f32 = 0.25;
    let mut ghost = Autopilot::approach(target);
    let stop_at = target.stop_radius(planet_radius);
    let (mut here, mut velocity) = (at, velocity);
    for tick in 0..(600.0 / STEP) as usize {
        // Arrived is *reaching* the stop radius: a planet's plan never lets
        // go out here -- the handover to gravity does that -- so the clock
        // reads the distance rather than the lock.
        if (target.centre(system) - here).length() <= stop_at {
            return Some(tick as f32 * STEP);
        }
        match steer(&mut ghost, system, planet_radius, here, velocity, accel) {
            Some(push) => velocity += push * (accel * STEP),
            None if !ghost.engaged() => return Some(tick as f32 * STEP),
            None => {}
        }
        here += velocity * STEP;
    }
    None
}

/// The lock-on's on-screen furniture: the bracket that sits over the chosen
/// body, and the tag beside it that reads out range, closing speed and ETA.
#[derive(Component)]
pub struct Marker;

#[derive(Component)]
pub struct MarkerLabel;

/// Puts the lock-on bracket and its tag on the screen, once, hidden. The
/// same spawn-and-rewrite shape every other HUD element here has: a marker
/// that respawned would flicker on every lock.
pub fn spawn_hud(commands: &mut Commands) {
    commands.spawn((
        Marker,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(48.0),
            height: Val::Px(48.0),
            border: UiRect::all(Val::Px(2.0)),
            border_radius: BorderRadius::all(Val::Px(10.0)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.3, 0.9, 1.0, 0.9)),
        Visibility::Hidden,
    ));
    commands.spawn((
        MarkerLabel,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgba(0.75, 0.95, 1.0, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// A range in metres, said the way a HUD says it.
fn range_label(metres: f32) -> String {
    match metres >= 1000.0 {
        true => format!("{:.1} km", metres / 1000.0),
        false => format!("{:.0} m", metres.max(0.0)),
    }
}

/// Seconds as `m:ss`, or the honest shrug.
fn eta_label(seconds: Option<f32>) -> String {
    match seconds {
        Some(seconds) => format!("{}:{:02}", seconds as u32 / 60, seconds as u32 % 60),
        None => "--:--".to_string(),
    }
}

/// Keeps the bracket over the locked body and the tag's numbers current:
/// range to the surface, closing speed, and the plan's own ETA. The bracket
/// is sized off the body's projected disc, so a world grows in the frame as
/// it grows in the glass; everything turns amber when the brake takes over,
/// which is the one phase change worth a glance.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn hud(
    id: Res<LevelId>,
    pilot: Res<Autopilot>,
    system: Res<SolarSystem>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    tuning: Res<GameTuning>,
    pose: Res<RenderPose>,
    curve: Res<crate::flatten::Curve>,
    fixed: Res<Time<Fixed>>,
    player: Query<&Controller, With<Player>>,
    camera: Query<(&Camera, &Transform), With<Camera3d>>,
    mut marker: Query<
        (&mut Node, &mut Visibility, &mut BorderColor),
        (With<Marker>, Without<MarkerLabel>),
    >,
    mut label: Query<
        (&mut Node, &mut Text, &mut TextColor, &mut Visibility),
        (With<MarkerLabel>, Without<Marker>),
    >,
) {
    let Ok((mut bracket, mut bracket_shown, mut border)) = marker.single_mut() else {
        return;
    };
    let Ok((mut tag, mut text, mut tint, mut tag_shown)) = label.single_mut() else {
        return;
    };
    let mut hide = || {
        bracket_shown.set_if_neq(Visibility::Hidden);
        tag_shown.set_if_neq(Visibility::Hidden);
    };
    let (Some(target), LevelId::PlanetOrbit) = (pilot.target, *id) else {
        hide();
        return;
    };
    let (Ok(ctrl), Ok((camera, view))) = (player.single(), camera.single()) else {
        hide();
        return;
    };
    // This frame's camera, not last frame's: `GlobalTransform` is only
    // written back in `PostUpdate`, so read here -- after `camera::update`,
    // before propagation -- it is a frame stale, and a bracket projected
    // through yesterday's eye at this frame's planet buzzes on the glass by
    // exactly the frame's worth of flight between the two. The camera rides
    // at the root, so its local transform *is* its world pose, one schedule
    // earlier.
    let eye = GlobalTransform::from(*view);
    // Where the body is *drawn* this frame -- the same blend the scenery
    // rides -- and where that lands on the glass. Behind the camera there is
    // nothing honest to draw, so the lock keeps quietly to itself.
    let blended = system.blended(fixed.overstep_fraction().clamp(0.0, 1.0));
    let (centre, body_radius) = match target {
        // The planets through the frame's blend, because they move; the sun
        // and the fixtures stand still.
        Target::Planet(index) => (blended[index.min(1)].0, planet_radius(&level)),
        _ => (
            target.centre(&system),
            target.body_radius(planet_radius(&level)),
        ),
    };
    // Bent through the frame's flat-map before projecting, like every world
    // position that has to land where the *picture* is. Today a lockable body
    // always sits far outside the map's altitude band, so the bend is the
    // identity -- but the bracket asking the same question as the vertex
    // shader is what keeps that true by construction rather than by luck.
    let Ok(screen) = camera.world_to_viewport(&eye, curve.bend(centre)) else {
        hide();
        return;
    };
    // Sized off the projected disc, with margin: the bracket holds the whole
    // body at any range without swallowing the screen up close.
    let rim: Vec3 = eye.right().into();
    let projected = camera
        .world_to_viewport(&eye, curve.bend(centre + rim * body_radius))
        .map(|edge| (edge - screen).length())
        .unwrap_or(20.0);
    let size = (projected * 2.0 + 18.0).clamp(44.0, 320.0);
    bracket.left = Val::Px(screen.x - size * 0.5);
    bracket.top = Val::Px(screen.y - size * 0.5);
    bracket.width = Val::Px(size);
    bracket.height = Val::Px(size);
    let colour = match pilot.phase {
        Phase::Brake => Color::srgba(1.0, 0.65, 0.25, 0.95),
        Phase::Burn | Phase::Idle | Phase::Orbit => Color::srgba(0.3, 0.9, 1.0, 0.9),
    };
    *border = BorderColor::all(colour);
    tint.0 = colour;
    tag.left = Val::Px(screen.x + size * 0.5 + 10.0);
    tag.top = Val::Px(screen.y - 12.0);

    let gap = centre - pose.translation;
    let range = (gap.length() - body_radius).max(0.0);
    let closing = ctrl.velocity.dot(gap.normalize_or_zero());
    // The ETA is the plan's own answer, and only a crossing has one: on the
    // ground the numbers would be a story about a burn nobody is flying, and
    // a parking orbit never arrives anywhere -- it says what it is instead.
    let clock = match pilot.mode {
        Mode::Orbit { .. } => "orbit".to_string(),
        Mode::Approach => {
            let seconds = gravity.weightless(pose.translation).then(|| {
                let accel = tuning.fly_accel
                    * match tuning.infinite_thrust > 0.5 {
                        true => INFINITE_BURN,
                        false => 1.0,
                    };
                eta(
                    target,
                    &system,
                    planet_radius(&level),
                    pose.translation,
                    ctrl.velocity,
                    accel,
                )
            });
            format!("eta {}", eta_label(seconds.flatten()))
        }
    };
    **text = format!(
        "{}\n{}  ·  {:+.0} m/s  ·  {}",
        target.tag(),
        range_label(range),
        closing,
        clock,
    );
    bracket_shown.set_if_neq(Visibility::Visible);
    tag_shown.set_if_neq(Visibility::Visible);
}

/// The measured radius the guidance and the HUD both quote, off whatever
/// planet collision is up; the stand-in while the level loads measures 1.
fn planet_radius(level: &LevelData) -> f32 {
    match level.shape() {
        Shape::Planet { radius, .. } => radius,
        Shape::Flat => 0.0,
    }
}

/// Says what the flight computer is doing, once per change, on the console.
///
/// A watcher rather than lines inside [`steer`], because steering runs
/// thirty times a second and the console wants each fact once: the burn
/// turning into the brake, and the letting-go at the far end -- which is
/// "arrived" if it was braking and "cancelled" if a thumb on the stick took
/// it back mid-burn.
pub fn report(
    pilot: Res<Autopilot>,
    mut console: ResMut<ConsoleState>,
    mut last: Local<(bool, Phase)>,
) {
    let now = (pilot.engaged(), pilot.phase);
    if now == *last {
        return;
    }
    match (last.0, now.0, now.1) {
        (_, true, Phase::Burn) => console.report("autopilot: burning".to_string()),
        (_, true, Phase::Brake) => console.report("autopilot: braking".to_string()),
        // Only a lock that was braking has *arrived*; every other way of
        // going dark already said its piece in [`select`].
        (true, false, _) if last.1 == Phase::Brake => {
            console.report("autopilot: arrived -- she is yours".to_string());
        }
        _ => {}
    }
    *last = now;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::FIXED_DT;

    fn system() -> SolarSystem {
        SolarSystem::default()
    }

    /// The crosshair is the selector: dead on a body locks it, a near miss
    /// within the rim's grace still locks it, and empty sky locks nothing --
    /// which [`select`] turns into letting go.
    #[test]
    fn the_crosshair_picks_the_body_it_rests_on() {
        let system = system();
        let radius = 300.0;
        let from = Vec3::new(0.0, 2000.0, 0.0);
        let at_planet = (system.bodies[1].centre - from).normalize();
        assert_eq!(
            aimed_at(&system, radius, 0, from, at_planet),
            Some(Target::Planet(1))
        );
        // A near miss inside the slop is the same lock: nobody should have
        // to centre a three-degree disc at five kilometres.
        let brushed = Quat::from_rotation_x(0.04) * at_planet;
        assert_eq!(
            aimed_at(&system, radius, 0, from, brushed),
            Some(Target::Planet(1))
        );
        let at_sun = (SUN_CENTRE - from).normalize();
        assert_eq!(
            aimed_at(&system, radius, 0, from, at_sun),
            Some(Target::Sun)
        );
        // Empty sky is nothing, which is the off switch.
        assert_eq!(aimed_at(&system, radius, 0, from, Vec3::Y), None);
    }

    /// The whole crossing, flown: from rest in the weightless middle, the
    /// guidance accelerates at the target, comes back down, and crosses the
    /// stop radius at a walking pace with the lock still on -- the handover
    /// to the planet's gravity, where the descent code takes the rest. This
    /// is the accelerate-then-decelerate the feature asks for, held as an
    /// outcome rather than as a phase sequence.
    #[test]
    fn the_crossing_burns_out_and_hands_over_at_a_walk() {
        let system = system();
        let radius = 300.0;
        let accel = 24.0 * 6.0; // the infinite booster's burn
        let mut pilot = Autopilot::approach(Target::Planet(1));
        let mut at = system.bodies[0].centre
            + (system.bodies[1].centre - system.bodies[0].centre).normalize() * 800.0;
        let mut velocity = Vec3::ZERO;
        let stop_at = Target::Planet(1).stop_radius(radius);
        let mut top_speed = 0.0f32;
        let mut braked = false;
        let mut crossed = None;
        for _ in 0..(240.0 / FIXED_DT) as usize {
            if let Some(push) = steer(&mut pilot, &system, radius, at, velocity, accel) {
                braked |= pilot.phase == Phase::Brake;
                velocity += push * (accel * FIXED_DT);
            }
            at += velocity * FIXED_DT;
            top_speed = top_speed.max(velocity.length());
            if (at - system.bodies[1].centre).length() <= stop_at {
                crossed = Some(velocity.length());
                break;
            }
        }
        let crossing = crossed.expect("four minutes and it never arrived");
        assert!(pilot.engaged(), "the lock let go before the handover");
        assert!(
            pilot.flown,
            "a whole crossing and it never counted as flown"
        );
        assert!(braked, "it arrived without ever braking");
        assert!(top_speed > 100.0, "it never really burned: {top_speed} m/s");
        assert!(
            crossing < HANDOVER + 10.0,
            "it crossed the boundary doing {crossing} m/s"
        );
    }

    /// The fixtures are destinations too: with them filed, the crosshair
    /// picks the test sphere out of the sky, and the crossing flies there
    /// and hands over at its boundary exactly the way a planet's does.
    #[test]
    fn a_fixture_is_a_destination_like_any_planet() {
        let system = system();
        let radius = 300.0;
        let sphere = crate::world::FIXTURES[0].stands_at;
        // Aimed straight at it from over the sun, with all three present it
        // locks; loaded without fixtures the same aim is empty sky.
        let from = Vec3::new(0.0, 2000.0, 0.0);
        let at_sphere = (sphere - from).normalize();
        assert_eq!(
            aimed_at(&system, radius, 3, from, at_sphere),
            Some(Target::Fixture(0))
        );
        assert_eq!(aimed_at(&system, radius, 0, from, at_sphere), None);
        // And the crossing arrives: same guidance, same handover.
        let accel = 24.0 * 6.0;
        let mut pilot = Autopilot::approach(Target::Fixture(0));
        let (mut at, mut velocity) = (from, Vec3::ZERO);
        let stop_at = Target::Fixture(0).stop_radius(radius);
        let mut crossed = None;
        for _ in 0..(240.0 / FIXED_DT) as usize {
            if let Some(push) = steer(&mut pilot, &system, radius, at, velocity, accel) {
                velocity += push * (accel * FIXED_DT);
            }
            at += velocity * FIXED_DT;
            if (at - sphere).length() <= stop_at {
                crossed = Some(velocity.length());
                break;
            }
        }
        let crossing = crossed.expect("four minutes and it never reached the sphere");
        assert!(pilot.engaged(), "the lock let go before the handover");
        assert!(
            crossing < HANDOVER + 10.0,
            "it crossed the sphere's boundary doing {crossing} m/s"
        );
    }

    /// The parking orbit settles onto its ring and stays there, going round:
    /// engaged well outside the ring, the guidance closes on it, holds the
    /// radius, and keeps the lock -- an orbit is not a trip that ends.
    #[test]
    fn the_parking_orbit_settles_on_its_ring_and_goes_round() {
        let system = system();
        let radius = 300.0;
        let accel = 24.0 * 6.0;
        let centre = system.bodies[1].centre;
        let hold = Target::Planet(1).stop_radius(radius) + 60.0;
        let mut pilot = Autopilot {
            target: Some(Target::Planet(1)),
            mode: Mode::Orbit { hold },
            ..Default::default()
        };
        let mut at = centre + Vec3::X * (hold + 900.0);
        let mut velocity = Vec3::ZERO;
        let mut swept = 0.0;
        let mut prev = (at - centre).normalize();
        let settle = (90.0 / FIXED_DT) as usize;
        for tick in 0..(120.0 / FIXED_DT) as usize {
            if let Some(push) = steer(&mut pilot, &system, radius, at, velocity, accel) {
                velocity += push * (accel * FIXED_DT);
            }
            at += velocity * FIXED_DT;
            let dir = (at - centre).normalize();
            if tick > settle {
                let out = (at - centre).length();
                assert!(
                    (out - hold).abs() < 40.0,
                    "the ring wandered to {out} m against a hold of {hold}"
                );
                swept += prev.angle_between(dir);
            }
            prev = dir;
        }
        assert!(pilot.engaged(), "a parking orbit let go of its lock");
        assert_eq!(pilot.phase, Phase::Orbit);
        assert!(
            swept > 1.0,
            "half a minute on the ring swept only {swept} rad"
        );
    }

    /// The descent under gravity is landed rather than hovered: held to
    /// [`descent_limit`] by the jet, a fall from the handover altitude
    /// reaches the ground at the touchdown pace, and in seconds rather than
    /// on a parachute.
    #[test]
    fn the_descent_is_landed_rather_than_hovered() {
        // The profile itself: shrinks with the height left, floored at the
        // touchdown pace so the last metres do not asymptote.
        assert!(descent_limit(36.0, 240.0) > descent_limit(36.0, 60.0));
        assert_eq!(descent_limit(36.0, 0.0), TOUCHDOWN);
        // And flown, the way the movement code flies it: gravity every tick,
        // the jet only when the fall outruns the limit.
        let (gravity, jet_per_tick) = (36.0, 2.4);
        let brake = jet_per_tick / FIXED_DT - gravity;
        let mut altitude = 240.0;
        let mut fall = HANDOVER;
        let mut ticks = 0;
        while altitude > 0.0 {
            ticks += 1;
            assert!(
                ticks < (60.0 / FIXED_DT) as usize,
                "a minute and still aloft"
            );
            fall += gravity * FIXED_DT;
            if fall > descent_limit(brake, altitude) {
                fall -= jet_per_tick;
            }
            altitude -= fall * FIXED_DT;
        }
        assert!(
            fall < TOUCHDOWN + 2.0,
            "hit the ground doing {fall} m/s against a touchdown of {TOUCHDOWN}"
        );
    }

    /// The drift is killed, not just outrun: start moving square across the
    /// line to the target, and the guidance still closes on the body rather
    /// than spiralling past it.
    #[test]
    fn a_sideways_start_is_straightened_out() {
        let system = system();
        let accel = 24.0 * 6.0;
        let mut pilot = Autopilot::approach(Target::Sun);
        let mut at = SUN_CENTRE + Vec3::new(2000.0, 400.0, 0.0);
        let mut velocity = Vec3::Z * 80.0;
        for _ in 0..(240.0 / FIXED_DT) as usize {
            match steer(&mut pilot, &system, 300.0, at, velocity, accel) {
                Some(push) => velocity += push * (accel * FIXED_DT),
                None if !pilot.engaged() => break,
                None => {}
            }
            at += velocity * FIXED_DT;
        }
        assert!(!pilot.engaged(), "never made the sun");
        let out = (at - SUN_CENTRE).length();
        assert!(
            (out - Target::Sun.stop_radius(300.0)).abs() < 60.0,
            "let go {out} m from the sun"
        );
    }

    /// The ETA is the plan's own clock: further is longer, and a burn that
    /// cannot arrive inside the horizon says so rather than guessing.
    #[test]
    fn the_eta_is_the_plans_own_clock() {
        let system = system();
        let burn = 24.0 * 6.0;
        let near = eta(
            Target::Sun,
            &system,
            300.0,
            SUN_CENTRE + Vec3::X * 1500.0,
            Vec3::ZERO,
            burn,
        )
        .expect("a short hop never arrived");
        let far = eta(
            Target::Sun,
            &system,
            300.0,
            SUN_CENTRE + Vec3::X * 5000.0,
            Vec3::ZERO,
            burn,
        )
        .expect("a longer leg never arrived");
        assert!(near > 1.0, "arrival in under a second: {near}");
        assert!(far > near, "the longer leg reads shorter: {far} vs {near}");
        // An engine too weak to make it inside ten minutes is an honest
        // shrug, which the HUD prints as `--:--`.
        assert_eq!(
            eta(
                Target::Sun,
                &system,
                300.0,
                SUN_CENTRE + Vec3::X * 5000.0,
                Vec3::ZERO,
                0.001,
            ),
            None
        );
    }

    /// The tag's numbers read like a HUD and not like a debugger.
    #[test]
    fn ranges_and_etas_read_like_a_hud() {
        assert_eq!(range_label(432.4), "432 m");
        assert_eq!(range_label(4321.0), "4.3 km");
        assert_eq!(range_label(-3.0), "0 m");
        assert_eq!(eta_label(Some(83.0)), "1:23");
        assert_eq!(eta_label(Some(4.0)), "0:04");
        assert_eq!(eta_label(None), "--:--");
    }

    /// With nothing locked the bracket and the tag stay dark -- the HUD's
    /// resting state, and the one every level but the system lives in.
    #[test]
    fn the_lock_marker_stays_dark_without_a_lock() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(LevelId::Castle);
        world.insert_resource(Autopilot::default());
        world.insert_resource(SolarSystem::default());
        world.insert_resource(crate::level::LevelData::planet(
            &[],
            &[],
            Vec3::ZERO,
            300.0,
            None,
        ));
        world.insert_resource(Gravity::default());
        world.insert_resource(GameTuning::default());
        world.init_resource::<crate::flatten::Curve>();
        world.insert_resource(RenderPose {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        });
        world.init_resource::<Time<Fixed>>();
        world
            .run_system_once(|mut commands: Commands| spawn_hud(&mut commands))
            .expect("the marker would not spawn");
        world.run_system_once(hud).expect("the hud would not run");
        let mut shown = world.query::<(&Visibility, Option<&Marker>, Option<&MarkerLabel>)>();
        let mut seen = 0;
        for (visibility, marker, label) in shown.iter(&world) {
            if marker.is_some() || label.is_some() {
                seen += 1;
                assert_eq!(*visibility, Visibility::Hidden, "lit with nothing locked");
            }
        }
        assert_eq!(seen, 2, "the bracket or the tag never spawned");
    }

    /// Arrival stops outside the sun, because there is no gravity there to
    /// finish the job and the surface is a wall.
    #[test]
    fn the_sun_approach_stops_off_the_surface() {
        assert!(Target::Sun.stop_radius(300.0) > SUN_RADIUS);
        // And a planet's handover is to its gravity, at the weightless edge.
        assert_eq!(
            Target::Planet(0).stop_radius(300.0),
            300.0 + GRAVITY_RANGE + GRAVITY_FADE
        );
    }
}
