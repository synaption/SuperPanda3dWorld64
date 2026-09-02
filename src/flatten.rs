//! The world drawn flat under your feet, while staying round underneath.
//!
//! Standing on a three-hundred-metre planet, the ground genuinely curves away:
//! walk towards a thing and it climbs your view as the sphere rolls it up over
//! the horizon, and the camera -- which holds itself level against local up --
//! turns under you the whole way. That is the physics being honest, and it is
//! also why aiming at anything further than a stone's throw feels like aiming
//! from a rolling deck. The user's words: the stepping "is obviously due to
//! the planet being round".
//!
//! So the *picture* is unbent, and only the picture. This module measures one
//! [`Curve`] per frame -- which world the player rides, where its centre is
//! drawn, which way is up under his feet, and how hard the flattening is
//! gripping -- and `n64.wgsl` applies it to every vertex on its way to clip
//! space. Nothing else is touched: transforms, colliders, gravity, navigation
//! and every gameplay query keep working on the true sphere, and the shader's
//! own lighting, lamplight and fog all read the *true* world position too, so
//! the flattening changes where a triangle lands on the glass and nothing
//! about what colour it is.
//!
//! The mapping is the azimuthal equidistant one -- the exponential map, if you
//! ask a geometer -- taken about the player's own radial line. A vertex at
//! polar angle `θ` from the player's zenith, `h` metres above sea level, is
//! put down on the tangent plane `R·θ` metres out in the direction it was
//! walked in, `h` metres up. Three properties pay for the choice: walking
//! distances are preserved exactly (a path `R·θ` long on the sphere is `R·θ`
//! long on the map, so nothing near you swims as you move), directions from
//! the player are preserved exactly (a thing due east is drawn due east, which
//! is the aiming half), and the map is smooth everywhere but the single point
//! on the far side of the world. The alternatives lose one or the other: a
//! plain tangent projection (`V` dropped onto the plane) compresses distances
//! towards the horizon and cannot see past it at all, and cancelling only the
//! sagitta (pushing vertices radially) keeps the horizon's foreshortening,
//! which is the very thing being complained about.
//!
//! Three seams are worth knowing about. The map is anchored to the player's
//! *drawn* feet over the world's *blended* centre, the same frame everything
//! else on the screen rides, so the anchor never disagrees with the terrain
//! about where the ground is this frame. The grip is the [`Rider`]'s own hold
//! -- the gravity fraction -- so the world relaxes back to its true shape
//! through the same fade band that lets go of the camera and the ride, and in
//! open space, where there is no parent, the map is off and world space is
//! honest. And each *vertex* fades out of the map with altitude over the
//! gravity band too, which is what keeps the sun, the other planet and
//! everything else beyond this world's grip standing exactly where the true
//! sky has them.

use crate::{
    console::GameTuning,
    gravity::{GRAVITY_FADE, GRAVITY_RANGE},
    level::{LevelData, Shape},
    orbit::{Rider, SolarSystem},
    player::{Player, RenderPose},
    world::LevelId,
};
use bevy::{
    asset::uuid_handle, camera::visibility::NoFrustumCulling, prelude::*,
    render::storage::ShaderBuffer,
};

/// The one buffer every material's curve binding points at, rewritten once a
/// frame by [`chart`]. The same shape of thing as [`crate::n64::LAMPLIGHT`]
/// and for the same reason: the anchor moves every frame the player does, and
/// per-frame data must not live in a material's uniform or moving it would
/// rewrite every material in the world -- see [`crate::n64::relight`] for
/// what that costs.
pub const CURVE: Handle<ShaderBuffer> = uuid_handle!("3f9a6c2d-71b5-4e08-8c44-2d9e5a7b6f10");

/// Past this many map-metres per sphere-metre the unrolling stops stretching.
///
/// The exponential map is singular at the antipode: the point exactly under
/// the player's feet on the far side is every direction at once, and the
/// arithmetic there divides by a sine that has reached zero. Uncapped, a
/// triangle with one vertex in that last fraction of a degree is flung
/// millions of metres and drawn as a sliver across the whole sky. Capped, the
/// far cap folds gently back on itself instead -- at a 300 m radius the cap
/// only engages inside the last three degrees around the antipode, which is
/// over 900 map-metres out and several fog depths past visible.
const STRETCH: f32 = 8.0;

/// How the round world is being drawn flat this frame.
///
/// One resource and one buffer holding the same numbers: the resource is for
/// anything on the CPU that projects a world position onto the glass and
/// wants to agree with the picture (call [`Curve::bend`] on the point first),
/// and the buffer is the copy the shader reads. `bend` here and `n64_unbend`
/// in `n64.wgsl` must stay the same function -- this one is the reference,
/// because this one is the one the tests can reach.
#[derive(Resource, Clone, Copy, bevy::render::render_resource::ShaderType, Debug, PartialEq)]
pub struct Curve {
    /// `xyz` the ridden world's centre as *drawn* this frame -- the blended
    /// pose, not the tick pose. `w` its sea-level radius.
    home: Vec4,
    /// `xyz` unit up under the player's drawn feet: the map's axis. `w` the
    /// grip, `0..=1` -- zero is the true sphere and one is the full map.
    zenith: Vec4,
    /// `x` metres above sea level where a vertex starts to slip off the map,
    /// `y` where it is free of it. `zw` unused: a uniform's fields align to
    /// sixteen bytes whatever their size, the same reasoning as
    /// [`crate::n64::N64Uniform`].
    band: Vec4,
}

/// A curve that bends nothing, which is every level that is not the solar
/// system and every frame spent adrift between its worlds.
impl Default for Curve {
    fn default() -> Self {
        Self {
            home: Vec4::ZERO,
            zenith: Vec3::Y.extend(0.0),
            band: Vec4::new(GRAVITY_RANGE, GRAVITY_RANGE + GRAVITY_FADE, 0.0, 0.0),
        }
    }
}

impl Curve {
    /// The map for a player held to the world centred at `centre`, standing
    /// along `up` from it, at `strength` of full grip.
    ///
    /// The altitude band is the gravity's own fade band on purpose: the
    /// picture lets go of a climbing vertex exactly where the pull lets go of
    /// a climbing player, so the two stories about leaving a planet are one
    /// story.
    pub fn over(centre: Vec3, radius: f32, up: Vec3, strength: f32) -> Self {
        Self {
            home: centre.extend(radius),
            zenith: up.normalize_or(Vec3::Y).extend(strength.clamp(0.0, 1.0)),
            ..default()
        }
    }

    /// Where the flattening draws a true world position.
    ///
    /// The arithmetic, in full, because the WGSL copy has to match it line
    /// for line:
    ///
    ///  1. measure the vertex off the world's centre -- its radial height
    ///     `h` above sea level and its unit direction `dir`;
    ///  2. split `dir` against the zenith: the polar angle `θ` between them,
    ///     and the horizontal unit direction the vertex lies in;
    ///  3. put the vertex down on the tangent plane at the zenith: `R·θ` out
    ///     along that horizontal direction -- the *geodesic* distance, so a
    ///     walk keeps its length -- and `h` up, so towers stay towers;
    ///  4. blend the true position towards that by the grip, faded per vertex
    ///     over the altitude band so only this world's neighbourhood takes
    ///     part.
    ///
    /// The horizontal direction comes out of `dir - up·cosθ`, whose length is
    /// exactly `sinθ`, so the unrolling multiplies by `θ/sinθ` -- which is 1
    /// at the zenith (the map touches the sphere under your feet), grows
    /// gently with distance, and is capped by [`STRETCH`] at the antipode
    /// where it would otherwise be infinite.
    pub fn bend(&self, world: Vec3) -> Vec3 {
        let grip = self.zenith.w;
        if grip <= 0.0 {
            return world;
        }
        let (centre, radius) = (self.home.truncate(), self.home.w);
        let up = self.zenith.truncate();
        let arm = world - centre;
        let reach = arm.length();
        // The centre itself has no direction to be unrolled in, the same
        // degeneracy `Gravity::up` guards. Nothing drawable lives there.
        if reach < 1e-3 {
            return world;
        }
        let height = reach - radius;
        let share = grip * (1.0 - smoothstep(self.band.x, self.band.y, height));
        if share <= 0.0 {
            return world;
        }
        let dir = arm / reach;
        let cos_polar = dir.dot(up).clamp(-1.0, 1.0);
        let polar = cos_polar.acos();
        let level = dir - up * cos_polar;
        let sin_polar = level.length();
        let stretch = if sin_polar > 1e-6 {
            (polar / sin_polar).min(STRETCH)
        } else {
            // Directly overhead or dead astern: no horizontal part to
            // stretch, so the factor is moot -- 1.0 keeps the maths finite.
            1.0
        };
        let flat = centre + up * (radius + height) + level * (radius * stretch);
        world.lerp(flat, share)
    }
}

/// The GLSL curve everyone knows, which WGSL has as a built-in and Rust does
/// not. Here so `bend` above and `n64_unbend` in the shader stay one function.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Measures this frame's [`Curve`] and writes it where the shader reads.
///
/// After [`crate::player::sync_visual`], because the anchor is the player's
/// *drawn* position over the world's *blended* centre -- the map has to be
/// pinned to the frame being painted, not the tick being simulated, or the
/// terrain and the map disagree by up to a tick of orbital motion and the
/// stutter this whole chain of work has been hunting comes straight back
/// through the shader.
///
/// The buffer is written every frame whatever the level, like
/// [`crate::n64::lamplight`] and for the same reason: every material binds
/// it, and a binding that names an asset that does not exist draws nothing
/// at all.
pub fn chart(
    fixed: Res<Time<Fixed>>,
    id: Res<LevelId>,
    tuning: Res<GameTuning>,
    system: Res<SolarSystem>,
    collision: Res<LevelData>,
    pose: Res<RenderPose>,
    riders: Query<&Rider, With<Player>>,
    mut curve: ResMut<Curve>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let mut measured = Curve::default();
    if *id == LevelId::PlanetOrbit {
        if let Shape::Planet { radius, .. } = collision.shape() {
            let rider = riders.single().ok().copied().unwrap_or_default();
            if let Rider {
                world: Some(held),
                hold,
            } = rider
            {
                if hold > 0.0 {
                    let alpha = fixed.overstep_fraction().clamp(0.0, 1.0);
                    let (centre, _) = system.blended(alpha)[held];
                    let up = (pose.translation - centre).normalize_or(Vec3::Y);
                    // The grip is the rider's hold times the console row: the
                    // hold is what fades the map out through the gravity band,
                    // and the row is the debug toggle -- `flatten 0` is the
                    // true sphere for the price of one console line.
                    measured = Curve::over(centre, radius, up, hold * tuning.flatten);
                }
            }
        }
    }
    *curve = measured;
    let _ = buffers.insert(&CURVE, ShaderBuffer::from(measured));
}

/// Turns Bevy's frustum culling off for everything on the orbiting level.
///
/// The culler tests each mesh's bounding box where the mesh *truly* stands,
/// and the shader then bends it somewhere else -- so ground that the map has
/// lifted into the bottom of a sky-facing view sits, in true space, in a
/// sphere below and behind the frustum, and vanishes the moment the culler
/// looks. Bevy has no hook for "cull against where the vertex shader will put
/// it", so the honest fix is not to cull: the solar system is two planet
/// scenes, a handful of pylons and whatever flies, which is nothing beside
/// the castle's crowds, and drawing all of it always costs less than one
/// pop-in at the edge of the glass.
///
/// The marker is only ever added, never removed: the level's own meshes take
/// theirs with them when the level is torn down, and the few that persist
/// across levels -- the player, the squad -- are the things the camera never
/// has out of frame anyway.
pub fn uncull(
    mut commands: Commands,
    id: Res<LevelId>,
    fenced: Query<Entity, (With<Mesh3d>, Without<NoFrustumCulling>)>,
) {
    if *id != LevelId::PlanetOrbit {
        return;
    }
    for entity in &fenced {
        commands.entity(entity).insert(NoFrustumCulling);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const RADIUS: f32 = 300.0;
    const CENTRE: Vec3 = Vec3::new(2600.0, 0.0, 0.0);

    fn full_grip() -> Curve {
        Curve::over(CENTRE, RADIUS, Vec3::Y, 1.0)
    }

    /// A point on the sphere `polar` radians from the zenith, walked off in
    /// the `+X` horizontal direction, `height` metres above sea level.
    fn on_the_sphere(polar: f32, height: f32) -> Vec3 {
        let dir = Quat::from_rotation_z(-polar) * Vec3::Y;
        CENTRE + dir * (RADIUS + height)
    }

    /// The map touches the sphere at the player: his own ground, his own
    /// model and everything at his feet are drawn exactly where they truly
    /// are, which is what keeps the near field rock-steady as the grip
    /// changes.
    #[test]
    fn the_ground_underfoot_is_left_where_it_stands() {
        let curve = full_grip();
        let feet = CENTRE + Vec3::Y * RADIUS;
        assert!(curve.bend(feet).distance(feet) < 1e-3);
        let head = CENTRE + Vec3::Y * (RADIUS + 1.8);
        assert!(curve.bend(head).distance(head) < 1e-3);
    }

    /// A walk keeps its length: sixty degrees round the sphere is `R·θ`
    /// metres of ground, and the map puts it down `R·θ` metres out on the
    /// tangent plane, dead level, in the direction it was walked.
    #[test]
    fn a_walk_unrolls_to_its_own_length_on_the_level_plane() {
        let curve = full_grip();
        let polar = 60_f32.to_radians();
        let flat = curve.bend(on_the_sphere(polar, 0.0));
        let anchor = CENTRE + Vec3::Y * RADIUS;
        let laid = flat - anchor;
        assert!(
            laid.dot(Vec3::Y).abs() < 1e-2,
            "sea level did not land on the plane: {laid}"
        );
        assert!(
            (laid.length() - RADIUS * polar).abs() < 1e-2,
            "the walk changed length: {} for {}",
            laid.length(),
            RADIUS * polar
        );
        assert!(
            laid.normalize().dot(Vec3::X) > 0.999,
            "the walk changed direction: {laid}"
        );
    }

    /// Height is carried straight up off the plane, so a tower on the far
    /// side of a hill is still a tower: same spot on the map as its own
    /// foundations, its height between them.
    #[test]
    fn a_tower_stays_a_tower() {
        let curve = full_grip();
        let polar = 40_f32.to_radians();
        let foot = curve.bend(on_the_sphere(polar, 0.0));
        let top = curve.bend(on_the_sphere(polar, 50.0));
        assert!(
            (top - foot).distance(Vec3::Y * 50.0) < 1e-2,
            "the tower leaned: {}",
            top - foot
        );
    }

    /// Only this world takes part. The sun and the second planet are far
    /// outside the altitude band, and they must stand exactly where the true
    /// sky has them or the autopilot's bracket and the picture part company.
    #[test]
    fn the_rest_of_the_system_is_left_alone() {
        let curve = full_grip();
        for far in [
            Vec3::ZERO,                     // the sun
            Vec3::new(0.0, 0.0, 4200.0),    // the other world, roughly
            CENTRE + Vec3::Y * (RADIUS + GRAVITY_RANGE + GRAVITY_FADE + 1.0),
        ] {
            assert_eq!(curve.bend(far), far, "something out of the band moved");
        }
    }

    /// The grip is a dial, not a switch: no hold is the true sphere, and half
    /// a hold draws everything halfway between the sphere and the map, which
    /// is what makes leaving a planet a fade instead of a snap.
    #[test]
    fn the_grip_lets_go_by_degrees() {
        let world = on_the_sphere(1.0, 5.0);
        let sphere = Curve::over(CENTRE, RADIUS, Vec3::Y, 0.0).bend(world);
        assert_eq!(sphere, world);
        let flat = full_grip().bend(world);
        let half = Curve::over(CENTRE, RADIUS, Vec3::Y, 0.5).bend(world);
        assert!(
            half.distance(world.lerp(flat, 0.5)) < 1e-3,
            "half a grip was not half the map"
        );
        assert_eq!(Curve::default().bend(world), world);
    }

    /// The far side of the world folds instead of flying: the exponential
    /// map's one singular point is capped, so the whole sphere lands within a
    /// bounded, fog-deep disc and no triangle is flung across the sky.
    #[test]
    fn the_antipode_folds_instead_of_flying() {
        let curve = full_grip();
        for nearly in [
            CENTRE - Vec3::Y * RADIUS,
            CENTRE + Quat::from_rotation_z(179.9_f32.to_radians()) * (Vec3::Y * RADIUS),
        ] {
            let flat = curve.bend(nearly);
            assert!(flat.is_finite(), "the antipode was not finite: {flat}");
            assert!(
                flat.distance(CENTRE) < RADIUS * (1.0 + STRETCH),
                "the far side flew off the map: {flat}"
            );
        }
    }

    /// The whole road, resource to buffer: a held rider on the orbiting level
    /// writes a gripping curve where the shader reads, and an empty hold
    /// writes a flat-off one. What a test through [`Curve::over`] alone would
    /// not catch is the buffer never being written, or written in a layout
    /// the shader does not read -- the same reasoning as
    /// [`crate::n64::Lamplight::read`].
    #[test]
    fn the_chart_measures_the_ground_being_ridden() {
        let mut world = World::new();
        world.init_resource::<Assets<ShaderBuffer>>();
        world.insert_resource(Time::<Fixed>::from_hz(30.0));
        world.insert_resource(LevelId::PlanetOrbit);
        world.init_resource::<GameTuning>();
        world.init_resource::<Curve>();
        let system = SolarSystem::default();
        let centre = system.bodies[0].centre;
        world.insert_resource(system);
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, RADIUS, None));
        world.insert_resource(RenderPose {
            translation: centre + Vec3::Y * (RADIUS + 2.0),
            rotation: Quat::IDENTITY,
        });
        let rider = world
            .spawn((
                Player,
                Rider {
                    world: Some(0),
                    hold: 1.0,
                },
            ))
            .id();

        world.run_system_once(chart).expect("the chart should run");
        let held = {
            let buffers = world.resource::<Assets<ShaderBuffer>>();
            Curve::read(buffers.get(&CURVE).expect("the buffer should exist"))
        };
        assert!(held.zenith.w > 0.99, "a full hold did not grip the map");
        assert!(
            (held.home.truncate() - centre).length() < 1e-3,
            "the map was drawn over the wrong world"
        );
        assert!(held.zenith.truncate().dot(Vec3::Y) > 0.999);

        // And adrift -- no hold -- the map is off, whatever level is up.
        world.entity_mut(rider).insert(Rider::default());
        world.run_system_once(chart).expect("the chart should run");
        let adrift = {
            let buffers = world.resource::<Assets<ShaderBuffer>>();
            Curve::read(buffers.get(&CURVE).expect("the buffer should exist"))
        };
        assert_eq!(adrift.zenith.w, 0.0, "an adrift player still got a map");
    }
}

#[cfg(test)]
impl Curve {
    /// Reads one back out of the bytes the GPU would have been handed -- the
    /// test-side of the buffer, exactly as [`crate::n64::Lamplight::read`].
    pub fn read(buffer: &ShaderBuffer) -> Self {
        use bevy::render::render_resource::encase;
        let bytes = buffer.data.clone().unwrap_or_default();
        encase::StorageBuffer::new(bytes)
            .create()
            .expect("the curve buffer was not a curve")
    }
}
