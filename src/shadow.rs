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

use crate::{
    gravity::Gravity,
    level::{LevelData, Shape},
};
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

/// How many alphas the fade is allowed to take across the band it covers.
///
/// A shadow's alpha is a continuous function of how far its caster is off the
/// ground, and following it exactly costs more than it looks. Writing
/// `material.base_color` is not an ordinary field assignment: it marks the
/// material asset changed, and a changed material is re-extracted into the
/// render world and has its bind group rebuilt before the next draw. Every disc
/// in the level paid that on every frame -- and because no two discs shared a
/// material, no two of them could be batched into one draw either.
///
/// So the fade is quantised onto a ladder of materials built once at startup
/// and never written to again. A shadow picks the nearest rung, which costs a
/// handle comparison, and every disc at the same height ends up on the same
/// handle and batches. Twelve steps across the band [`dim_with_drop`] covers is
/// about six of the original's 255 alpha levels per step, on a black blob that
/// is already soft at the edges.
const FADE_STEPS: usize = 12;

/// How far the disc floats off the floor, along the floor's own normal. Small
/// enough not to be seen from a playing camera and large enough that the disc
/// and the ground it sits on never argue about which is in front.
pub const LIFT: f32 = 0.03;

/// How far in front of where it lies a disc is *drawn*, as a multiple of its
/// own radius.
///
/// [`LIFT`] answers a different question and only that one: two surfaces at the
/// same height arguing about which is in front. It cannot answer this one. A
/// disc is a flat thing laid on ground that is not flat, and on the castle's
/// lawn the ground rises across the width of the disc by far more than three
/// centimetres -- so the uphill half of it is *inside the hill*, and what the
/// depth buffer draws is no shadow at all. On the slope west of the castle both
/// kinds of disc vanish outright: the far crowd's squares are laid flat by
/// [`crate::impostor::build_shadows`] and bury themselves on any slope, and the
/// near ones bury themselves wherever [`place`] could not agree on a slope and
/// fell back to level.
///
/// Raising the disc instead is the obvious move and the wrong one: enough lift
/// to clear a hillside is enough lift to be seen floating on the flat, and the
/// amount needed is a property of the ground rather than of the shadow.
///
/// So the disc is drawn nearer than it lies, by the same trick
/// [`crate::impostor::float_toward`] uses on the sprites: scaled about the eye,
/// which under a perspective projection leaves it on exactly the pixels it was
/// on at exactly the size, and changes only the depth it is tested at.
///
/// One radius, because that is what the geometry asks for. A disc of radius `r`
/// laid flat on ground leaning at an angle buries its uphill rim by `r` times
/// the tangent of that angle, so one radius of clearance carries it up to
/// forty-five degrees -- past anything a caster can stand on. What it costs is
/// the disc winning the depth test against whatever stands within a radius in
/// front of it, and [`HEADROOM`] is what keeps that from mattering.
pub const FLOAT: f32 = 1.0;

/// The share of its caster's height a disc may be brought forward by.
///
/// The thing standing on a disc is the one thing the disc must never be drawn
/// in front of, and what protects a caster is how much *deeper* than the float
/// its own body reaches. That is a question about height, not about width, and
/// the two part company on the slime: a dome about as high as it is wide, whose
/// disc floated by its radius is a disc floated through its face. The ant is
/// tall and never notices. So both are asked and the smaller answer wins.
///
/// A quarter rather than a half, measured. A caster's advantage in depth over
/// its own disc is its full height only with the camera directly overhead and
/// shrinks toward nothing as the camera comes down to its level, so half was
/// already too much from a playing camera -- it drew a dark bar across the
/// slime. A quarter leaves the slime clean and still recovers a sixth of the
/// shadow the hillside was eating.
const HEADROOM: f32 = 0.25;

/// How far in front of where it lies a disc of this size may be drawn.
pub fn clearance(radius: f32, height: f32) -> f32 {
    (radius * FLOAT).min(height * HEADROOM)
}

/// How much of the way from the eye to `at` a disc of this radius is drawn at,
/// so that it comes forward by [`clearance`].
///
/// Taken from the centre and applied to the whole disc, which is what keeps the
/// scale uniform and therefore invisible. The rim furthest from the eye -- the
/// buried one, on a slope -- is further away than the centre, so it moves
/// forward by more than the centre does and is the corner this is really for.
///
/// Guarded at both ends: a disc at the eye has no ray to slide along, and one
/// whose radius is larger than its distance from the camera must not be turned
/// inside out through it.
pub fn float_toward(eye: Vec3, at: Vec3, clearance: f32) -> f32 {
    let reach = (at - eye).length();
    let wanted = reach - clearance;
    if reach <= f32::EPSILON || wanted <= 0.0 {
        return 1.0;
    }
    wanted / reach
}

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
    /// How tall the thing standing on the disc is. Nothing about the disc is
    /// drawn from it -- it is here to bound the eye-ward float, which is the
    /// one number that decides whether correcting a buried shadow turns into a
    /// shadow drawn over its own caster. See [`clearance`].
    pub height: f32,
    /// How dark it is at full strength, 0 to 1.
    pub solidity: f32,
}

impl ShadowCaster {
    pub fn new(radius: f32, height: f32) -> Self {
        Self {
            radius,
            height,
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

/// Everything every shadow draws with. One mesh and one texture for the whole
/// world -- the disc is the same disc whoever is standing on it -- and a ladder
/// of materials differing only in alpha, which is the one thing that does vary.
#[derive(Resource)]
pub struct ShadowArt {
    mesh: Handle<Mesh>,
    /// One material per rung of [`ShadowArt::ladder`], darkest first. Written
    /// once at startup and only ever read afterwards.
    fades: Vec<Handle<StandardMaterial>>,
}

impl ShadowArt {
    /// The alpha between one rung of the ladder and the next.
    fn rung() -> f32 {
        (SOLID - FAR_SOLIDITY) / (FADE_STEPS - 1) as f32
    }

    /// The alphas the ladder is built at: [`SOLID`] and then every rung down
    /// from it, as far as there is anything left to draw.
    ///
    /// Counted down from `SOLID` rather than up from nothing so that both ends
    /// of the band the fade actually uses land exactly on a rung -- a shadow on
    /// the ground is [`SOLID`] to the bit, not [`SOLID`] plus a rounding error.
    /// It carries on past [`FAR_SOLIDITY`] to the bottom so a caster given a
    /// fainter solidity than this port's one still gets the shadow it asked for
    /// rather than the faintest one in the band.
    fn ladder() -> impl Iterator<Item = f32> {
        let rung = Self::rung();
        (0..=(SOLID / rung) as usize).map(move |step| SOLID - step as f32 * rung)
    }

    /// Which rung `alpha` belongs on, given how many there are.
    ///
    /// Nearest rather than next-faintest, so the error is half a rung either
    /// way instead of a whole rung too light, and clamped at both ends.
    fn rung_for(alpha: f32, rungs: usize) -> usize {
        let step = ((SOLID - alpha) / Self::rung()).round();
        (step.max(0.0) as usize).min(rungs - 1)
    }

    /// The material nearest to `alpha`.
    pub fn fade(&self, alpha: f32) -> Handle<StandardMaterial> {
        self.fades[Self::rung_for(alpha, self.fades.len())].clone()
    }
}

/// Builds the shared disc and the ladder of materials. Called once, from
/// startup.
/// Returns the art rather than inserting it, so that `setup` can hand the solid
/// rung to [`crate::impostor`] -- which draws the far crowd's shadows itself,
/// as part of the one mesh that whole crowd is drawn by, and would otherwise
/// have to wait a frame for a resource that is still a queued command.
pub fn prepare(
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> ShadowArt {
    let texture = images.add(disc());
    let fades = ShadowArt::ladder()
        .map(|alpha| {
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, alpha),
                base_color_texture: Some(texture.clone()),
                alpha_mode: AlphaMode::Blend,
                // A shadow is a shadow, not a surface: nothing about it should
                // change with where the sun is.
                unlit: true,
                // Nor with how far away it is, which is a stronger claim than
                // it sounds and is where the ants' outline came from.
                //
                // Fog replaces a surface's colour with the sky's, which is
                // right for something opaque and wrong for something you can
                // see through: what a haze does to a shadow is make it fainter,
                // not paler. Left on, every disc past `AIR_FOG.0` stopped being
                // black and became a sky-coloured patch -- and because it is
                // blended, it did not fade *into* the distance, it painted sky
                // *over* whatever was under it. Under it, for the far crowd, is
                // the sprite: the disc lies through the enemy's feet and the
                // card stands through them, so the two planes cross, and the
                // half of the disc that wins the depth test washes the legs and
                // the bottom of the body in sky colour. That is the "odd blue
                // outline" on the ants, which is why it was blue, and why it
                // turned magenta the moment the sky did.
                fog_enabled: false,
                // Seen from below through a water sheet, or from under a floor
                // the caster is standing on the far side of.
                double_sided: true,
                cull_mode: None,
                ..default()
            })
        })
        .collect();
    ShadowArt {
        // A unit circle in the XY plane facing +Z, which is why [`project`]
        // turns it by the rotation that takes +Z onto the floor's normal.
        mesh: meshes.add(Circle::new(1.0).mesh().resolution(SEGMENTS).build()),
        fades,
    }
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

/// How far apart the two floor answers may be before the slope one is thrown
/// away. Two queries that land on the same surface agree to within rounding;
/// anything wider than this means they found different surfaces.
const SAME_SURFACE: f32 = 0.05;

/// Where the disc under a caster at `here` belongs: the height to put it at,
/// and the direction to lie along. `None` when there is nothing underneath at
/// all.
///
/// Two different questions, deliberately asked separately.
///
/// The *height* comes from the same query the physics uses, so the disc is at
/// the height the caster's own feet are resting at -- if something is holding
/// him up, that is what he is standing on and that is where his shadow goes.
///
/// The *slope* comes from [`LevelData::ground_at`], which only considers
/// surfaces flat enough to be ground. That matters because the collision grid's
/// floor list is barely filtered: over the castle, seven per cent of points get
/// their floor from a triangle leaning more than sixty degrees, and near a wall
/// the winning surface is often the wall itself. A disc turned onto a wall's
/// normal stands on its edge, which is to say it vanishes, and it flips in and
/// out as the caster walks past the point where the wall stops winning -- which
/// is exactly the flicker this pairing exists to remove.
///
/// When the two answers disagree they found different surfaces, and the slope
/// is not this floor's slope. The disc lies flat rather than taking a tilt from
/// something it is not resting on. Falling back rather than giving up is the
/// point: on the playable ground the two agree better than nine times in ten,
/// and the rest still get a shadow instead of a hole where one should be.
///
/// A planet takes the shorter road. Its ground query is a ray rather than a
/// column, so the surface it found and the surface whose slope it reports are
/// the same triangle by construction, and there is nothing for the two answers
/// to disagree about.
pub fn place(level: &LevelData, here: Vec3, up: Vec3) -> Option<(Vec3, Vec3)> {
    let (floor, normal) = level.ground_below(here, up)?;
    if level.shape() != Shape::Flat {
        return Some((floor, normal));
    }
    let slope = level
        .ground_at(here)
        .filter(|(ground, _)| (ground - floor.y).abs() <= SAME_SURFACE)
        .map_or(up, |(_, slope)| slope);
    Some((floor, slope))
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
/// A new disc starts on the solid rung of the ladder; [`project`] moves it to
/// whichever rung its height calls for before it is ever shown.
pub fn attach(
    mut commands: Commands,
    art: Res<ShadowArt>,
    new_casters: Query<Entity, (With<ShadowCaster>, Without<HasShadow>)>,
    casters: Query<(), With<ShadowCaster>>,
    shadows: Query<(Entity, &Shadow)>,
) {
    for owner in &new_casters {
        commands.spawn((
            Shadow { owner },
            Mesh3d(art.mesh.clone()),
            MeshMaterial3d(art.fade(SOLID)),
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
    gravity: Res<Gravity>,
    art: Res<ShadowArt>,
    tuning: Res<crate::console::GameTuning>,
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<crate::portal::PortalView>)>,
    casters: Query<(&Transform, &ShadowCaster, Option<&Visibility>), Without<Shadow>>,
    mut shadows: Query<(
        &Shadow,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    // No camera is not a reason to stop placing discs -- it is a reason to
    // place them where they lie. The float is a drawing correction and only
    // means anything once there is an eye to be drawn for.
    let eye = camera.single().ok().map(|view| view.translation());
    let drawn = tuning.shadows >= 0.5;
    for (shadow, mut transform, mut visibility, mut material) in &mut shadows {
        if !drawn {
            *visibility = Visibility::Hidden;
            continue;
        }
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
        let up = gravity.up(here);
        let Some((floor, slope)) = place(&level, here, up) else {
            // Over open space -- off the edge of the map, or above a hole.
            // There is nothing for a shadow to land on.
            *visibility = Visibility::Hidden;
            continue;
        };
        *visibility = Visibility::Visible;
        let drop = (here - floor).dot(up).max(0.0);
        // Lifted along the floor's normal rather than straight up, so the
        // clearance is the same all the way round the disc on a slope.
        let lying = floor + slope * LIFT;
        let radius = settings.radius * scale_with_drop(drop);
        // And then drawn nearer than it lies, so the hill it is laid on cannot
        // swallow it. Scaling about the eye is one factor on both the offset
        // and the size, which is what leaves the disc exactly where it was on
        // screen -- see [`FLOAT`].
        let near = eye.map_or(1.0, |eye| {
            float_toward(eye, lying, clearance(radius, settings.height))
        });
        transform.translation = eye.map_or(lying, |eye| eye + (lying - eye) * near);
        transform.rotation = Quat::from_rotation_arc(Vec3::Z, slope);
        transform.scale = Vec3::splat(radius * near);
        // Assigned only when the rung actually changes. Writing the same
        // handle back would mark the component changed, and a changed material
        // component puts the disc through the render world's specialise-and-
        // rebuild path -- which is most of what quantising the fade was for.
        let fade = art.fade(dim_with_drop(settings.solidity, drop));
        if material.0 != fade {
            *material = MeshMaterial3d(fade);
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

    /// A shadow is not a surface, and fogging it as if it were is where the far
    /// crowd's "blue outline" came from.
    ///
    /// Fog swaps a fragment's colour for the sky's. On something opaque that
    /// reads as distance; on something blended it reads as sky painted over
    /// whatever is behind, because the alpha is left alone and only the colour
    /// moves. Every disc past `AIR_FOG.0` stopped being a black smudge and
    /// became a pale sky-coloured one -- and the far crowd's discs lie through
    /// their enemies' feet while the sprite cards stand through them, so the
    /// two planes cross and the winning half of the disc washed the ants' legs
    /// in sky. Change the sky and the outline changed with it, which is how it
    /// was finally caught.
    #[test]
    fn a_shadow_is_never_fogged() {
        let mut meshes = Assets::<Mesh>::default();
        let mut images = Assets::<Image>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let art = prepare(&mut meshes, &mut images, &mut materials);
        assert!(!art.fades.is_empty(), "a ladder with no rungs on it");
        for rung in &art.fades {
            let material = materials.get(rung).expect("a rung with no material");
            assert!(
                !material.fog_enabled,
                "a shadow disc is being fogged, which turns it into a sky-coloured \
                 patch painted over whatever it lands on"
            );
        }
    }

    /// The float must never be able to reach the thing standing on the disc.
    ///
    /// Both halves of the rule earn their place. The player is tall and narrow,
    /// so his radius is what bounds him and his height never comes into it. The
    /// slime is a dome about as high as it is wide, and *only* its height
    /// bounds it -- taking the radius alone drew a black bar across its face
    /// from an ordinary playing camera.
    #[test]
    fn a_disc_is_never_floated_far_enough_to_cover_its_own_caster() {
        let slime = clearance(0.6, 0.6);
        assert_eq!(
            slime,
            0.6 * HEADROOM,
            "the dome should be bounded by height"
        );
        let player = clearance(crate::player::PLAYER_RADIUS, crate::player::PLAYER_HEIGHT);
        assert_eq!(
            player,
            crate::player::PLAYER_RADIUS * FLOAT,
            "someone tall and narrow should be bounded by radius"
        );
        for (radius, height) in [(0.6, 0.6), (0.42, 1.75), (0.84, 1.2), (3.0, 0.2)] {
            assert!(
                clearance(radius, height) < height,
                "a caster {height} tall gets a disc drawn {} in front of it",
                clearance(radius, height)
            );
        }
    }

    /// The float is a scale about the eye, so it has to survive the two cases
    /// where there is nothing to scale: a disc at the eye, and one whose
    /// clearance is deeper than it is far away.
    #[test]
    fn a_disc_is_never_turned_inside_out_through_the_camera() {
        let eye = Vec3::new(0.0, 5.0, 0.0);
        assert_eq!(float_toward(eye, eye, 0.5), 1.0, "no ray to slide along");
        assert_eq!(
            float_toward(eye, eye + Vec3::X * 0.25, 0.5),
            1.0,
            "a disc closer than its own clearance must be left where it is"
        );
        let near = float_toward(eye, eye + Vec3::X * 10.0, 1.0);
        assert!((near - 0.9).abs() < 1e-6, "{near}");
    }

    /// The bug this pairing was written for, pinned against the real castle.
    ///
    /// A disc is turned to lie along what it rests on, so the direction it is
    /// given had better be one a floor could actually have. The collision
    /// grid's floor list is filtered at `0.01` -- a triangle leaning
    /// eighty-nine degrees is in it -- and taking its normal unfiltered turned
    /// seven per cent of the castle's shadows onto their edge, where they are
    /// invisible, and flipped them in and out as a caster crossed the point
    /// where a wall stopped winning the query.
    #[test]
    fn a_shadow_is_never_turned_onto_a_wall() {
        let (level, _) = crate::level::load();
        let mut worst = 1.0_f32;
        let mut at = Vec3::ZERO;
        for (here, _) in walkable(&level) {
            let (_, up) = place(&level, here, Vec3::Y).expect("a floor was found and then lost");
            if up.y < worst {
                worst = up.y;
                at = here;
            }
        }
        assert!(
            worst >= crate::level::GROUND_NORMAL_Y,
            "a shadow at {at:?} is laid on a surface leaning to {worst:.3}, which is a \
             wall rather than a floor"
        );
    }

    /// Wherever a caster can stand, it gets a disc. Falling back to a flat one
    /// is what buys this: asking only for ground-like surfaces and giving up
    /// otherwise leaves holes over a good sixth of the map.
    #[test]
    fn anywhere_a_caster_can_stand_gets_a_shadow() {
        let (level, _) = crate::level::load();
        let mut standing = 0;
        for (here, _) in walkable(&level) {
            standing += 1;
            assert!(
                place(&level, here, Vec3::Y).is_some(),
                "a caster standing at {here:?} casts no shadow at all"
            );
        }
        assert!(
            standing > 10_000,
            "the sweep only found {standing} places to stand"
        );
    }

    /// Every place on the castle a caster could be standing, as the position
    /// its feet would be at. Coarse on purpose: this is a sweep for surfaces
    /// the two floor queries disagree about, not a test of the collision.
    fn walkable(level: &crate::level::LevelData) -> Vec<(Vec3, f32)> {
        let mut found = Vec::new();
        for gz in -80..=80 {
            for gx in -80..=80 {
                let (x, z) = (gx as f32, gz as f32);
                if let Some(floor) = level.floor_height(Vec3::new(x, 20.0, z)) {
                    found.push((Vec3::new(x, floor, z), floor));
                }
            }
        }
        found
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

    /// The quantised fade has to be indistinguishable from the real one, or the
    /// saving bought a visible artefact. Checked across the whole band
    /// `dim_with_drop` produces, at a much finer step than the ladder itself.
    #[test]
    fn the_ladder_reproduces_the_fade_it_replaces() {
        let ladder: Vec<f32> = ShadowArt::ladder().collect();
        let tolerance = ShadowArt::rung() / 2.0 + 1e-6;
        let mut worst = 0.0_f32;
        for step in 0..=1000 {
            let drop = FADE_DISTANCE * 1.5 * step as f32 / 1000.0;
            let exact = dim_with_drop(SOLID, drop);
            let drawn = ladder[ShadowArt::rung_for(exact, ladder.len())];
            worst = worst.max((drawn - exact).abs());
        }
        assert!(
            worst <= tolerance,
            "the ladder is off the true fade by {worst}, more than the half rung \
             of {tolerance} that rounding to the nearest allows"
        );
        // In the units the original works in, so the number means something.
        assert!(
            worst * 255.0 < 4.0,
            "{:.1} of 255 alpha levels is a step you could see",
            worst * 255.0
        );
    }

    /// Both ends of the band land exactly on a rung: a shadow on the ground is
    /// as dark as `shadow.c` makes it, not a rounding error away from it.
    #[test]
    fn the_band_the_fade_uses_lands_on_rungs() {
        let ladder: Vec<f32> = ShadowArt::ladder().collect();
        for exact in [SOLID, FAR_SOLIDITY] {
            let drawn = ladder[ShadowArt::rung_for(exact, ladder.len())];
            assert!((drawn - exact).abs() < 1e-6, "{exact} was drawn as {drawn}");
        }
        assert_eq!(ladder[0], SOLID, "the first rung should be a solid shadow");
    }

    /// The point of the ladder: two casters at the same height pick the same
    /// rung, so their discs share a material and batch into one draw instead of
    /// each rebuilding a bind group of its own every frame.
    #[test]
    fn casters_at_the_same_height_share_a_rung() {
        let rungs = ShadowArt::ladder().count();
        let rung_at = |drop| ShadowArt::rung_for(dim_with_drop(SOLID, drop), rungs);
        assert_eq!(rung_at(0.0), rung_at(0.0));
        assert_eq!(
            rung_at(0.0),
            rung_at(FADE_DISTANCE / 400.0),
            "heights a rung apart should still share a material"
        );
        assert_ne!(
            rung_at(0.0),
            rung_at(FADE_DISTANCE),
            "a shadow at full height should not be drawn as a solid one"
        );
        // Every caster off the ground far enough is on one rung together, which
        // is where the batching actually pays.
        assert_eq!(rung_at(FADE_DISTANCE), rung_at(FADE_DISTANCE * 10.0));
    }

    /// A caster fainter than this port's `SOLID` keeps its own darkness rather
    /// than being clamped into the band the fade happens to use.
    #[test]
    fn a_faint_caster_is_not_dragged_up_to_the_bands_floor() {
        let ladder: Vec<f32> = ShadowArt::ladder().collect();
        let faint = FAR_SOLIDITY / 2.0;
        let drawn = ladder[ShadowArt::rung_for(faint, ladder.len())];
        assert!(
            (drawn - faint).abs() <= ShadowArt::rung() / 2.0 + 1e-6,
            "a caster asking for {faint} was drawn at {drawn}"
        );
    }
}
