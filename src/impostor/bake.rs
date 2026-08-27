//! Baking the impostor sheets: rendering an actor from every angle, once.
//!
//! Run it with
//!
//! ```text
//! cargo run --release -- bake-impostors [slime|ant]
//! ```
//!
//! and it writes `assets/impostors/<kind>.png` and `<kind>.json`.
//!
//! This is a rewrite of a tool that no longer exists. The sheets in the
//! repository were baked by a Panda3D script that went with the Panda3D build,
//! which left two atlases nobody could regenerate and a sidecar in a format
//! whose meaning had to be guessed at. So the format here is defined by this
//! file rather than reverse-engineered from those, and the numbers in it are
//! ones the baker knows rather than ones it measured.
//!
//! It runs inside the game rather than beside it, and that is the point: the
//! actor is loaded by the same glTF loader, converted onto the same
//! [`crate::n64::N64Material`], and lit by the same [`crate::n64::N64Lighting`]
//! as the skinned model it stands in for. A sprite baked by a separate tool
//! with its own idea of lighting is a sprite that visibly changes colour at the
//! swap distance.
//!
//! # How the geometry is pinned down
//!
//! The hard part of an impostor sheet is not drawing it, it is knowing what a
//! cell *means* afterwards -- how big a quad it belongs on, and where in it the
//! ground is. Measuring that from the pixels is guesswork. Instead the camera is
//! orthographic, so a cell covers an exactly known number of world units:
//!
//!   * `world_size` is the camera's vertical extent. That is the quad's size.
//!   * the camera is aimed at `focus` above the model's origin, so the origin
//!     sits `0.5 - focus * cos(elevation) / world_size` of the way up the cell.
//!     That is `foot`; the cosine is there because a camera tilted down
//!     foreshortens world height, and at fifty-five degrees it is the
//!     difference between a sprite standing on the floor and one buried in it.
//!
//! Both fall out of the camera rather than out of the image, which is why the
//! runtime can trust them.
//!
//! # Tiers
//!
//! The sheet is baked from more than one height -- see [`ELEVATIONS`] -- and
//! each is a whole block of `ANGLES` rows, stacked flattest first. That is the
//! only thing about the layout the runtime has to be told, and it is told it in
//! the sidecar rather than here.
//!
//! Everything else is shared between the tiers on purpose. One survey covers
//! all of them, one extent is fitted to the largest thing any of them saw, and
//! the tiers differ only in where the camera stood and where the origin landed
//! in the picture. A per-tier extent would be tighter and would make every
//! sprite in the field change size the moment the camera crossed a tier
//! boundary, which is much more visible than the margin it would save.
//!
//! The one thing that *is* measured is how big the camera has to be. A first
//! pass renders the whole sheet at a deliberately generous extent and records
//! the alpha bounding box; a second pass re-renders it at an extent tightened
//! onto what the first pass found. That costs twice the time -- a few seconds --
//! and spends the cell's pixels on the actor instead of on empty margin.

use crate::{
    enemy::Kind,
    impostor::{SheetMeta, Tier},
    n64::{self, N64Lighting},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::{visibility::RenderLayers, ImageRenderTarget, RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
    },
    window::ExitCondition,
};

/// Pixels along one side of a cell.
///
/// 128 is what the retired tool used and is kept. It is about four times the
/// height an impostor is ever drawn at -- past the swap distance an enemy is a
/// dozen pixels tall -- and the headroom is what keeps the sprite from turning
/// to mush when somebody raises the swap distance.
const CELL_PX: u32 = 128;

/// How many samples across a cell's texel the final render is taken at.
///
/// The camera below bakes with [`Msaa::Off`], so a cell rendered at
/// [`CELL_PX`] rasterises every feature thinner than a texel as a dotted line:
/// an ant's legs and antennae arrive in the sheet as broken dashes, and every
/// gap between the dashes shows whatever is behind the sprite. Against the sky
/// that is a pale blue stipple running the length of each leg -- the artefact
/// that reads, in a crowd, as a blue outline round every distant enemy. It is
/// baked in, so no amount of care at draw time removes it and it does not go
/// away with distance.
///
/// Rendering at four times the width and reducing by boxes of sixteen is
/// sixteen-sample supersampling, which is what MSAA would have given -- except
/// that the reduction in [`blit`] ends in a threshold, so the sheet stays the
/// cutout the rest of the pipeline needs it to be. See [`COVERAGE`].
const SUPERSAMPLE: u32 = 4;

/// Pixels along one side of the target one cell is rendered into.
const SAMPLE_PX: u32 = CELL_PX * SUPERSAMPLE;

/// How much of a texel has to be covered for that texel to be in the sprite.
///
/// A quarter rather than a half, and the whole point is the thin features. A
/// leg half a texel wide covers a quarter of the texels it crosses, so a half
/// threshold erases it just as thoroughly as the aliasing did -- and erases it
/// everywhere, which is worse than dashes. A quarter keeps it, at the cost of
/// growing the silhouette by rather less than a texel: the edge texels of a
/// solid body average half coverage and were going to be kept either way.
///
/// It has to be a threshold rather than the coverage itself. `n64.wgsl` tests
/// alpha against a cutoff and `n64::cutout` promotes a material to
/// `AlphaMode::Mask` on the strength of the sheet's alpha being binary; a sheet
/// of soft edges would be drawn in the transparent pass, sorted by quad origin,
/// and would stipple against itself in a crowd.
const COVERAGE: f32 = 0.25;

/// Viewing angles round the model, and frames of its walk.
const ANGLES: usize = 16;
const FRAMES: usize = 16;

/// How far the camera is tilted down towards the model, in degrees, once per
/// tier of the sheet. Flattest first, which is the order the rows go in.
///
/// Fifteen is where the follow camera sits above a distant enemy on the flat:
/// it is above the player and looks slightly down, so a sheet baked dead level
/// would show an enemy's face where the game shows the top of its head.
///
/// Fifty-five is for the case the flat tier cannot cover at all. The camera's
/// own pitch reaches forty-three degrees down (`camera::PITCH_LIMITS`), and a
/// player standing on anything -- a wall, a stair, the lip of a pit -- adds the
/// drop to the crowd below on top of that, so looking down on the far field
/// from sixty or seventy degrees is ordinary rather than exotic. Sprites are
/// picked by nearest elevation, so these two put the hand-over at thirty-five
/// degrees and leave nothing more than twenty degrees from a baked picture.
///
/// Two rather than three, and the cost is the reason: a tier is `ANGLES` more
/// rows of atlas, so it doubles the sheet -- 2048x4096 rather than 2048x2048,
/// 32 MB of texture a kind. That is a real bill on the machine this is aimed
/// at, and the third tier would be buying a ten-degree improvement in the
/// middle of a range that is already never worse than twenty.
const ELEVATIONS: [f32; 2] = [15.0, 55.0];

/// How many world units across the first pass looks, and how much room the
/// second pass leaves around what the first one measured.
///
/// The first is generous because a walk cycle swings limbs well outside the
/// bind pose and anything clipped in pass one is measured wrong for pass two.
/// It is a guess at how big an actor gets rather than a fact about one, so an
/// actor bigger than it does not fail the bake -- see [`survey_fits`], which
/// notices and looks again from further back. The second is the margin that
/// survives into the sheet.
///
/// It was 2.6 when the sheet was one flat tier, and the steep one is what
/// raised it: seen from fifty-five degrees up, an actor's *length* is in the
/// picture's vertical as well as its horizontal, so the slime filled a camera
/// from above that it had rattled around in from the side. That is not a
/// failure -- the survey widened and baked correctly -- but a widening doubles
/// the bake and measures the actor at half the resolution it could have, and
/// paying that on every bake for ever is worse than starting from a number
/// that fits.
const SURVEY_MARGIN: f32 = 3.6;
const FINAL_MARGIN: f32 = 1.06;

/// How much room a *seeded* survey leaves round the size the last bake fitted.
///
/// [`SURVEY_MARGIN`] is only the cold-start number now: where a sheet already
/// exists, its sidecar records the extent the previous bake fitted onto this
/// actor and the height it aimed at, and the survey starts from those instead.
/// Forty per cent of slack in every direction is far more than an edit to a
/// model moves it, and an edit that does move it further is not a failure --
/// the widening below still catches it, exactly as it does from a cold start.
///
/// The ant is why this exists. It fits in six units, `SURVEY_MARGIN` is 3.6,
/// so every single bake of it surveyed, found the actor filling the camera,
/// threw the whole pass away and surveyed again from 7.2 -- a wasted third of
/// the bake, paid every time, for a number the sheet beside it already knew.
const SURVEY_SEED: f32 = 1.4;

/// How many times the survey may double its camera before giving up.
///
/// Bounded because the alternative to a wrong sheet is a bake that never
/// finishes, and four doublings is a fortyfold range of actor sizes: anything
/// outside that is a model with a scale problem rather than a big model.
const WIDENINGS: usize = 4;

/// Frames rendered and thrown away after moving the model before the pixels are
/// asked for.
///
/// Not superstition. A pose set this frame is written to the joints by
/// `animate_targets`, propagated by the transform systems, and only then
/// extracted into the render world -- and the readback returns whatever the GPU
/// last finished, which is a frame or two behind the main world. Reading too
/// early gives the *previous* cell's picture, and the failure looks like a
/// sheet that is correct but rotated by one cell, which is a miserable thing to
/// debug. Four is comfortably more than the pipeline is deep.
const SETTLE: usize = 4;

/// Which pass the bake is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pass {
    /// Rendering at [`SURVEY_MARGIN`] to find out how big the actor really gets.
    Survey,
    /// Rendering the sheet that gets written.
    Final,
}

/// Everything one bake needs to remember between frames.
#[derive(Resource)]
struct Bake {
    kind: Kind,
    pass: Pass,
    /// Which cell is being rendered: an index into `angles * frames`.
    cell: usize,
    /// Frames left to settle before the pixels are worth asking for.
    settle: usize,
    /// True once the readback for this cell has been asked for, so it is not
    /// asked for again on the next frame while the first is still in flight.
    reading: bool,
    /// The camera's vertical extent in world units, this pass. One for the
    /// whole sheet, tiers included.
    extent: f32,
    /// How far above the model's origin the camera is aimed, per tier.
    ///
    /// Per tier because a tilted camera aimed at a fixed height puts the model
    /// somewhere else in the frame: the survey aims all of them at the same
    /// place and [`tighten`] centres each on what that tier actually saw.
    focus: Vec<f32>,
    /// The tightest box, in cell fractions, that held content in the survey,
    /// per tier. `(low, high)`, y measured downward like the image.
    seen: Vec<Option<(Vec2, Vec2)>>,
    /// How many times the survey has already been re-run from further back.
    widened: usize,
    /// The finished sheet, `cols * cell_px` by `rows * cell_px`, RGBA8.
    atlas: Vec<u8>,
    /// The clip being sampled, and how long it is.
    clip: Option<(AnimationNodeIndex, f32)>,
    started: std::time::Instant,
}

impl Bake {
    fn cells(&self) -> usize {
        ELEVATIONS.len() * ANGLES * FRAMES
    }

    /// The tier, angle and frame indices of the cell being rendered. Frames
    /// advance fastest and tiers slowest, so a row of the atlas is one angle's
    /// whole walk cycle and a block of `ANGLES` rows is one tier -- which is
    /// the layout [`SheetMeta::grid`] promises.
    fn indices(&self) -> (usize, usize, usize) {
        let tier = self.cell / (ANGLES * FRAMES);
        (tier, (self.cell / FRAMES) % ANGLES, self.cell % FRAMES)
    }
}

/// The offscreen target one cell is drawn into.
fn cell_target(images: &mut Assets<Image>) -> Handle<Image> {
    let mut image = Image::new_fill(
        Extent3d {
            width: SAMPLE_PX,
            height: SAMPLE_PX,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        // Not sRGB: the readback is written straight into a PNG, and the
        // game reads that PNG back as sRGB. Asking the GPU to linearise on the
        // way out and then treating the result as sRGB on the way in is how a
        // sheet comes out visibly paler than the model it was baked from.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC
        | TextureUsages::RENDER_ATTACHMENT;
    images.add(image)
}

/// The actor being baked, so the systems below can find and pose it.
#[derive(Component)]
struct Subject;

/// The camera, so its projection can be retuned between passes.
#[derive(Component)]
struct BakeCamera;

fn model(kind: Kind) -> &'static str {
    kind.model()
}

/// Sets the bake up: the target, the camera, and the actor.
fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    bake: Res<Bake>,
) {
    let target = cell_target(&mut images);
    commands.insert_resource(CellImage(target.clone()));
    commands.spawn((
        BakeCamera,
        Camera3d::default(),
        Camera {
            // Transparent, so the alpha channel that reaches the PNG is the
            // actor's silhouette rather than a box of sky.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        // The target is its own component in this version rather than a field
        // on the camera, the same way `display::world_camera_target` gives it.
        RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            scale_factor: 1.0,
        }),
        // Orthographic on purpose: a cell then covers an exactly known number of
        // world units, which is what lets `world_size` and `foot` be known
        // rather than measured. A perspective camera would make both a function
        // of the distance the model happened to be at.
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: bake.extent,
                height: bake.extent,
            },
            ..OrthographicProjection::default_3d()
        }),
        // MSAA would feather the silhouette into the transparent background and
        // leave a halo of half-alpha pixels round every sprite in the game.
        Msaa::Off,
        Tonemapping::None,
        camera_pose(bake.extent, bake.focus[0], 0.0, ELEVATIONS[0]),
        RenderLayers::layer(0),
    ));
    commands.spawn((
        Subject,
        // The same marker the game spawns these actors with. Without it
        // `billboard::two_sided` never makes the quads double-sided, and a
        // billboard seen from its back face is culled away entirely.
        crate::billboard::BillboardActor,
        // Deliberately *not* `n64::Translucent`, which the game does spawn
        // these with. A sheet is drawn with `AlphaMode::Mask`, and the whole
        // arrangement rests on its alpha being nothing or everything: baking a
        // see-through body would fill the sheet with middling alpha, which the
        // cutoff rounds back to solid and the nearest sampler stipples at every
        // edge -- `the_baked_sheets_are_cutouts_rather_than_soft_edged` fails on exactly that. So the
        // far crowd is opaque, and pays for it at a range where an actor is a
        // few pixels tall.
        crate::WorldAssetRoot(assets.load(format!("{}#Scene0", model(bake.kind)))),
        // Unscaled, exactly as the game spawns one, so the world units this
        // camera measures are the game's world units. A sheet baked at any other
        // size is a sprite that changes size at the swap distance.
        Transform::default(),
    ));
}

/// The render target, kept so the readback can be asked for on it.
#[derive(Resource)]
struct CellImage(Handle<Image>);

/// Where the camera sits to see a model of `extent` turned to `yaw`, from
/// `elevation` degrees above it.
///
/// The camera orbits the model rather than the model turning under a fixed
/// camera. Both would do, and this way round the actor's own transform stays
/// the identity, so nothing has to reason about whether a rotation applied
/// before or after the animation.
fn camera_pose(extent: f32, focus: f32, yaw: f32, elevation: f32) -> Transform {
    let tilt = elevation.to_radians();
    // Far enough back that the near plane is never inside the model. With an
    // orthographic projection the distance changes nothing about the picture,
    // so it can simply be generous.
    let away = extent * 4.0;
    let at = Vec3::new(0.0, focus, 0.0);
    let eye = at + Quat::from_rotation_y(yaw) * (Vec3::new(0.0, tilt.sin(), tilt.cos()) * away);
    Transform::from_translation(eye).looking_at(at, Vec3::Y)
}

/// Gives the actor's animation player the clip, once its scene has arrived.
fn claim(
    mut bake: ResMut<Bake>,
    clips: Res<Assets<AnimationClip>>,
    assets: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
    players: Query<Entity, (With<AnimationPlayer>, Without<AnimationGraphHandle>)>,
) {
    if bake.clip.is_some() {
        return;
    }
    let Ok(player) = players.single() else {
        return;
    };
    let handle: Handle<AnimationClip> = assets.load(format!(
        "{}#Animation{}",
        model(bake.kind),
        bake.kind.clip()
    ));
    let Some(clip) = clips.get(&handle) else {
        return;
    };
    let duration = clip.duration();
    let (graph, node) = AnimationGraph::from_clip(handle);
    commands
        .entity(player)
        .insert(AnimationGraphHandle(graphs.add(graph)));
    bake.clip = Some((node, duration));
}

/// Poses the actor for the current cell and asks for its pixels when it has
/// settled.
#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut bake: ResMut<Bake>,
    cell: Res<CellImage>,
    mut players: Query<&mut AnimationPlayer>,
    mut camera: Query<(Entity, &mut Transform), With<BakeCamera>>,
    subject: Query<Entity, With<Subject>>,
) {
    let Some((node, duration)) = bake.clip else {
        return;
    };
    // Nothing to pose until the scene has actually spawned something.
    if subject.iter().next().is_none() {
        return;
    }
    let Ok((camera_entity, mut view)) = camera.single_mut() else {
        return;
    };
    if bake.reading {
        return;
    }
    // The last cell of the final pass is collected, written and the app told to
    // exit -- but `done` runs after this, so there is one more turn through
    // here with `cell` one past the end. Reading a tier out of that is an index
    // panic on the frame *after* a bake that worked, which reads as a bake that
    // failed.
    if bake.cell >= bake.cells() {
        return;
    }
    let (tier, angle, frame) = bake.indices();
    let yaw = angle as f32 / ANGLES as f32 * std::f32::consts::TAU;
    *view = camera_pose(bake.extent, bake.focus[tier], yaw, ELEVATIONS[tier]);
    // Sampled at the middle of each frame's slice rather than at its start, so
    // a sixteen-frame sheet of a looping clip does not show the first pose
    // twice -- once at the beginning and once at the end.
    let at = duration * (frame as f32 + 0.5) / FRAMES as f32;
    for mut player in &mut players {
        // Paused and seeked rather than played: the clock here is the cell
        // index, and a clip advancing on its own would put a different pose in
        // the picture than the one that was asked for.
        player.play(node).seek_to(at).pause();
    }
    if bake.settle > 0 {
        bake.settle -= 1;
        return;
    }
    commands
        .entity(camera_entity)
        .insert(Readback::texture(cell.0.clone()));
    bake.reading = true;
}

/// Takes one cell's pixels and moves on.
fn collect(
    trigger: On<ReadbackComplete>,
    mut commands: Commands,
    mut bake: ResMut<Bake>,
    mut images: ResMut<Assets<Image>>,
    mut projection: Query<&mut Projection, With<BakeCamera>>,
) {
    if !bake.reading {
        return;
    }
    let pixels = trigger.event().data.clone();
    commands.entity(trigger.event().entity).remove::<Readback>();
    bake.reading = false;
    let expected = (SAMPLE_PX * SAMPLE_PX * 4) as usize;
    if pixels.len() < expected {
        eprintln!(
            "impostor bake: short readback ({} of {expected} bytes), retrying",
            pixels.len()
        );
        bake.settle = SETTLE;
        return;
    }
    match bake.pass {
        Pass::Survey => note_bounds(&mut bake, &pixels),
        Pass::Final => blit(&mut bake, &pixels),
    }
    bake.cell += 1;
    bake.settle = SETTLE;
    if bake.cell < bake.cells() {
        return;
    }
    // End of a pass.
    match bake.pass {
        Pass::Survey => {
            if survey_fits(&bake) {
                tighten(&mut bake);
                bake.pass = Pass::Final;
            } else if bake.widened < WIDENINGS {
                bake.widened += 1;
                println!(
                    "impostor bake: the actor filled the survey camera at {:.3} units; \
                     looking again from {:.3}",
                    bake.extent,
                    bake.extent * 2.0,
                );
                bake.extent *= 2.0;
                bake.focus = vec![bake.extent * 0.25; ELEVATIONS.len()];
                bake.seen = vec![None; ELEVATIONS.len()];
            } else {
                eprintln!(
                    "impostor bake: {:?} still fills a {:.3} unit camera after {WIDENINGS} \
                     widenings -- baking it cropped, which is a Kind::draw_scale to check",
                    bake.kind, bake.extent,
                );
                tighten(&mut bake);
                bake.pass = Pass::Final;
            }
            bake.cell = 0;
            if let Ok(mut projection) = projection.single_mut() {
                *projection = Projection::from(OrthographicProjection {
                    scaling_mode: ScalingMode::Fixed {
                        width: bake.extent,
                        height: bake.extent,
                    },
                    ..OrthographicProjection::default_3d()
                });
            }
            let _ = &mut images;
        }
        Pass::Final => finish(&bake),
    }
}

/// Widens the survey's running bounding box by whatever this cell contained.
fn note_bounds(bake: &mut Bake, pixels: &[u8]) {
    let mut low = Vec2::splat(f32::MAX);
    let mut high = Vec2::splat(f32::MIN);
    for y in 0..SAMPLE_PX {
        for x in 0..SAMPLE_PX {
            // Anything more than barely transparent counts. A threshold of zero
            // would let a single stray dithered pixel decide the sheet's size.
            if pixels[((y * SAMPLE_PX + x) * 4 + 3) as usize] > 8 {
                low = low.min(Vec2::new(x as f32, y as f32));
                high = high.max(Vec2::new(x as f32 + 1.0, y as f32 + 1.0));
            }
        }
    }
    if low.x > high.x {
        return;
    }
    let scale = 1.0 / SAMPLE_PX as f32;
    let (low, high) = (low * scale, high * scale);
    // Into this tier's box and no other. The tiers are measured apart because
    // they are aimed apart: what the final pass takes from all of them together
    // is the one extent, and what it takes from each alone is where that tier's
    // picture of the actor sat in the frame.
    let tier = bake.indices().0;
    bake.seen[tier] = Some(match bake.seen[tier] {
        Some((old_low, old_high)) => (old_low.min(low), old_high.max(high)),
        None => (low, high),
    });
}

/// Whether the survey saw the whole actor, or ran out of cell.
///
/// A survey camera too small for its model does not fail, it saturates: every
/// cell comes back full, [`tighten`] reads the crop as the measurement, and the
/// sheet is written at the survey's own extent with the actor cut off at
/// whichever angle it is longest. `world_size` is then a lie about what the cell
/// contains, and since the runtime sizes its quads from `world_size` the sprites
/// are wrong in the game as well as in the sheet.
///
/// That is not hypothetical. A re-exported ant arrived four times the size of
/// the one it replaced, filled the 2.6-unit survey at every angle, and baked a
/// sheet of ants missing their heads and abdomens. Both halves of the failure
/// are worth keeping in mind: the *cause* was a stale [`Kind::draw_scale`], and
/// this only makes the baker say so instead of quietly writing the crop.
fn survey_fits(bake: &Bake) -> bool {
    let edge = 1.0 / CELL_PX as f32;
    // Every tier, because they see different silhouettes: a long actor fits a
    // camera easily from the side and fills the same camera from above, and a
    // sheet is only as good as its worst tier.
    bake.seen.iter().flatten().all(|(low, high)| {
        low.x > edge && low.y > edge && high.x < 1.0 - edge && high.y < 1.0 - edge
    })
}

/// Chooses the extent and focus the final pass renders at, from what the survey
/// saw.
///
/// Square, because a cell is square and the quad drawn from it is square: the
/// extent has to cover the wider of the two axes or the sheet clips the actor
/// in the other one.
fn tighten(bake: &mut Bake) {
    if bake.seen.iter().all(Option::is_none) {
        eprintln!("impostor bake: the survey saw nothing at all; keeping the wide camera");
        return;
    }
    let survey = bake.extent;
    // The extent is the largest thing *any* tier saw, so that one camera fits
    // them all and a sprite does not change size when its tier changes.
    let mut wanted: f32 = 0.0;
    for (tier, elevation) in ELEVATIONS.iter().enumerate() {
        let Some((low, high)) = bake.seen[tier] else {
            eprintln!("impostor bake: the survey saw nothing at {elevation} degrees");
            continue;
        };
        let span = high - low;
        // Back into world units: the survey cell covered `survey` units across.
        wanted = wanted.max(span.max_element() * survey * FINAL_MARGIN);
        // Where this tier's content sat, relative to the camera's aim point.
        // The image's y runs downward and the world's runs up, hence the
        // negation -- and the cosine puts an offset measured in the picture
        // back into the world height the camera has to be raised by, which a
        // camera tilted `elevation` degrees down foreshortens on the way in.
        let middle = (low + high) * 0.5 - Vec2::splat(0.5);
        bake.focus[tier] += -middle.y * survey / elevation.to_radians().cos();
        println!(
            "impostor bake: at {elevation:.0} degrees the survey found {:.3} x {:.3} world \
             units, aiming {:.3} up",
            span.x * survey,
            span.y * survey,
            bake.focus[tier],
        );
    }
    bake.extent = wanted;
    println!("impostor bake: drawing every tier at {:.3} units", bake.extent);
}

/// Copies one cell's pixels into the atlas at its place.
fn blit(bake: &mut Bake, pixels: &[u8]) {
    let (tier, angle, frame) = bake.indices();
    let stride = FRAMES as u32 * CELL_PX * 4;
    let row = (tier * ANGLES + angle) as u32;
    for y in 0..CELL_PX {
        let to = ((row * CELL_PX + y) * stride + frame as u32 * CELL_PX * 4) as usize;
        for x in 0..CELL_PX {
            let texel = reduce(pixels, x, y);
            let at = to + (x * 4) as usize;
            bake.atlas[at..at + 4].copy_from_slice(&texel);
        }
    }
}

/// One texel of the sheet, from the box of samples that covers it.
///
/// Two averages and a threshold. The alpha average is the coverage -- what
/// fraction of the texel the model was actually over -- and it decides whether
/// the texel is in the sprite at all, against [`COVERAGE`]. The colour average
/// is weighted by that same alpha, so the transparent samples contribute
/// nothing to it: an unweighted average would drag every edge texel towards the
/// target's clear colour and ring the whole sprite in a dark halo, which is the
/// exact fault the bake camera's [`Msaa::Off`] was there to avoid.
fn reduce(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let mut colour = [0.0_f32; 3];
    let mut alpha = 0.0_f32;
    for row in 0..SUPERSAMPLE {
        for column in 0..SUPERSAMPLE {
            let at = (((y * SUPERSAMPLE + row) * SAMPLE_PX + x * SUPERSAMPLE + column) * 4) as usize;
            let weight = pixels[at + 3] as f32;
            for channel in 0..3 {
                colour[channel] += pixels[at + channel] as f32 * weight;
            }
            alpha += weight;
        }
    }
    let samples = (SUPERSAMPLE * SUPERSAMPLE) as f32;
    if alpha < COVERAGE * samples * 255.0 {
        return [0, 0, 0, 0];
    }
    let out = colour.map(|channel| (channel / alpha).round().clamp(0.0, 255.0) as u8);
    [out[0], out[1], out[2], 255]
}

/// Writes the atlas and its sidecar, and stops the app.
fn finish(bake: &Bake) {
    let root = crate::asset_path().join(super::SHEETS);
    let stem = super::stem(bake.kind);
    let width = FRAMES as u32 * CELL_PX;
    let height = (ELEVATIONS.len() * ANGLES) as u32 * CELL_PX;
    let png = root.join(format!("{stem}.png"));
    if let Err(error) = write_png(&png, width, height, &bake.atlas) {
        eprintln!("impostor bake: could not write {}: {error}", png.display());
        return;
    }
    let meta = SheetMeta {
        model: stem.to_string(),
        cell_px: CELL_PX,
        angles: ANGLES,
        frames: FRAMES,
        world_size: bake.extent,
        // One walk cycle across the columns, at the clip's own length.
        fps: bake.clip.map_or(FRAMES as f32, |(_, duration)| {
            FRAMES as f32 / duration.max(0.001)
        }),
        tiers: ELEVATIONS
            .iter()
            .enumerate()
            .map(|(tier, elevation)| Tier {
                elevation: *elevation,
                // The origin's height in the cell, from the bottom. The camera
                // is aimed `focus` above it and covers `extent`, and the tilt
                // foreshortens that gap by its cosine on the way into the
                // picture -- so it sits exactly this far up.
                foot: 0.5 - bake.focus[tier] * elevation.to_radians().cos() / bake.extent,
            })
            .collect(),
    };
    let json = root.join(format!("{stem}.json"));
    match serde_json::to_string_pretty(&meta) {
        Ok(text) => {
            if let Err(error) = std::fs::write(&json, text + "\n") {
                eprintln!("impostor bake: could not write {}: {error}", json.display());
                return;
            }
        }
        Err(error) => {
            eprintln!("impostor bake: could not encode the sidecar: {error}");
            return;
        }
    }
    println!(
        "impostor bake: wrote {} ({width}x{height}) and {} in {:.1}s\n  {meta:#?}",
        png.display(),
        json.display(),
        bake.started.elapsed().as_secs_f32(),
    );
}

/// Writes an RGBA8 buffer as a PNG.
fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")))?;
    image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "{} bytes is not a {width}x{height} RGBA image",
                rgba.len()
            ))
        })?
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(std::io::Error::other)
}

/// Ends the app once the final pass has been written.
fn done(bake: Res<Bake>, mut exit: MessageWriter<AppExit>) {
    if bake.pass == Pass::Final && bake.cell >= bake.cells() {
        exit.write(AppExit::Success);
    }
}

/// Where to stand the survey camera before it has seen anything.
///
/// Returns the extent to survey at and the height to aim each tier at, taken
/// from the sheet already on disk where there is one: `world_size` is the
/// extent the last bake fitted, and `foot` inverts back to the height it
/// aimed at, since `finish` derives one from the other by
/// `foot = 0.5 - focus * cos(elevation) / extent`.
///
/// Read back rather than written down because it is a measurement of the
/// *model*, and the model is a file somebody edits. A constant here would be a
/// number to keep in step with `assets/actors/`, which is precisely the thing
/// the survey exists to avoid having.
///
/// Falls back to [`SURVEY_MARGIN`] whenever the sidecar cannot be trusted to
/// mean what this needs it to: no sheet yet, a sheet from a bake with a
/// different set of [`ELEVATIONS`], or a `world_size` that is not a size. The
/// fallback is a slower bake and never a wrong one -- and a seed that turns out
/// too tight is caught by [`survey_fits`] and widened, the same as a cold start
/// that is too tight.
fn seed(kind: Kind) -> (f32, Vec<f32>) {
    let cold = (SURVEY_MARGIN, vec![SURVEY_MARGIN * 0.25; ELEVATIONS.len()]);
    let Ok(meta) = super::read_meta(&crate::asset_path(), kind) else {
        return cold;
    };
    if !meta.world_size.is_finite()
        || meta.world_size <= 0.0
        || meta.tiers.len() != ELEVATIONS.len()
        || !meta
            .tiers
            .iter()
            .zip(ELEVATIONS)
            .all(|(tier, elevation)| (tier.elevation - elevation).abs() < 0.5)
    {
        return cold;
    }
    let focus = meta
        .tiers
        .iter()
        .map(|tier| {
            (0.5 - tier.foot) * meta.world_size / tier.elevation.to_radians().cos()
        })
        .collect();
    let extent = meta.world_size * SURVEY_SEED;
    println!(
        "impostor bake: the sheet on disk was fitted at {:.3} units; surveying from {extent:.3}",
        meta.world_size,
    );
    (extent, focus)
}

/// Runs a bake for each named kind and returns when they are written.
pub fn run(kinds: &[Kind]) {
    for &kind in kinds {
        println!("impostor bake: {kind:?} -- surveying");
        let (extent, focus) = seed(kind);
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .build()
                .disable::<bevy::winit::WinitPlugin>()
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: crate::asset_path().to_string_lossy().into_owned(),
                    ..default()
                }),
        )
        .add_plugins(n64::N64Plugin)
        .init_resource::<N64Lighting>()
        .insert_resource(Bake {
            kind,
            pass: Pass::Survey,
            cell: 0,
            settle: SETTLE,
            reading: false,
            extent,
            // Cold, a quarter of the way up rather than half, so the survey
            // sees well *below* the model's origin as well as above it. Aiming
            // at the middle puts the cell's bottom edge exactly on y = 0, which
            // clips whatever hangs under the origin -- and since the survey is
            // what the final pass is sized from, a bottom edge measured wrong
            // there is a sheet with its actor's feet cut off. Warm, it is
            // whatever the last bake aimed at; see `seed`.
            focus,
            seen: vec![None; ELEVATIONS.len()],
            widened: 0,
            atlas: vec![
                0;
                FRAMES * CELL_PX as usize * ELEVATIONS.len() * ANGLES * CELL_PX as usize * 4
            ],
            clip: None,
            started: std::time::Instant::now(),
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (claim, step, done).chain())
        // Literally the game's own draw chain, not a copy of it. A sheet baked
        // through anything less is a sheet of something the game never draws --
        // see [`crate::drawing`] for what that cost the first time.
        //
        // Which means it needs what that chain reads, and the tuning is part of
        // that: the draw chain asks it questions like whether shadows are being
        // drawn at all. A missing resource is not a system that skips, it is a
        // panic on the first frame, and the bake is the one place that runs
        // these systems without `crate::add_game` having set the world up.
        .init_resource::<crate::impostor::ImpostorStats>()
        .insert_resource(crate::console::GameTuning::default())
        .add_systems(PostUpdate, crate::drawing())
        .add_observer(collect);
        crate::register_world_asset_types(&mut app);

        // Driven by hand rather than by `App::run`. With `WinitPlugin` off
        // there is no event loop to be the runner, and Bevy's fallback runner
        // calls `update` exactly once and returns -- which looks precisely like
        // a bake that did nothing and said nothing about why.
        //
        // The cap is a bug-stop, not a schedule: every pass this can run, at
        // `angles * frames` cells of `SETTLE` frames each, and generous slack on
        // top for the frames spent waiting on the glTF to load.
        //
        // **Every pass includes the re-surveys.** A survey that finds the actor
        // filling its camera starts again from twice as far back, up to
        // [`WIDENINGS`] times, and each of those is a whole pass of cells. Sized
        // for two, an ant that needed two widenings ran out of budget partway
        // through the final pass and reported it as "something upstream is not
        // producing pixels" -- which is what running out of frames looks like
        // from here, and sent the next reader looking at the renderer.
        app.finish();
        app.cleanup();
        let cap = ELEVATIONS.len() * ANGLES * FRAMES * (WIDENINGS + 2) * (SETTLE + 4) + 2_000;
        let mut frames = 0;
        while app.should_exit().is_none() {
            app.update();
            frames += 1;
            if frames > cap {
                let bake = app.world().resource::<Bake>();
                eprintln!(
                    "impostor bake: gave up after {frames} frames on cell {} of pass {:?}, \
                     having widened the survey {} time(s) -- either something upstream is not \
                     producing pixels, or the cap is too small for the passes actually run",
                    bake.cell, bake.pass, bake.widened,
                );
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell's worth of samples, with `covered` of the sixteen under the texel
    /// at (0, 0) painted `colour` and everything else left as the camera's
    /// transparent clear.
    fn samples(covered: usize, colour: [u8; 3]) -> Vec<u8> {
        let mut pixels = vec![0u8; (SAMPLE_PX * SAMPLE_PX * 4) as usize];
        for sample in 0..covered {
            let (row, column) = (sample as u32 / SUPERSAMPLE, sample as u32 % SUPERSAMPLE);
            let at = ((row * SAMPLE_PX + column) * 4) as usize;
            pixels[at..at + 3].copy_from_slice(&colour);
            pixels[at + 3] = 255;
        }
        pixels
    }

    /// The whole reason for the threshold. An ant's leg is thinner than a texel
    /// and covers a corner of the ones it crosses; a half threshold would erase
    /// it from the sheet as surely as the aliasing it replaced.
    #[test]
    fn a_texel_a_quarter_covered_is_in_the_sprite() {
        let quarter = (SUPERSAMPLE * SUPERSAMPLE / 4) as usize;
        assert_eq!(
            reduce(&samples(quarter, [200, 30, 20]), 0, 0),
            [200, 30, 20, 255],
            "a quarter of a texel is a leg, and legs are the point",
        );
        assert_eq!(
            reduce(&samples(quarter - 1, [200, 30, 20]), 0, 0),
            [0, 0, 0, 0],
            "and below the threshold it is nothing at all",
        );
    }

    /// The colour of an edge texel comes from the model alone. Averaging the
    /// clear colour in with it -- which is what an unweighted mean does -- rings
    /// every sprite in the game with a dark halo.
    #[test]
    fn the_clear_colour_never_reaches_the_sheet() {
        let (colour, covered) = ([180, 40, 30], (SUPERSAMPLE * SUPERSAMPLE / 2) as usize);
        let texel = reduce(&samples(covered, colour), 0, 0);
        assert_eq!(
            [texel[0], texel[1], texel[2]],
            colour,
            "a half-covered texel should be the model's own colour",
        );
        assert_eq!(texel[3], 255, "and opaque, because the sheet is a cutout");
    }

    /// Every texel the reduction writes is one the runtime's `AlphaMode::Mask`
    /// can decide with a single sample. See `n64::cutout`, which promotes a
    /// material to that pass only when the sheet's alpha is binary.
    #[test]
    fn the_reduction_only_ever_writes_binary_alpha() {
        for covered in 0..=(SUPERSAMPLE * SUPERSAMPLE) as usize {
            let alpha = reduce(&samples(covered, [90, 90, 90]), 0, 0)[3];
            assert!(
                alpha == 0 || alpha == 255,
                "{covered} of {} samples gave alpha {alpha}",
                SUPERSAMPLE * SUPERSAMPLE,
            );
        }
    }

    /// The box a texel is reduced from is its own. Reading the wrong stride
    /// would mirror the sheet's rows into its columns, which is a sprite that
    /// looks nearly right and animates wrongly.
    #[test]
    fn a_texel_reads_the_box_it_sits_over() {
        let mut pixels = vec![0u8; (SAMPLE_PX * SAMPLE_PX * 4) as usize];
        // One whole box filled, at texel (3, 5) and nowhere else.
        let (x, y) = (3u32, 5u32);
        for row in 0..SUPERSAMPLE {
            for column in 0..SUPERSAMPLE {
                let at = (((y * SUPERSAMPLE + row) * SAMPLE_PX + x * SUPERSAMPLE + column) * 4)
                    as usize;
                pixels[at..at + 4].copy_from_slice(&[10, 20, 30, 255]);
            }
        }
        assert_eq!(reduce(&pixels, x, y), [10, 20, 30, 255]);
        assert_eq!(reduce(&pixels, y, x), [0, 0, 0, 0], "not its transpose");
        assert_eq!(reduce(&pixels, x, y + 1), [0, 0, 0, 0]);
    }
}
