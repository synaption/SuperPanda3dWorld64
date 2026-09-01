//! The solar system, actually turning.
//!
//! The first cut of the orbiting level kept the world still and moved the
//! *sky*: spin and orbit were painted onto the dome, because from one planet's
//! surface the two are indistinguishable. Then the level grew a second planet
//! and a sun you are meant to fly to, and the trick ran out -- there is no
//! frame in which two planets on different orbits both stand still, and a sun
//! that is only a picture is a destination that recedes as you approach it.
//!
//! So the motion is real now, and *kinematic*: each planet runs a circle round
//! the sun at a rate off the console and turns on its axis at another, and
//! nothing attracts anything. This is deliberately not the full physics --
//! the user's words -- and it is also how `experimental/ow` gets away with a
//! playable system: the planets are clockwork, and only the player is
//! simulated.
//!
//! Three things have to move together or the world comes apart at the seams,
//! and [`advance`] moves all three in one tick: the **collision** (the filed
//! triangles stay put and every query is turned instead -- see
//! [`LevelData::place_planets`]), the **gravity** (each pull is towards where
//! its world is *now*), and the **scenery** (each scene root carries a
//! [`PlanetBody`] naming which world it draws). The player is the fourth
//! thing: anyone inside a world's pull rides that world's frame -- carried
//! round the sun and round the spin axis, which is what lets you stand still
//! on a planet that is doing twenty metres a second -- and the ride lets go
//! exactly as fast as the gravity does, so drifting out through the fade band
//! hands you smoothly over to the inertial frame where the planets visibly
//! sail past.
//!
//! The sun is the one body with no gravity and no mesh: a huge luminous
//! sphere (drawn by [`crate::sky`], since it is as much lamp as landmark)
//! that you coast up to and bump against -- [`advance`] holds every body out
//! of it analytically, which for a smooth sphere is cheaper and better than
//! half a million triangles would be.

use crate::{
    console::GameTuning,
    gravity::Gravity,
    level::{LevelData, Shape},
    player::{Controller, Player, FIXED_DT, PLAYER_RADIUS},
    world::{LevelId, Respawn},
};
use bevy::prelude::*;

/// Where the sun stands, which is the one position in the system that never
/// moves, and how big it is. Five hundred metres of radius against the
/// planets' three hundred: unmistakably the biggest thing in the sky, while
/// still small enough that the whole system reads in one glance.
pub const SUN_CENTRE: Vec3 = Vec3::ZERO;
pub const SUN_RADIUS: f32 = 500.0;

/// Which world a scene root draws, by index into [`SolarSystem::bodies`].
/// What lets [`advance`] move the scenery it did not spawn.
#[derive(Component, Clone, Copy)]
pub struct PlanetBody(pub usize);

/// One planet's place in its round: where it is along the orbit and how far
/// it has turned on itself, plus the worked-out world pose so every reader
/// gets the same answer without redoing the trigonometry.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    /// Radians along the orbit circle, anticlockwise seen from `+Y`.
    pub angle: f32,
    /// Radians of spin about the body's own `+Y`.
    pub spin: f32,
    /// Where the body's centre stands in the world, this tick.
    pub centre: Vec3,
    /// The turn its ground has made, this tick.
    pub rotation: Quat,
}

impl Body {
    fn at(angle: f32, spin: f32, distance: f32) -> Self {
        Self {
            angle,
            spin,
            centre: SUN_CENTRE + Vec3::new(angle.cos(), 0.0, angle.sin()) * distance,
            rotation: Quat::from_rotation_y(spin),
        }
    }

    /// A world position expressed in this body's own turning frame -- a
    /// *seat*: where you are relative to the ground, in coordinates the
    /// ground carries with it. Someone standing still on a moving planet has
    /// a constant seat, which is the whole point of having the word.
    pub fn seat_of(&self, world: Vec3) -> Vec3 {
        self.rotation.inverse() * (world - self.centre)
    }

    /// The seat put back into the world, through wherever the body stands
    /// now. `seated(seat_of(p))` is `p` for the same tick; across ticks the
    /// pair is exactly the ride [`advance`] gives a held player.
    pub fn seated(&self, seat: Vec3) -> Vec3 {
        self.centre + self.rotation * seat
    }
}

/// Being parented to a world that moves.
///
/// Bevy's own hierarchy is the wrong tool for this: the player's physics is
/// written in world space against a collision that answers in world space,
/// and re-basing all of it under a spinning scene root would be a rewrite in
/// exchange for nothing the maths does not already do. So the parenting is a
/// *record* rather than a hierarchy -- [`advance`] keeps it current: which
/// body holds this entity, and how hard. The physics half of the parent
/// transform is already applied by hand (the ride in [`advance`] is
/// `new_frame * old_frame⁻¹`, which is what a parent does); what the record
/// buys is the *drawn* half: [`crate::player::sync_visual`] resolves a held
/// entity's frame-blend through its parent's own blended frame, so a figure
/// standing on ground doing twenty metres a second is glued to it pixel for
/// pixel instead of chasing it chord by chord, tick by tick.
#[derive(Component, Clone, Copy, Default)]
pub struct Rider {
    /// Index into [`SolarSystem::bodies`] of the world doing the holding;
    /// `None` adrift, where there is no parent and world space is honest.
    pub world: Option<usize>,
    /// The grip, `0..=1`: the same gravity fraction that scales the physical
    /// ride, so the drawn handover through the fade band matches the felt one.
    pub hold: f32,
}

/// The clockwork's state: where each planet is in its round.
///
/// The *rates* live in [`GameTuning`] -- `planet1_dist`, `planet1_orbit`,
/// `planet1_spin` and their seconds -- because rates are settings and the
/// console is where settings live. What is here is only what accumulates.
#[derive(Resource, Debug)]
pub struct SolarSystem {
    pub bodies: [Body; 2],
    /// Where the bodies stood at the start of the last tick. The clockwork
    /// steps at 30 Hz and the screen does not: anything drawn straight off
    /// `bodies` moves in visible jumps -- nearly a metre a step at orbital
    /// speed -- so everything *rendered* reads [`Self::blended`] instead,
    /// exactly the way the player's own model rides
    /// [`crate::player::sync_visual`].
    pub previous: [Body; 2],
    /// Where the level put the player down, in the first planet's own filed
    /// coordinates. The world-space [`Respawn`] goes stale the moment the
    /// planet moves -- respawning at yesterday's coordinates is respawning
    /// into empty space -- so [`advance`] re-resolves it from this every
    /// tick. `None` until the level has somewhere to stand.
    pub respawn_local: Option<Vec3>,
}

impl Default for SolarSystem {
    fn default() -> Self {
        let tuning = GameTuning::default();
        // A quarter turn apart, so the level opens with the second world
        // hanging clearly off to one side of the sun rather than hiding
        // behind it or queueing in front of it.
        let bodies = [
            Body::at(0.0, 0.0, tuning.planet1_dist),
            Body::at(std::f32::consts::FRAC_PI_2, 0.0, tuning.planet2_dist),
        ];
        Self {
            bodies,
            previous: bodies,
            respawn_local: None,
        }
    }
}

impl SolarSystem {
    pub fn centres(&self) -> [Vec3; 2] {
        [self.bodies[0].centre, self.bodies[1].centre]
    }

    /// The bodies part-way between their last two ticks, for everything that
    /// draws: `alpha` is the frame's place inside the current fixed step,
    /// exactly the fraction [`crate::player::sync_visual`] blends the player
    /// by, so the ground and the figure standing on it move as one.
    pub fn blended(&self, alpha: f32) -> [(Vec3, Quat); 2] {
        [0, 1].map(|index| {
            let (was, is) = (self.previous[index], self.bodies[index]);
            (
                was.centre.lerp(is.centre, alpha),
                was.rotation.slerp(is.rotation, alpha),
            )
        })
    }

    /// The distances and rates for body `index`, read off the console rows.
    fn tuned(tuning: &GameTuning, index: usize) -> (f32, f32, f32) {
        match index {
            0 => (
                tuning.planet1_dist,
                tuning.planet1_orbit,
                tuning.planet1_spin,
            ),
            _ => (
                tuning.planet2_dist,
                tuning.planet2_orbit,
                tuning.planet2_spin,
            ),
        }
    }
}

/// One tick of the clockwork: the planets move, and everything that has to
/// agree about where they are is told in the same breath.
///
/// Runs at the head of the fixed step, before [`crate::player::movement`], so
/// the ground the player is about to be resolved against is the ground as it
/// stands this tick and not a fiftieth of a second ago.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn advance(
    id: Res<LevelId>,
    tuning: Res<GameTuning>,
    mut system: ResMut<SolarSystem>,
    mut gravity: ResMut<Gravity>,
    mut collision: ResMut<LevelData>,
    mut respawn: ResMut<Respawn>,
    mut scenes: Query<(&PlanetBody, &mut Transform), Without<Player>>,
    mut player: Query<(&mut Transform, &mut Controller, Option<&mut Rider>), With<Player>>,
) {
    if *id != LevelId::PlanetOrbit {
        return;
    }
    // The authored centre: where the filed geometry's middle sits in its own
    // coordinates, which is what every frame below is measured against. The
    // stand-in collision that holds the fort while the glTF loads has one
    // too, so nothing here waits for the load.
    let filed = match collision.shape() {
        Shape::Planet { centre, .. } => centre,
        Shape::Flat => return,
    };

    let before = system.bodies;
    for (index, body) in system.bodies.iter_mut().enumerate() {
        let (distance, orbit, spin) = SolarSystem::tuned(&tuning, index);
        *body = Body::at(
            body.angle + orbit.to_radians() * FIXED_DT,
            body.spin + spin.to_radians() * FIXED_DT,
            distance,
        );
    }
    system.previous = before;

    // The player rides whichever world holds him, by exactly as much as it
    // holds him: full pull is a full ride, and the fade band lets go by
    // degrees, so leaving a planet is a handover and not a jolt. Measured
    // against where the worlds *were*, because that is the frame he is
    // standing in as this tick opens.
    if let Ok((mut transform, mut ctrl, rider)) = player.single_mut() {
        let fraction = gravity.strength(transform.translation) / gravity.accel().max(1e-6);
        let nearest = (0..2)
            .min_by(|&a, &b| {
                (transform.translation - before[a].centre)
                    .length_squared()
                    .total_cmp(&(transform.translation - before[b].centre).length_squared())
            })
            .unwrap_or(0);
        // The parenting record, kept current in the same breath as the ride
        // it describes. Optional the way `energy` is optional in `movement`:
        // a player assembled without one still rides, he just is not drawn
        // through his parent's frame.
        if let Some(mut rider) = rider {
            rider.world = (fraction > 0.0).then_some(nearest);
            rider.hold = fraction;
        }
        if fraction > 0.0 {
            let (old, new) = (before[nearest], system.bodies[nearest]);
            let turn = new.rotation * old.rotation.inverse();
            let carried = new.centre + turn * (transform.translation - old.centre);
            transform.translation = transform.translation.lerp(carried, fraction);
            ctrl.velocity = Quat::IDENTITY.slerp(turn, fraction) * ctrl.velocity;
            transform.rotation = Quat::IDENTITY.slerp(turn, fraction) * transform.rotation;
        }
        // The sun is solid, and it is solid here rather than in the collision
        // because it is a perfect sphere: one distance test beats half a
        // million triangles. Held out by the body's radius, with the inbound
        // speed spent -- the same contact rule every other surface applies.
        let clear = SUN_RADIUS + PLAYER_RADIUS;
        let standing_off = transform.translation - SUN_CENTRE;
        if standing_off.length() < clear {
            let outward = standing_off.normalize_or(Vec3::Y);
            transform.translation = SUN_CENTRE + outward * clear;
            let into = ctrl.velocity.dot(outward);
            if into < 0.0 {
                ctrl.velocity -= outward * into;
            }
        }
    }

    // Now everything that answers "where is the world" is told the same
    // answer: the queries, the pull, the picture, and the way back.
    let placed: Vec<(Vec3, Quat)> = system
        .bodies
        .iter()
        .map(|body| (body.centre, body.rotation))
        .collect();
    collision.place_planets(&placed);
    gravity.place_system(system.centres());
    for (body, mut transform) in &mut scenes {
        let Some(state) = system.bodies.get(body.0) else {
            continue;
        };
        // The root draws authored coordinates, so it wears the whole frame:
        // spin about the authored centre, then stand where the body stands.
        transform.rotation = state.rotation;
        transform.translation = state.centre - state.rotation * filed;
    }
    if let Some(local) = system.respawn_local {
        respawn.0 = system.bodies[0].seated(local - filed);
    }
}

/// Re-hangs the whole system every drawn frame, part-way between the
/// clockwork's last two ticks: the scenery, the collision's frames, and the
/// gravity's centres, all at the same blended pose.
///
/// [`advance`] places all three too, but at 30 Hz -- and a planet that jumps
/// most of a metre thirty times a second under a player whose own model is
/// smoothly interpolated is a picture that stutters exactly where the eye is
/// looking. This is the scenery's [`crate::player::sync_visual`]: same blend
/// fraction, same schedule, so the ground and the figure on it cross the
/// frame together.
///
/// The collision and the gravity come along -- not for the physics, which
/// runs at the top of every fixed tick and is re-pointed at the tick pose by
/// [`advance`] before anything simulated asks -- but for everything that
/// queries the world *per frame* and draws the answer: the contact shadow,
/// the camera's boom probe, the aim. Left at the tick pose, each of those
/// answers up to a tick of orbital motion away from the terrain as drawn,
/// which is a shadow sawing back and forth under the player's feet. One
/// resource, two schedules, each re-pointed at the top of its own: the
/// simulation always sees tick poses, the frame always sees blended ones.
pub fn glide(
    fixed: Res<Time<Fixed>>,
    id: Res<LevelId>,
    system: Res<SolarSystem>,
    mut gravity: ResMut<Gravity>,
    mut collision: ResMut<LevelData>,
    mut scenes: Query<(&PlanetBody, &mut Transform)>,
) {
    if *id != LevelId::PlanetOrbit {
        return;
    }
    let Shape::Planet { centre: filed, .. } = collision.shape() else {
        return;
    };
    let blended = system.blended(fixed.overstep_fraction().clamp(0.0, 1.0));
    collision.place_planets(&blended);
    gravity.place_system([blended[0].0, blended[1].0]);
    for (body, mut transform) in &mut scenes {
        let Some(&(centre, rotation)) = blended.get(body.0) else {
            continue;
        };
        transform.rotation = rotation;
        transform.translation = centre - rotation * filed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const RADIUS: f32 = 300.0;

    /// A world holding just what [`advance`] reads: the orbiting level's
    /// resources, a player, and one scene root per body.
    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(LevelId::PlanetOrbit);
        world.insert_resource(GameTuning::default());
        let system = SolarSystem::default();
        world.insert_resource(Gravity::binary(
            system.bodies[0].centre,
            system.bodies[1].centre,
            RADIUS,
        ));
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, RADIUS, None));
        world.insert_resource(Respawn(Vec3::ZERO));
        for index in 0..2 {
            world.spawn((PlanetBody(index), Transform::default()));
        }
        let start = system.bodies[0].centre + Vec3::Y * RADIUS;
        world.insert_resource(system);
        world.spawn((
            Player,
            Controller::default(),
            Rider::default(),
            Transform::from_translation(start),
        ));
        world
    }

    fn rider(world: &mut World) -> Rider {
        let mut query = world.query_filtered::<&Rider, With<Player>>();
        *query.single(world).unwrap()
    }

    fn tick(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world.run_system_once(advance).expect("advance could not run");
        }
    }

    /// The parenting record reads like the ride feels: standing on the first
    /// planet he is that planet's rider at full grip, and carried out past
    /// the fade band he is nobody's -- the record `sync_visual` draws him
    /// through is never a world he is not actually held by.
    #[test]
    fn the_parents_books_name_who_holds_him() {
        let mut world = world();
        tick(&mut world, 1);
        let held = rider(&mut world);
        assert_eq!(held.world, Some(0), "on its surface he is planet one's");
        assert!(
            held.hold > 0.99,
            "full gravity should be a full grip, not {}",
            held.hold
        );
        let adrift = world.resource::<SolarSystem>().bodies[0].centre
            + Vec3::Y * (RADIUS + crate::gravity::GRAVITY_RANGE + crate::gravity::GRAVITY_FADE + 50.0);
        let mut query = world.query_filtered::<&mut Transform, With<Player>>();
        query.single_mut(&mut world).unwrap().translation = adrift;
        tick(&mut world, 1);
        let let_go = rider(&mut world);
        assert_eq!(let_go.world, None, "out past the fade band nobody holds him");
        assert_eq!(let_go.hold, 0.0);
    }

    fn player_at(world: &mut World) -> (Vec3, Vec3) {
        let mut query = world.query_filtered::<(&Transform, &Controller), With<Player>>();
        let (transform, ctrl) = query.single(world).unwrap();
        (transform.translation, ctrl.velocity)
    }

    /// The clockwork is the console's: a second of ticks moves each planet by
    /// its own `*_orbit` row along a circle of its own `*_dist` row, spins it
    /// by its `*_spin` row, and every reader -- gravity, collision, scenery --
    /// is told the same position.
    #[test]
    fn the_planets_run_the_rounds_the_console_sets() {
        let mut world = world();
        let opening = world.resource::<SolarSystem>().bodies[0];
        tick(&mut world, 30);
        let tuning = world.resource::<GameTuning>().clone();
        let system = world.resource::<SolarSystem>();
        for (index, body) in system.bodies.iter().enumerate() {
            let (distance, orbit, spin) = SolarSystem::tuned(&tuning, index);
            assert!(
                ((body.centre - SUN_CENTRE).length() - distance).abs() < 0.5,
                "body {index} orbits at {} rather than {distance}",
                (body.centre - SUN_CENTRE).length()
            );
            let expected = orbit.to_radians();
            let turned = body.angle
                - match index {
                    0 => 0.0,
                    _ => std::f32::consts::FRAC_PI_2,
                };
            assert!(
                (turned - expected).abs() < 1e-3,
                "body {index} swept {turned} rad in a second, not {expected}"
            );
            assert!(
                (body.spin - spin.to_radians()).abs() < 1e-3,
                "body {index} spun {} rad in a second",
                body.spin
            );
        }
        assert!(
            (system.bodies[0].centre - opening.centre).length() > 10.0,
            "a second of orbit moved the planet almost nowhere"
        );
        // Gravity pulls towards where the worlds now are.
        let gravity = *world.resource::<Gravity>();
        let Gravity::System { centres, .. } = gravity else {
            panic!("the system's gravity is {gravity:?}");
        };
        assert_eq!(centres, world.resource::<SolarSystem>().centres());
        // And the scene roots stand where the bodies do. The authored centre
        // is the origin here, so the root's translation is the body's centre.
        let mut roots = world.query::<(&PlanetBody, &Transform)>();
        let system_bodies = world.resource::<SolarSystem>().bodies;
        for (body, transform) in roots.iter(&world) {
            assert!(
                (transform.translation - system_bodies[body.0].centre).length() < 1e-3,
                "scene root {} is not under its planet",
                body.0
            );
        }
    }

    /// Standing on a planet that is doing twenty metres a second means doing
    /// twenty metres a second: the rider keeps his place on the ground --
    /// through the orbit and the spin both -- which is the whole reason the
    /// drag exists.
    #[test]
    fn a_grounded_player_rides_his_planet_round_the_sun() {
        let mut world = world();
        {
            let mut query = world.query_filtered::<&mut Controller, With<Player>>();
            query.single_mut(&mut world).unwrap().grounded = true;
        }
        let local = |world: &mut World| {
            let (at, _) = player_at(world);
            let body = world.resource::<SolarSystem>().bodies[0];
            body.rotation.inverse() * (at - body.centre)
        };
        let seat = local(&mut world);
        tick(&mut world, 90);
        let held = local(&mut world);
        assert!(
            (held - seat).length() < 1e-2,
            "three seconds of orbit slid the rider {} m across his own ground",
            (held - seat).length()
        );
        // And he genuinely went somewhere: the seat held because he moved,
        // not because nothing did.
        let (at, _) = player_at(&mut world);
        let start = SolarSystem::default().bodies[0].centre + Vec3::Y * RADIUS;
        assert!(
            (at - start).length() > 10.0,
            "the planet moved and the rider stayed behind"
        );
    }

    /// The other half of the handover: out in the weightless middle nothing
    /// drags you anywhere. The planets sail past a coasting body -- Outer
    /// Wilds' whole mood -- rather than towing all of space along.
    #[test]
    fn a_weightless_flyer_watches_the_planets_go_by() {
        let mut world = world();
        let adrift = SUN_CENTRE + Vec3::new(0.0, 2000.0, 0.0);
        {
            let mut query = world.query_filtered::<&mut Transform, With<Player>>();
            query.single_mut(&mut world).unwrap().translation = adrift;
        }
        tick(&mut world, 90);
        let (at, velocity) = player_at(&mut world);
        assert_eq!(at, adrift, "empty space towed the flyer along");
        assert_eq!(velocity, Vec3::ZERO);
    }

    /// You can get to the sun, and you stop *at* it: the surface holds, and
    /// the arrival speed is spent rather than banked against the day you
    /// bounce off.
    #[test]
    fn the_sun_has_a_surface_and_it_holds() {
        let mut world = world();
        {
            let mut query =
                world.query_filtered::<(&mut Transform, &mut Controller), With<Player>>();
            let (mut transform, mut ctrl) = query.single_mut(&mut world).unwrap();
            transform.translation = SUN_CENTRE + Vec3::X * (SUN_RADIUS - 40.0);
            ctrl.velocity = Vec3::NEG_X * 120.0;
        }
        tick(&mut world, 1);
        let (at, velocity) = player_at(&mut world);
        assert!(
            (at - SUN_CENTRE).length() >= SUN_RADIUS + PLAYER_RADIUS - 1e-3,
            "the player is {} m from the sun's centre, inside it",
            (at - SUN_CENTRE).length()
        );
        assert!(
            velocity.dot(Vec3::NEG_X) <= 1e-4,
            "the inbound speed survived the surface: {velocity:?}"
        );
    }

    /// The respawn point is a seat on the first planet, not a coordinate: as
    /// the planet goes round, where you come back to goes round with it.
    #[test]
    fn the_respawn_rides_the_first_planet() {
        let mut world = world();
        let seat = Vec3::Y * RADIUS;
        world.resource_mut::<SolarSystem>().respawn_local = Some(seat);
        tick(&mut world, 60);
        let body = world.resource::<SolarSystem>().bodies[0];
        let resolved = world.resource::<Respawn>().0;
        assert!(
            (resolved - (body.centre + body.rotation * seat)).length() < 1e-3,
            "the respawn was left at {resolved} while its planet moved on"
        );
    }

    /// What the screen rides: [`SolarSystem::blended`] runs each body from
    /// where the last tick began to where it ended, so a frame drawn between
    /// two ticks stands the planet between its two poses rather than on
    /// either -- the scenery's half of the player's own interpolation.
    #[test]
    fn the_drawn_planet_rides_between_two_ticks() {
        let mut world = world();
        tick(&mut world, 1);
        let system = world.resource::<SolarSystem>();
        assert_eq!(system.blended(0.0)[0].0, system.previous[0].centre);
        assert_eq!(system.blended(1.0)[0].0, system.bodies[0].centre);
        let expected = system.previous[0]
            .centre
            .lerp(system.bodies[0].centre, 0.5);
        assert_eq!(system.blended(0.5)[0].0, expected);
        // A tick genuinely separates the two, or this proves nothing.
        assert_ne!(system.previous[0].centre, system.bodies[0].centre);
    }

    /// Every other level is none of this module's business.
    #[test]
    fn the_castle_is_left_entirely_alone() {
        let mut world = world();
        world.insert_resource(LevelId::Castle);
        let before = world.resource::<SolarSystem>().bodies[0].centre;
        tick(&mut world, 30);
        assert_eq!(world.resource::<SolarSystem>().bodies[0].centre, before);
    }
}
