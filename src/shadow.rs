//! The dark circle under everything that moves.
//!
//! Ported from `shadow.c`. The N64 could not afford a shadow map, so SM64 drew
//! shadows as their own geometry: a small textured disc laid flat on whatever
//! floor was under the object, scaled down and faded out the higher the object
//! got above it. That is not a cheaper approximation of a real shadow, it is a
//! different thing with different rules -- it does not care what shape the
//! caster is, it never falls on a wall, and two objects standing on each other
//! both get one on the ground.
//!
//! The two curves below are the original's, and the distance they run over is
//! its 600 units at this port's scale of 1/100. Everything else here is the
//! machinery to keep one disc under each caster.
//!
//! What this does *not* do is bend the disc over a broken floor. SM64 draws
//! nine vertices and drops each onto the collision separately, so a shadow
//! creases where the ground creases; this draws one flat disc turned to face
//! the way the ground faces, which is right on a slope and wrong across the
//! join between two of them.

use crate::level::LevelData;
use bevy::{
    asset::RenderAssetUsages,
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    light::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// How far above its floor a caster has to get before its shadow has shrunk
/// and faded as far as it ever will: 600 units in `shadow.c`.
const FADE_DISTANCE: f32 = 6.0;

/// What a shadow settles at once its caster is [`FADE_DISTANCE`] up -- half the
/// size, and the 120-of-255 alpha `dim_shadow_with_distance` bottoms out at.
const FAR_SCALE: f32 = 0.5;
const FAR_SOLIDITY: f32 = 120.0 / 255.0;

/// How dark a shadow is with its caster stood on the ground. 200 of 255, which
/// is what the original gives Mario.
pub const SOLID: f32 = 200.0 / 255.0;

/// How far the disc floats off the floor, along the floor's own normal. Small
/// enough not to be seen from a playing camera and large enough that the disc
/// and the ground it sits on never argue about which is in front.
const LIFT: f32 = 0.03;

/// Segments around the disc. The original's circle shadow is nine vertices --
/// eight triangles in a fan -- and the reason a low number is fine here is the
/// same reason it was fine there: the texture fades to nothing before the rim,
/// so what the eye sees is the round edge of the fade and never the polygon.
const SEGMENTS: u32 = 24;

/// The disc texture, in pixels across, and where its fade runs between as a
/// fraction of its radius: solid to the first, gone by the second.
const TEXTURE_SIZE: u32 = 64;
const CORE: f32 = 0.62;
const EDGE: f32 = 1.0;

/// Something that gets a shadow drawn under it.
///
/// A component rather than a rule about which kinds of thing have shadows, so
/// the pipes' brood gets one the moment it exists without the pipe knowing
/// anything about shadows -- it spawns enemies and allies, and those carry it.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShadowCaster {
    /// The disc's radius in world units with the caster on the ground. Not
    /// derived from the model: a shadow is a readability device and wants to be
    /// about as wide as the caster's feet, not as wide as its widest part.
    pub radius: f32,
    /// How dark it is at full strength, 0 to 1.
    pub solidity: f32,
}

impl ShadowCaster {
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            solidity: SOLID,
        }
    }
}

/// A drawn disc, and whose it is.
#[derive(Component)]
pub struct Shadow {
    owner: Entity,
}

/// Marks a caster that already has a disc, so [`attach`] can find the ones that
/// do not by filtering rather than by searching the shadows for each.
///
/// Which disc is not recorded, because nothing needs it: a shadow knows its
/// owner and that direction is the one every question here is asked in.
#[derive(Component)]
pub struct HasShadow;

/// The mesh and texture every shadow shares. One of each for the whole world:
/// the disc is the same disc whoever is standing on it, and only the transform
/// and the alpha differ.
#[derive(Resource)]
pub struct ShadowArt {
    mesh: Handle<Mesh>,
    texture: Handle<Image>,
}

/// Builds the shared disc. Called once, from startup.
pub fn prepare(commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) {
    commands.insert_resource(ShadowArt {
        // A unit circle in the XY plane facing +Z, which is why [`project`]
        // turns it by the rotation that takes +Z onto the floor's normal.
        mesh: meshes.add(Circle::new(1.0).mesh().resolution(SEGMENTS).build()),
        texture: images.add(disc()),
    });
}

/// The shadow texture: black, with the alpha falling from solid at the middle
/// to nothing at the rim.
///
/// Generated rather than shipped. It is a formula either way, and a formula in
/// the source is one fewer file for the Windows packaging step to be told
/// about.
fn disc() -> Image {
    let size = TEXTURE_SIZE as f32;
    let mut pixels = Vec::with_capacity((TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize);
    for y in 0..TEXTURE_SIZE {
        for x in 0..TEXTURE_SIZE {
            // Where this pixel sits in the disc, as a fraction of its radius.
            let across = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / size * 2.0 - Vec2::ONE;
            let alpha = 1.0 - smooth_step(CORE, EDGE, across.length());
            pixels.extend_from_slice(&[0, 0, 0, (alpha * 255.0).round() as u8]);
        }
    }
    Image::new(
        Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        // Nothing reads this back once it is on the GPU.
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn smooth_step(from: f32, to: f32, at: f32) -> f32 {
    let t = ((at - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `scale_shadow_with_distance` from `shadow.c`: a shadow shrinks to half its
/// size as its caster rises, and no further.
pub fn scale_with_drop(drop: f32) -> f32 {
    if drop <= 0.0 {
        1.0
    } else if drop >= FADE_DISTANCE {
        FAR_SCALE
    } else {
        1.0 - (1.0 - FAR_SCALE) * drop / FADE_DISTANCE
    }
}

/// `dim_shadow_with_distance` from `shadow.c`: it fades towards, but never
/// past, [`FAR_SOLIDITY`]. Something already fainter than that is left alone,
/// which is the original's `solidity < 121` early return.
pub fn dim_with_drop(solidity: f32, drop: f32) -> f32 {
    if solidity <= FAR_SOLIDITY || drop <= 0.0 {
        solidity
    } else if drop >= FADE_DISTANCE {
        FAR_SOLIDITY
    } else {
        solidity + (FAR_SOLIDITY - solidity) * drop / FADE_DISTANCE
    }
}

/// Gives every caster a disc, and reclaims the discs of casters that are gone.
///
/// Each shadow gets a material of its own because its alpha is its own: they
/// share the mesh and the texture, which is the expensive part, and differ by
/// one colour in one small uniform.
pub fn attach(
    mut commands: Commands,
    art: Res<ShadowArt>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    new_casters: Query<Entity, (With<ShadowCaster>, Without<HasShadow>)>,
    casters: Query<(), With<ShadowCaster>>,
    shadows: Query<(Entity, &Shadow)>,
) {
    for owner in &new_casters {
        commands.spawn((
            Shadow { owner },
            Mesh3d(art.mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, SOLID),
                base_color_texture: Some(art.texture.clone()),
                alpha_mode: AlphaMode::Blend,
                // A shadow is a shadow, not a surface: nothing about it
                // should change with where the sun is.
                unlit: true,
                // Seen from below through a water sheet, or from under a
                // floor the caster is standing on the far side of.
                double_sided: true,
                cull_mode: None,
                ..default()
            })),
            Transform::default(),
            // Placed by `project` before it is ever shown, so that a shadow
            // spawned this frame is not drawn at the origin for one frame.
            Visibility::Hidden,
            NotShadowCaster,
            NotShadowReceiver,
        ));
        commands.entity(owner).insert(HasShadow);
    }
    for (entity, shadow) in &shadows {
        if casters.get(shadow.owner).is_err() {
            commands.entity(entity).despawn();
            // The owner has usually been despawned -- an ally the population
            // count dropped, an enemy that was stomped -- but it may merely
            // have stopped casting. `try_` because those two cases are told
            // apart by the entity still being there when the command runs.
            commands.entity(shadow.owner).try_remove::<HasShadow>();
        }
    }
}

/// Drops each disc onto the floor under its caster and sizes it for how far up
/// the caster is.
///
/// `Without<Shadow>` on the casters is the usual disjointness proof: both
/// queries reach `Transform` and `Visibility`, and Bevy takes nothing on trust
/// from the fact that a shadow is never its own caster.
#[allow(clippy::type_complexity)]
pub fn project(
    level: Res<LevelData>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    casters: Query<(&Transform, &ShadowCaster, Option<&Visibility>), Without<Shadow>>,
    mut shadows: Query<(
        &Shadow,
        &mut Transform,
        &mut Visibility,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    for (shadow, mut transform, mut visibility, material) in &mut shadows {
        let Ok((caster, settings, shown)) = casters.get(shadow.owner) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        // A caster nobody is drawing casts nothing. This is what keeps the far
        // half of the field's enemies from leaving a carpet of discs behind
        // them once `enemy::update` has culled the enemies themselves.
        if shown == Some(&Visibility::Hidden) {
            *visibility = Visibility::Hidden;
            continue;
        }
        let here = caster.translation;
        // The floor query already tolerates half a unit above the point, so a
        // caster standing exactly on the ground finds the ground it is on.
        let Some((floor, up)) = level.floor_at(here) else {
            // Over open space -- off the edge of the map, or above a hole.
            // There is nothing for a shadow to land on.
            *visibility = Visibility::Hidden;
            continue;
        };
        *visibility = Visibility::Visible;
        let drop = (here.y - floor).max(0.0);
        // Lifted along the floor's normal rather than straight up, so the
        // clearance is the same all the way round the disc on a slope.
        transform.translation = Vec3::new(here.x, floor, here.z) + up * LIFT;
        transform.rotation = Quat::from_rotation_arc(Vec3::Z, up);
        transform.scale = Vec3::splat(settings.radius * scale_with_drop(drop));
        if let Some(mut material) = materials.get_mut(&material.0) {
            material
                .base_color
                .set_alpha(dim_with_drop(settings.solidity, drop));
        }
    }
}

pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (attach, project).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two curves, at the three points the original pins them at.
    #[test]
    fn a_shadow_shrinks_and_fades_with_height_and_then_stops() {
        assert_eq!(scale_with_drop(0.0), 1.0);
        assert_eq!(scale_with_drop(FADE_DISTANCE / 2.0), 0.75);
        assert_eq!(scale_with_drop(FADE_DISTANCE), FAR_SCALE);
        assert_eq!(
            scale_with_drop(FADE_DISTANCE * 10.0),
            FAR_SCALE,
            "a shadow kept shrinking past the distance it is supposed to stop at"
        );
        assert_eq!(dim_with_drop(SOLID, 0.0), SOLID);
        assert_eq!(dim_with_drop(SOLID, FADE_DISTANCE), FAR_SOLIDITY);
        assert_eq!(dim_with_drop(SOLID, FADE_DISTANCE * 10.0), FAR_SOLIDITY);
        // Halfway up, halfway between the two.
        let middle = dim_with_drop(SOLID, FADE_DISTANCE / 2.0);
        assert!(
            (middle - (SOLID + FAR_SOLIDITY) / 2.0).abs() < 1e-6,
            "{middle}"
        );
    }

    /// The fade only ever dims. Something already fainter than the floor value
    /// must not be *brightened* by rising, which is what the original's early
    /// return is there to prevent.
    #[test]
    fn a_faint_shadow_is_not_brightened_by_height() {
        let faint = FAR_SOLIDITY / 2.0;
        for drop in [0.0, 1.0, FADE_DISTANCE, 100.0] {
            assert_eq!(dim_with_drop(faint, drop), faint);
        }
    }

    /// The texture is what makes an octagon read as a circle, so the fade has
    /// to actually reach nothing by the rim -- and has to be solid in the
    /// middle, or the shadow is a ring.
    #[test]
    fn the_disc_is_solid_in_the_middle_and_gone_at_the_rim() {
        let image = disc();
        let alpha = |x: u32, y: u32| {
            image.data.as_ref().unwrap()[((y * TEXTURE_SIZE + x) * 4 + 3) as usize]
        };
        assert_eq!(alpha(TEXTURE_SIZE / 2, TEXTURE_SIZE / 2), 255);
        // The corners are outside the disc entirely.
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(TEXTURE_SIZE - 1, TEXTURE_SIZE - 1), 0);
        // The middle of each edge is the rim itself. Not exactly zero, because
        // the last row of pixel *centres* sits half a pixel inside the rim, and
        // one part in 255 is the whole of what is left there.
        assert!(alpha(TEXTURE_SIZE / 2, 0) <= 2);
        assert!(alpha(0, TEXTURE_SIZE / 2) <= 2);
    }

    /// A disc turned to face the floor's normal lies flat on it. Checked on the
    /// level case and on a slope, because the level case would also pass if the
    /// rotation were dropped altogether -- and then every shadow in the game
    /// would stand on edge, facing the camera like a coin.
    #[test]
    fn a_disc_lies_flat_on_whatever_it_is_dropped_onto() {
        for up in [
            Vec3::Y,
            Vec3::new(0.4, 1.0, 0.0).normalize(),
            Vec3::new(-0.3, 1.0, 0.6).normalize(),
        ] {
            let face = Quat::from_rotation_arc(Vec3::Z, up) * Vec3::Z;
            assert!(
                (face - up).length() < 1e-5,
                "a disc dropped onto {up:?} faces {face:?}"
            );
        }
    }
}
