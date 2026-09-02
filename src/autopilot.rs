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
//! It stops *short*: at a planet's weightless boundary, because inside that
//! the gravity is the better pilot and [`crate::player::movement`] hands the
//! body to it anyway; at a hop over the sun's surface, because the sun has no
//! gravity to hand over to and flying you into a wall is not arriving.

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
}

impl Target {
    /// In the order X steps through them, before wrapping round to off.
    pub const ALL: [Target; 3] = [Target::Planet(0), Target::Planet(1), Target::Sun];

    pub fn name(self) -> &'static str {
        match self {
            Target::Planet(0) => "the first planet",
            Target::Planet(_) => "the second planet",
            Target::Sun => "the sun",
        }
    }

    /// The short form the lock-on tag wears on screen, where "the first
    /// planet" is a sentence and a HUD wants a callsign.
    pub fn tag(self) -> &'static str {
        match self {
            Target::Planet(0) => "PLANET I",
            Target::Planet(_) => "PLANET II",
            Target::Sun => "SUN",
        }
    }

    /// Where the target is *now* -- asked every tick, because everything but
    /// the sun is moving.
    pub fn centre(self, system: &SolarSystem) -> Vec3 {
        match self {
            Target::Planet(index) => system.bodies[index.min(1)].centre,
            Target::Sun => SUN_CENTRE,
        }
    }

    /// The distance from the target's centre the approach plans to be
    /// stationary at.
    pub fn stop_radius(self, planet_radius: f32) -> f32 {
        match self {
            // The weightless boundary: past here the planet's own pull is
            // the better pilot, and the movement code hands over to it.
            Target::Planet(_) => planet_radius + GRAVITY_RANGE + GRAVITY_FADE,
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
}

/// The autopilot's whole state: where it is pointed, if anywhere, and what
/// its burn is doing about it.
#[derive(Resource, Default)]
pub struct Autopilot {
    pub target: Option<Target>,
    pub phase: Phase,
}

impl Autopilot {
    pub fn engaged(&self) -> bool {
        self.target.is_some()
    }

    pub fn disengage(&mut self) {
        self.target = None;
        self.phase = Phase::Idle;
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
    from: Vec3,
    along: Vec3,
) -> Option<Target> {
    let along = along.normalize_or(Vec3::Z);
    let mut best: Option<(f32, Target)> = None;
    for target in Target::ALL {
        let (centre, radius) = match target {
            Target::Planet(index) => (system.bodies[index.min(1)].centre, planet_radius),
            Target::Sun => (SUN_CENTRE, SUN_RADIUS),
        };
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
    match aimed_at(
        &system,
        planet_radius(&level),
        eye.translation,
        eye.forward().into(),
    ) {
        Some(target) => {
            pilot.target = Some(target);
            pilot.phase = Phase::Idle;
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
    if distance <= stop_at {
        pilot.disengage();
        return None;
    }
    let toward = gap / distance;
    // The fastest speed, straight at the target, that the remaining distance
    // can still absorb. Everything the guidance does falls out of pushing
    // the velocity towards this one vector.
    let allowed = (2.0 * accel * BRAKE_MARGIN * (distance - stop_at)).sqrt();
    let correction = toward * allowed - velocity;
    if correction.length() < CLOSE_ENOUGH {
        return None;
    }
    pilot.phase = match correction.dot(velocity) < 0.0 {
        true => Phase::Brake,
        false => Phase::Burn,
    };
    Some(correction / correction.length())
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
    let mut ghost = Autopilot {
        target: Some(target),
        phase: Phase::Idle,
    };
    let (mut here, mut velocity) = (at, velocity);
    for tick in 0..(600.0 / STEP) as usize {
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
        Target::Planet(index) => (blended[index.min(1)].0, planet_radius(&level)),
        Target::Sun => (SUN_CENTRE, SUN_RADIUS),
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
        Phase::Burn | Phase::Idle => Color::srgba(0.3, 0.9, 1.0, 0.9),
    };
    *border = BorderColor::all(colour);
    tint.0 = colour;
    tag.left = Val::Px(screen.x + size * 0.5 + 10.0);
    tag.top = Val::Px(screen.y - 12.0);

    let gap = centre - pose.translation;
    let range = (gap.length() - body_radius).max(0.0);
    let closing = ctrl.velocity.dot(gap.normalize_or_zero());
    // The ETA is the plan's own answer, and only the flight has a plan: on
    // the ground the numbers would be a story about a burn nobody is flying.
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
    **text = format!(
        "{}\n{}  ·  {:+.0} m/s  ·  eta {}",
        target.tag(),
        range_label(range),
        closing,
        eta_label(seconds.flatten()),
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
            aimed_at(&system, radius, from, at_planet),
            Some(Target::Planet(1))
        );
        // A near miss inside the slop is the same lock: nobody should have
        // to centre a three-degree disc at five kilometres.
        let brushed = Quat::from_rotation_x(0.04) * at_planet;
        assert_eq!(
            aimed_at(&system, radius, from, brushed),
            Some(Target::Planet(1))
        );
        let at_sun = (SUN_CENTRE - from).normalize();
        assert_eq!(aimed_at(&system, radius, from, at_sun), Some(Target::Sun));
        // Empty sky is nothing, which is the off switch.
        assert_eq!(aimed_at(&system, radius, from, Vec3::Y), None);
    }

    /// The whole crossing, flown: from rest in the weightless middle, the
    /// guidance accelerates at the target, comes back down, and lets go
    /// stationary-ish at the stop radius -- without ever crossing inside it.
    /// This is the accelerate-then-decelerate the feature asks for, held as
    /// an outcome rather than as a phase sequence.
    #[test]
    fn the_crossing_burns_out_and_brakes_in() {
        let system = system();
        let radius = 300.0;
        let accel = 24.0 * 6.0; // the infinite booster's burn
        let mut pilot = Autopilot {
            target: Some(Target::Planet(1)),
            phase: Phase::Idle,
        };
        let mut at = system.bodies[0].centre
            + (system.bodies[1].centre - system.bodies[0].centre).normalize() * 800.0;
        let mut velocity = Vec3::ZERO;
        let stop_at = Target::Planet(1).stop_radius(radius);
        let mut top_speed = 0.0f32;
        let mut braked = false;
        for _ in 0..(240.0 / FIXED_DT) as usize {
            let Some(push) = steer(&mut pilot, &system, radius, at, velocity, accel) else {
                if !pilot.engaged() {
                    break;
                }
                at += velocity * FIXED_DT;
                continue;
            };
            braked |= pilot.phase == Phase::Brake;
            velocity += push * (accel * FIXED_DT);
            at += velocity * FIXED_DT;
            top_speed = top_speed.max(velocity.length());
            let out = (at - system.bodies[1].centre).length();
            assert!(
                out > stop_at - 5.0,
                "the approach carried {} m inside the stop radius",
                stop_at - out
            );
        }
        assert!(!pilot.engaged(), "four minutes and it never arrived");
        assert!(braked, "it arrived without ever braking");
        assert!(top_speed > 100.0, "it never really burned: {top_speed} m/s");
        assert!(
            velocity.length() < 20.0,
            "it let go still doing {} m/s",
            velocity.length()
        );
        let out = (at - system.bodies[1].centre).length();
        assert!(
            (out - stop_at).abs() < 60.0,
            "it let go {out} m out against a stop radius of {stop_at}"
        );
    }

    /// The drift is killed, not just outrun: start moving square across the
    /// line to the target, and the guidance still closes on the body rather
    /// than spiralling past it.
    #[test]
    fn a_sideways_start_is_straightened_out() {
        let system = system();
        let accel = 24.0 * 6.0;
        let mut pilot = Autopilot {
            target: Some(Target::Sun),
            phase: Phase::Idle,
        };
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
