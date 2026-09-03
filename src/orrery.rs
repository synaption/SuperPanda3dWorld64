//! The solar system's instruments, drawn over the world.
//!
//! Flying between planets is steering three invisible things at once: a pull
//! that fades with altitude, a velocity that nothing brakes, and a boundary --
//! somewhere out there -- past which the first stops applying to the second.
//! None of them is drawn by the game proper, so a crossing that goes wrong is
//! undiagnosable in exactly the way walking into an invisible wall used to be
//! before [`crate::collide`]: the picture on screen and the numbers the
//! simulation is steering by are different pictures.
//!
//! So this draws the numbers, in two layers, off by default and turned on with
//! `space_debug` in the console:
//!
//!   1. **The flyer's instruments**: the pull on the player as an arrow whose
//!      length is its strength -- gone entirely in the weightless middle,
//!      where a ring is drawn instead so "no arrow" and "not drawn" cannot be
//!      confused; the velocity as an arrow of its own; and the coast ahead --
//!      the path the next twenty seconds take with hands off the keys,
//!      integrated with the same `strength` and `up` the movement code reads,
//!      ending where it meets a planet. The coast line is the instrument that
//!      answers the only question that matters mid-crossing: "am I going to
//!      make it, and where do I come down?"
//!   2. **The system itself**: each planet's two gravity shells -- where the
//!      full pull starts to fade, and where it runs out -- the tether between
//!      the two worlds, each spin axis, the sun's surface, the full ring of
//!      each orbit, the direction to the sun, and the terminator ring where
//!      each world's day meets its night. This is the layer that shows the
//!      *shape* of [`crate::gravity::Gravity::System`]: stand on either shell
//!      boundary and the arrow of layer 1 should change exactly there, which
//!      is the agreement this overlay exists to check -- and the orbit rings
//!      are the picture the `planet*_dist` rows redraw as they are dragged.
//!
//! Both layers share one panel of *numbers* -- the [`Readout`]: who holds the
//! player, the grip, the altitude, the felt pull, and the velocity said in
//! the ground's own frame. Arrows show direction and hide magnitude; when the
//! question is "why did my straight line bend", the numbers are the evidence.
//!
//! One instrument is on without either layer: an engaged [`Autopilot`] draws
//! its own line to its destination and the ring it will stop at, because the
//! player flying that crossing is not debugging, they are navigating.
//!
//! Immediate mode and per frame, like [`crate::collide::draw`] and
//! [`crate::path::draw`], and read beside them: those draw what the world
//! will let a body do, this draws what space is about to do to it.

use crate::{
    autopilot::{self, Autopilot, Phase},
    console::GameTuning,
    gravity::{Gravity, FALL, GRAVITY_FADE, GRAVITY_RANGE},
    level::{LevelData, Shape},
    orbit::{Rider, SolarSystem, SUN_CENTRE, SUN_RADIUS},
    player::{Controller, Player, RenderPose},
    sky::Sky,
    world::LevelId,
};
use bevy::prelude::*;

/// How long an arrow a full planet's pull draws, in metres. The velocity
/// arrow shares the scale -- a tenth of a second of travel -- so the two can
/// be compared by eye: when the green arrow is longer than your speed, you
/// are not leaving.
const PULL_ARROW: f32 = 3.0;

/// How far ahead the coast is integrated, and how coarsely. Twenty seconds is
/// most of a crossing; a quarter-second step at these accelerations is well
/// under a metre of error per step, which is nothing against a planet.
const COAST_SECONDS: f32 = 20.0;
const COAST_STEP: f32 = 0.25;

/// How far ahead the autopilot's plan is flown for its drawn trajectory, and
/// at what step. Longer than the coast because a whole crossing is minutes;
/// the line ends early anyway wherever the plan says "arrived".
const PLAN_SECONDS: f32 = 180.0;
const PLAN_STEP: f32 = 0.25;

/// How far past a planet's own radius its axis is drawn, and how long the
/// sun's direction arrow is.
const AXIS_REACH: f32 = 1.4;
const SUN_ARROW: f32 = 8.0;

/// The palette. The shells wear the colour of what they bound: green where
/// the pull is whole, blue where it has run out, and the fade band lies
/// between the two rings.
const FULL_PULL: Color = Color::srgba(0.35, 0.95, 0.45, 0.8);
const NO_PULL: Color = Color::srgba(0.35, 0.65, 1.0, 0.8);
const VELOCITY: Color = Color::srgba(0.3, 0.9, 1.0, 0.9);
const COAST: Color = Color::srgba(0.85, 0.55, 1.0, 0.7);
const TETHER: Color = Color::srgba(0.9, 0.9, 0.9, 0.35);
const SUNWARD: Color = Color::srgb(1.0, 0.85, 0.3);
const TERMINATOR: Color = Color::srgba(1.0, 0.55, 0.2, 0.7);

/// The instruments' one line of *text*: which world holds the player, how
/// high above it he is, and how hard it pulls. The arrows of [`draw`] show
/// direction well and magnitude poorly, and when the frame itself is in
/// question -- "why did my straight line bend" -- the numbers behind the ride
/// are the evidence: a grip that is not the 100% expected, an altitude inside
/// the fade band, a ground speed that is not what the keys are asking for.
#[derive(Component)]
pub struct Readout;

/// Puts the readout on the screen, hidden until `space_debug` asks for it.
/// Top left: the frame chart owns the top right, the bars the bottom left,
/// and the action picker the bottom right.
pub fn spawn(commands: &mut Commands) {
    commands.spawn((
        Readout,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.75, 0.9, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            top: Val::Px(40.0),
            ..default()
        },
        Visibility::Hidden,
        GlobalZIndex(20),
    ));
}

/// Writes the readout, every frame `space_debug` is up.
#[allow(clippy::too_many_arguments)]
pub fn readout(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    id: Res<LevelId>,
    pose: Res<RenderPose>,
    player: Query<(&Controller, Option<&Rider>), With<Player>>,
    mut text: Query<(&mut Text, &mut Visibility), With<Readout>>,
) {
    let Ok((mut text, mut visible)) = text.single_mut() else {
        return;
    };
    if tuning.space_debug.round() as u32 == 0 {
        *visible = Visibility::Hidden;
        return;
    }
    *visible = Visibility::Inherited;
    let Ok((ctrl, rider)) = player.single() else {
        return;
    };
    let at = pose.translation;

    // Who holds him: the same record the ride and `sync_visual` steer by, so
    // when the picture is wrong this line says which frame it was drawn in.
    let held = match *id {
        LevelId::PlanetOrbit => {
            let rider = rider.copied().unwrap_or_default();
            match rider.world.filter(|_| rider.hold > 0.0) {
                Some(world) => {
                    format!(
                        "held by planet {}  grip {:.0}%",
                        world + 1,
                        rider.hold * 100.0
                    )
                }
                None => "held by nobody (inertial frame)".to_string(),
            }
        }
        LevelId::Planet => "held by the planet  grip 100%".to_string(),
        LevelId::Castle => "held by the castle (flat)".to_string(),
    };

    // Altitude over the nearest world the gravity answers for; sea level on
    // the castle, which has no radius to be over.
    let worlds = planets(&gravity, &level);
    let altitude = worlds
        .iter()
        .map(|&(centre, radius)| (at - centre).length() - radius)
        .min_by(f32::total_cmp)
        .unwrap_or(at.y);

    // The pull as the movement code will feel it, against the tuned full
    // strength -- under 100% is the fade band, 0% is the weightless middle.
    let pull = gravity.strength(at);
    let fraction = 100.0 * pull / gravity.accel().max(1e-6);

    // The velocity in the frame the ground defines: along it and out of it.
    // This is the pair that should hold steady when walking or coasting
    // "straight" -- if these are constant while the drawn arrow swings, the
    // arrow is showing the world frame turning, not the path bending.
    let (rise, flat) = gravity.split(ctrl.velocity, at);

    // The last thing that bent the line. A jerk with a fresh "wall push" or
    // "swept into surface" beside it is the collision's doing; one with the
    // record standing minutes old is not the movement code at all. The
    // height is the tiebreaker for "but I was in the air": a wall push at
    // half a metre is terrain being brushed, and one at twenty metres of
    // clear air is the wall resolution inventing a wall.
    let kick = match ctrl.kick {
        Some(kick) => {
            let height = match kick.height.is_finite() {
                true => format!("{:.1} m off the ground", kick.height),
                false => "over nothing".to_string(),
            };
            format!(
                "last kick: {} {:.2} m/s, {height}, {:.1} s ago",
                kick.cause, kick.speed, kick.age
            )
        }
        None => "last kick: none".to_string(),
    };
    **text = format!(
        "{held}\naltitude {altitude:.1} m\ngravity {pull:.1} m/s\u{b2}  ({fraction:.0}%)\nground speed {:.2} m/s  climb {rise:+.2} m/s\n{kick}",
        flat.length()
    );
}

/// Every planet the gravity answers for, with the radius its shells are
/// measured from. A system knows both of its own; a lone planet's pull knows
/// its centre but not its size, which lives in the collision's shape.
fn planets(gravity: &Gravity, level: &LevelData) -> Vec<(Vec3, f32)> {
    match (*gravity, level.shape()) {
        (Gravity::System { .. }, _) => gravity
            .wells()
            .iter()
            .map(|well| (well.centre, well.radius))
            .collect(),
        (Gravity::Radial { centre, .. }, Shape::Planet { radius, .. }) => vec![(centre, radius)],
        _ => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    sky: Res<Sky>,
    id: Res<LevelId>,
    pilot: Res<Autopilot>,
    system: Res<SolarSystem>,
    pose: Res<RenderPose>,
    fixed: Res<Time<Fixed>>,
    mut gizmos: Gizmos,
    player: Query<&Controller, With<Player>>,
) {
    let Ok(ctrl) = player.single() else {
        return;
    };
    // Anchored on the *rendered* pose, not the simulated one. Gizmos are
    // drawn every frame; the simulation steps thirty times a second; an
    // overlay hung off the stepped transform judders against the smooth
    // world it is annotating, which reads as a bug in whatever it points at.
    let at = pose.translation;
    // And the bodies likewise: the same frame blend the scenery rides.
    let blended = system.blended(fixed.overstep_fraction().clamp(0.0, 1.0));

    // The autopilot's own instruments, on whenever it is. Ungated by
    // `space_debug`, because a pilot mid-crossing is not debugging, they are
    // navigating -- amber while braking, so the flip shows.
    if let Some(target) = pilot.target {
        let radius = match level.shape() {
            Shape::Planet { radius, .. } => radius,
            Shape::Flat => 0.0,
        };
        let centre = match target {
            autopilot::Target::Planet(index) => blended[index.min(1)].0,
            // The sun and the fixtures stand still, so where they are is
            // where they are drawn.
            _ => target.centre(&system),
        };
        let colour = match pilot.phase {
            Phase::Brake => TERMINATOR,
            Phase::Burn | Phase::Idle | Phase::Orbit => VELOCITY,
        };
        // The plan, flown ahead of time: the same guidance, the same burn
        // and the same gravity, integrated forward -- so the curve on the
        // screen is the curve the crossing will fly, bent where the pull
        // bends it and turning over where the brake takes hold. This is the
        // trajectory, where a straight line to the target is only the
        // errand.
        let accel = tuning.fly_accel
            * match tuning.infinite_thrust > 0.5 {
                true => crate::player::INFINITE_BURN,
                false => 1.0,
            };
        // Only a flight has a trajectory: locked on from the ground, the
        // bracket and the readout stand alone, and the curve appears the
        // moment there is weightless space for it to cross.
        if gravity.weightless(at) {
            let mut ghost = Autopilot {
                target: Some(target),
                phase: pilot.phase,
                mode: pilot.mode,
                flown: false,
            };
            let mut here = at;
            let mut velocity = ctrl.velocity;
            let mut points = vec![here];
            for _ in 0..(PLAN_SECONDS / PLAN_STEP) as usize {
                match autopilot::steer(&mut ghost, &system, radius, here, velocity, accel) {
                    Some(push) => velocity += push * (accel * PLAN_STEP),
                    None if !ghost.engaged() => break,
                    None => {}
                }
                velocity -= gravity.up(here) * (gravity.strength(here) * PLAN_STEP);
                here += velocity * PLAN_STEP;
                points.push(here);
                // A crossing's curve ends at the handover: past the stop
                // radius the descent belongs to gravity and the jet, which
                // this plan does not model -- drawn on, it would spear the
                // terrain it is really landed onto.
                if matches!(pilot.mode, autopilot::Mode::Approach)
                    && (target.centre(&system) - here).length() <= target.stop_radius(radius)
                {
                    break;
                }
            }
            // Bright at the hand, fading toward the far end: the eye reads
            // the fade as direction, so the curve says which way it is flown
            // without an arrowhead every hundred metres.
            let count = points.len().max(2) as f32;
            gizmos.linestrip_gradient(points.iter().enumerate().map(|(step, &point)| {
                let fade = 1.0 - step as f32 / (count - 1.0);
                (point, colour.with_alpha(0.15 + 0.75 * fade))
            }));
        }
        gizmos.circle(
            Isometry3d::new(
                centre,
                Quat::from_rotation_arc(Vec3::Z, (at - centre).normalize_or(Vec3::Y)),
            ),
            target.stop_radius(radius),
            colour,
        );
    }

    let layer = tuning.space_debug.round() as u32;
    if layer == 0 {
        return;
    }
    let up = gravity.up(at);
    let pull = gravity.strength(at);
    let mut worlds = planets(&gravity, &level);
    // Re-hung at their render-rate poses: the shells and rings must stand on
    // the planets as drawn this frame, not as simulated this tick.
    if *id == LevelId::PlanetOrbit {
        for (world, &(centre, _)) in worlds.iter_mut().zip(blended.iter()) {
            world.0 = centre;
        }
    }

    // -- layer 1: the flyer's instruments ------------------------------------

    // The pull, as it is at this exact altitude: full length and green on the
    // ground, shortening and warming through the fade band, and in the
    // weightless middle a ring about the waist instead -- absence drawn as a
    // thing, not as a missing one.
    if pull > 0.0 {
        let fraction = pull / FALL;
        let colour = FULL_PULL.mix(&NO_PULL, 1.0 - fraction);
        gizmos.arrow(at, at - up * (PULL_ARROW * fraction), colour);
    } else {
        gizmos.circle(
            Isometry3d::new(at + up * 1.0, Quat::from_rotation_arc(Vec3::Z, up)),
            1.0,
            NO_PULL,
        );
    }
    // The velocity, at a tenth of a second of travel so it shares the pull
    // arrow's scale.
    if ctrl.velocity.length_squared() > 0.01 {
        gizmos.arrow(at, at + ctrl.velocity * 0.1, VELOCITY);
    }
    // The coast: where the next twenty seconds go with the keys released,
    // stepped with the same two questions -- `strength` and `up` -- the
    // movement code asks, so what is drawn is what the simple model will do
    // and not a second model to disagree with it. Ends on the first planet it
    // meets, which is the answer being asked for.
    if !ctrl.grounded && ctrl.velocity.length_squared() > 1.0 {
        let mut points = vec![at];
        let mut here = at;
        let mut velocity = ctrl.velocity;
        for _ in 0..(COAST_SECONDS / COAST_STEP) as usize {
            velocity -= gravity.up(here) * (gravity.strength(here) * COAST_STEP);
            here += velocity * COAST_STEP;
            points.push(here);
            if worlds
                .iter()
                .any(|&(centre, radius)| (here - centre).length() < radius)
            {
                break;
            }
        }
        gizmos.linestrip(points, COAST);
    }

    if layer < 2 {
        return;
    }

    // -- layer 2: the system itself ------------------------------------------

    for &(centre, radius) in &worlds {
        // The two shells: where the full pull begins to fade, and where it is
        // gone. Between the arrow of layer 1 and these rings there is exactly
        // one fact -- `Gravity::strength` -- and crossing a shell with the
        // overlay up is watching the two agree.
        gizmos.sphere(
            Isometry3d::from_translation(centre),
            radius + GRAVITY_RANGE,
            FULL_PULL,
        );
        gizmos.sphere(
            Isometry3d::from_translation(centre),
            radius + GRAVITY_RANGE + GRAVITY_FADE,
            NO_PULL,
        );
        // The spin axis, drawn through the poles the sky turns about.
        gizmos.line(
            centre - Vec3::Y * (radius * AXIS_REACH),
            centre + Vec3::Y * (radius * AXIS_REACH),
            TETHER,
        );
    }
    if let [(first, _), (second, _), ..] = worlds[..] {
        gizmos.line(first, second, TETHER);
    }
    // The system's architecture, on the level that has one: the sun's
    // surface, and each world's whole orbit drawn as the ring it actually
    // runs -- which is the picture that answers "are the proportions right",
    // and answers it live as the `planet*_dist` rows are dragged.
    if *id == LevelId::PlanetOrbit {
        gizmos.sphere(
            Isometry3d::from_translation(SUN_CENTRE),
            SUN_RADIUS,
            SUNWARD,
        );
        let flat = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
        for &(centre, _) in &worlds {
            gizmos.circle(
                Isometry3d::new(SUN_CENTRE, flat),
                (centre - SUN_CENTRE).length(),
                TETHER,
            );
        }
    }

    // Where the sun stands from the player, and the line its day draws on
    // each world. The plain planet has neither -- its light is a constant
    // and drawing a sun for it would be drawing something that is not there.
    let sun = match *id {
        LevelId::PlanetOrbit => Some((SUN_CENTRE - at).normalize_or(Vec3::X)),
        LevelId::Castle => Some(sky.sun()),
        LevelId::Planet => None,
    };
    if let Some(sun) = sun {
        gizmos.arrow(at, at + sun * SUN_ARROW, SUNWARD);
        for &(centre, radius) in &worlds {
            // The terminator: the great circle square to the sun *as that
            // world sees it*, which is where its day ends. Slightly proud of
            // the surface so the line is not fighting the terrain.
            let lit_from = match *id {
                LevelId::PlanetOrbit => (SUN_CENTRE - centre).normalize_or(Vec3::X),
                _ => sun,
            };
            gizmos.circle(
                Isometry3d::new(centre, Quat::from_rotation_arc(Vec3::Z, lit_from)),
                radius * 1.02,
                TERMINATOR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay's idea of where the planets are is the gravity's, not a
    /// copy of it: a system draws both worlds at the measured radius, a lone
    /// planet draws itself, and the castle draws none.
    #[test]
    fn the_overlay_finds_the_worlds_the_gravity_answers_for() {
        let level = LevelData::planet(&[], &[], Vec3::ZERO, 300.0, None);
        let system = Gravity::binary(Vec3::ZERO, Vec3::X * 1100.0, 300.0);
        assert_eq!(
            planets(&system, &level),
            vec![(Vec3::ZERO, 300.0), (Vec3::X * 1100.0, 300.0)]
        );
        let lone = Gravity::towards(Vec3::ZERO);
        assert_eq!(planets(&lone, &level), vec![(Vec3::ZERO, 300.0)]);
        let (castle, _) = crate::level::load();
        assert_eq!(planets(&Gravity::default(), &castle), Vec::new());
    }
}
