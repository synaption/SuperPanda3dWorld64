//! The look: one diffuse light, resolved per vertex, and nothing else.
//!
//! The N64 had no per-pixel lighting and no shadow maps. Its RSP lit each
//! *vertex* -- normal against one or two fixed light directions, plus an
//! ambient floor -- wrote that into the vertex colour, and let the rasteriser
//! interpolate it across the triangle. Contact shadows were not lighting at all
//! but separate geometry laid on the floor, which is [`crate::shadow`].
//!
//! This module is the Rust half of that: the material, the one set of light
//! terms the whole world shares, and the swap that puts every surface arriving
//! out of a glTF onto it. The shading itself is in `n64.wgsl` next door.
//!
//! Two things are deliberately *not* here. There is no light entity, because
//! there is nothing for one to do: with the whole world on this material the
//! light is three numbers in a uniform, and a `DirectionalLight` would only be
//! a second place for them to live. And there is no shadow map, because asking
//! for one is asking for exactly the modern look this replaces.

use bevy::{
    asset::embedded_asset,
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    render::render_resource::{
        AsBindGroup, Face, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        TextureFormat,
    },
    shader::ShaderRef,
};
use std::collections::HashMap;

/// Where the shader lives once it is compiled into the executable.
///
/// Embedded rather than shipped beside the game on purpose: the Windows
/// packaging script copies a named list of assets, and a shader missing from
/// that list is a game that starts and draws nothing while saying why only in a
/// log nobody has a console open to read.
const SHADER: &str = "embedded://super_bevy_world_64/n64.wgsl";

/// The shader def that moves the lighting from the vertex stage to the
/// fragment stage. Set per pipeline by [`N64Material::specialize`] from the
/// material's own [`Shading`].
const PER_PIXEL_DEF: &str = "N64_PER_PIXEL_LIGHT";

/// Where the one diffuse light is resolved.
///
/// [`Shading::Vertex`] is the console's own answer and the one everything else
/// in this module is written around: the cosine is taken once per vertex and
/// Gouraud interpolated, so the shading breaks along the facets of geometry
/// this coarse. [`Shading::Pixel`] takes the same terms and the same light and
/// resolves them per fragment instead, which is what every renderer since has
/// done -- the models round off and the faceting goes.
///
/// It is a display option rather than a look this game commits to, because the
/// difference is the whole point of the port and is worth being able to see
/// both halves of. It is not a second lighting model: swapping it changes
/// *where* `ambient + key * cos` is evaluated and nothing about what it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Shading {
    #[default]
    Vertex,
    Pixel,
}

impl Shading {
    /// What the menu row calls it.
    pub fn label(self) -> &'static str {
        match self {
            Shading::Vertex => "Per vertex",
            Shading::Pixel => "Per pixel",
        }
    }

    /// The other one. With two modes there is no direction to step in: left
    /// and right on the row both land on the one you are not looking at.
    pub fn other(self) -> Self {
        match self {
            Shading::Vertex => Shading::Pixel,
            Shading::Pixel => Shading::Vertex,
        }
    }
}

/// The light every surface in the world is lit by.
///
/// One key and one ambient, which is what a `GEO_NODE_LIGHT` gave the RSP.
/// These are plain multipliers against a surface's own colour rather than
/// colours in their own right: a lit vertex ends up at `ambient + key * cos`,
/// so the ambient is what a surface facing away from the light keeps and their
/// sum is what one facing straight at it reaches.
#[derive(Resource, Clone, Copy, Debug)]
pub struct N64Lighting {
    /// Unit vector from a surface *towards* the key light. Stored this way
    /// round because it is what the cosine is taken against, and storing the
    /// direction the light travels would mean negating it on every vertex.
    pub to_light: Vec3,
    pub key: Vec3,
    pub ambient: Vec3,
    /// What a surface whose lighting was resolved *offline* is multiplied by.
    ///
    /// The key and the ambient above only reach a surface this renderer lights.
    /// The castle does not qualify -- its shading is its own vertex colours,
    /// baked by the original tools -- and neither do the impostor sheets, which
    /// are photographs of lit models. Both were made under one particular
    /// light, and this is how much of that light there is now: 1.0 is the light
    /// the bake was made under, and anything less is the same scene later in
    /// the day.
    ///
    /// Without it a day and a night is a day and a night for the actors alone,
    /// walking about on grass that stays noon-bright at midnight.
    /// [`crate::sky`] is the only thing that ever moves it.
    pub daylight: Vec3,
    /// Where the lit surfaces' terms are resolved. Lives here rather than beside the
    /// render scale in [`crate::display`] because it travels the same road the
    /// terms themselves do: into every material's uniform and pipeline through
    /// [`translate`] and [`relight`], which already exist to carry a changed
    /// light out to a world full of surfaces.
    pub shading: Shading,
}

impl Default for N64Lighting {
    fn default() -> Self {
        Self {
            // High and a little to one side, so the two visible faces of
            // anything boxy take different amounts of light and the shape
            // reads. Straight overhead would light every wall equally and flat.
            to_light: Vec3::new(0.35, 0.86, 0.37).normalize(),
            // Slightly warm against a slightly cool ambient, and summing to a
            // shade over 1.0 so a surface square-on to the light is a highlight
            // rather than merely its own colour.
            key: Vec3::new(0.68, 0.66, 0.58),
            ambient: Vec3::new(0.42, 0.44, 0.50),
            // Every bake in the game was made under exactly the two terms
            // above, so until something moves the sun a baked surface is shown
            // at full strength and nothing is dimmed at all.
            daylight: Vec3::ONE,
            // The console's answer is what the game starts as. Anything else
            // is something the player asked for.
            shading: Shading::Vertex,
        }
    }
}

/// What the shader reads. Laid out as `vec4`s because a uniform's fields align
/// to sixteen bytes whatever their size, so packing them as `vec3`s would cost
/// the same and read worse.
#[derive(Clone, Copy, ShaderType)]
pub struct N64Uniform {
    base_color: Vec4,
    /// `rgb` the key light. `a` says how this surface is shaded, and it has
    /// three states rather than the two it started with:
    ///
    ///   * **1** -- lit here, out of `light`, `ambient` and `to_light`.
    ///   * **0** -- lit somewhere else, its shading already in its own vertices
    ///     or in its picture, and dimmed by `daylight` as the day turns.
    ///   * **-1** -- luminous: its colour is final and nothing is done to it.
    ///     The sky, which is where the day's light comes *from*.
    light: Vec4,
    ambient: Vec4,
    to_light: Vec4,
    /// `rgb` [`N64Lighting::daylight`], read only by the middle case above.
    daylight: Vec4,
    alpha_cutoff: f32,
    /// 1.0 for a surface standing *in* the world, which the camera's fog closes
    /// over as it gets further away, and 0.0 for one that **is** the distance.
    ///
    /// Fog is a property of the view rather than of a material, so every
    /// surface a fogged camera draws is fogged -- which is right for everything
    /// the game had until the sky arrived. The sky is drawn six hundred units
    /// out, four times past where the haze becomes total, so fogged like the
    /// rest of the world it is a screen of flat fog colour and nothing else:
    /// no gradient, no sun, no stars. It is not *at* a distance, it is what
    /// distance looks like, and [`crate::sky`] matches the fog to its horizon
    /// so the two meet without a seam.
    fogged: f32,
}

impl N64Uniform {
    /// A surface whose colour is already finished: the texture is shown as it
    /// is, with no light of any kind added to it.
    ///
    /// `light.a` of zero is the flag the shader reads, and with it set the
    /// ambient, key and light direction are never looked at. Used by the
    /// impostor sheets, which are pictures of lit models and would be lit twice
    /// over otherwise -- the same reason the castle's own vertex colours are
    /// left alone.
    /// The same surface drawn see-through, at `opacity`.
    ///
    /// The tint multiplies the texture in the shader, so an alpha put here is
    /// an alpha on every texel of the picture -- which is how a sheet baked
    /// solid is drawn as something you can see through without touching the
    /// sheet. See [`crate::impostor`], which is the one caller.
    pub fn faded(mut self, opacity: f32) -> Self {
        self.base_color.w = opacity;
        self
    }

    pub fn unlit(alpha_cutoff: f32) -> Self {
        Self {
            base_color: Vec4::ONE,
            light: Vec4::ZERO,
            ambient: Vec4::ZERO,
            to_light: Vec4::Y,
            daylight: Vec4::ONE,
            alpha_cutoff,
            fogged: 1.0,
        }
    }

    /// A surface that gives off its own light: not lit here, and not dimmed
    /// with the day either.
    ///
    /// The distinction from [`unlit`](Self::unlit) is the whole day and night
    /// cycle. An unlit surface is one whose light was resolved *somewhere
    /// else*, so when the sun goes down it has to be dimmed or the castle stays
    /// noon-bright at midnight. A luminous one has no such debt: the sky is not
    /// lit by the sun, it is where the sun is, and dimming it at dusk would
    /// take the colour out of the sunset. [`crate::sky`] is the only caller.
    pub fn luminous(alpha_cutoff: f32) -> Self {
        Self {
            light: Vec4::new(0.0, 0.0, 0.0, -1.0),
            ..Self::unlit(alpha_cutoff)
        }
    }

    /// The same surface with the camera's fog taken off it. See
    /// [`N64Uniform::fogged`]; [`crate::sky`] is the only caller.
    pub fn beyond_the_fog(mut self) -> Self {
        self.fogged = 0.0;
        self
    }

    /// Whether the camera's fog closes over this surface. See
    /// [`N64Uniform::fogged`]; the field is private, and a test in
    /// [`crate::sky`] has to be able to say that the sky is not fogged.
    #[cfg(test)]
    pub fn is_fogged(&self) -> bool {
        self.fogged > 0.0
    }

    /// The tint the texture is multiplied by, as linear RGBA. The sky's colours
    /// change every few frames and its geometry does not, so it is written here
    /// rather than rebuilt into a mesh.
    pub fn tinted(mut self, tint: Vec4) -> Self {
        self.base_color = tint;
        self
    }
}

/// A surface lit the way the console lit it.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
#[bind_group_data(N64MaterialKey)]
pub struct N64Material {
    #[uniform(0)]
    pub uniform: N64Uniform,
    #[texture(1)]
    #[sampler(2)]
    #[dependency]
    pub base_color_texture: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    /// Drawn from both sides. True for every billboarded quad, because which of
    /// its faces was authored towards the viewer is not something this port can
    /// see -- the reasoning is in [`crate::billboard::two_sided`], which is
    /// where the flag is set before the swap below reads it.
    pub double_sided: bool,
    /// Where this surface's light is worked out. Copied off [`N64Lighting`]
    /// when the material is made and rewritten by [`relight`] when the player
    /// changes it, the same way the light terms themselves are -- except that
    /// this one is a *pipeline* difference rather than a uniform, which is why
    /// it appears in the key below as well.
    pub shading: Shading,
}

/// Everything about a material that changes the *pipeline* rather than the
/// contents of a uniform, and therefore has to be part of what pipelines are
/// cached by.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct N64MaterialKey {
    double_sided: bool,
    per_pixel: bool,
}

impl From<&N64Material> for N64MaterialKey {
    fn from(material: &N64Material) -> Self {
        Self {
            double_sided: material.double_sided,
            // A surface that takes no light at all is the same pipeline either
            // way, so it is pinned to one of them: without this the impostor
            // sheets and the whole castle would be compiled a second time on
            // the first frame after the setting is changed, to draw exactly
            // the picture they already were.
            per_pixel: material.shading == Shading::Pixel && material.uniform.light.w > 0.0,
        }
    }
}

impl Material for N64Material {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    /// No shadow map and no prepass, and both for the same reason: this game
    /// has no light entity for a shadow map to be cast from, and no camera asks
    /// for a depth or normal prepass. Saying so here means neither pipeline is
    /// ever specialised for this material rather than being built and unused.
    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = if key.bind_group_data.double_sided {
            None
        } else {
            Some(Face::Back)
        };
        // A shader def rather than a branch on a uniform, because the two modes
        // are not the same shader with a flag in it: the vertex one hands the
        // fragment stage a finished colour and the pixel one hands it a normal.
        // Compiled this way, the console's path carries no interpolator it does
        // not use and no test it cannot fail.
        if key.bind_group_data.per_pixel {
            descriptor.vertex.shader_defs.push(PER_PIXEL_DEF.into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push(PER_PIXEL_DEF.into());
            }
        }
        Ok(())
    }
}

/// The standard material each already-converted one came from, so a glTF
/// material shared by twenty surfaces is converted once rather than twenty
/// times. The castle alone has forty-five of them across a few thousand
/// triangles.
#[derive(Resource, Default)]
pub struct Converted(HashMap<AssetId<StandardMaterial>, Handle<N64Material>>);

/// An actor drawn see-through, at this much of its own opacity.
///
/// Carried by the thing the port spawns rather than by the surfaces it ends up
/// wearing: the model arrives a frame or more after the spawn, and what is
/// inside it is whatever the glTF happened to contain. [`soften`] walks down
/// from here.
#[derive(Component)]
pub struct Translucent(pub f32);

/// Fades the surfaces of a [`Translucent`] actor before [`convert`] reads them.
///
/// Written onto the standard material, the same way and for the same reason as
/// [`crate::billboard::two_sided`]: every instance of a scene shares one, so
/// one write does the whole crowd, and the swap below then carries it across
/// without being told about any of this.
pub fn soften(
    mut materials: ResMut<Assets<StandardMaterial>>,
    hierarchy: Query<&ChildOf>,
    actors: Query<&Translucent>,
    surfaces: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        Added<MeshMaterial3d<StandardMaterial>>,
    >,
) {
    for (entity, handle) in &surfaces {
        let Some(opacity) = translucency(entity, &hierarchy, &actors) else {
            continue;
        };
        let Some(mut material) = materials.get_mut(&handle.0) else {
            continue;
        };
        // A cutout is left alone. Its alpha is a stencil rather than a depth of
        // material -- the slime's eyes are two eyes painted on a sheet of
        // nothing -- and fading a stencil does not make the shape see-through,
        // it eats the shape inwards from the edge where the mask was soft, and
        // at this port's cutoff it would take the eyes away entirely.
        if matches!(material.alpha_mode, AlphaMode::Mask(_)) {
            continue;
        }
        material.base_color.set_alpha(opacity);
        material.alpha_mode = AlphaMode::Blend;
        // And back to being drawn from one side, whatever `two_sided` decided.
        // That flag is a guess made on behalf of flat quads, which cannot be
        // seen from behind; on a closed body it costs nothing while the body is
        // opaque, and the moment it is not, the inside of the far half of the
        // model is blended over the near half in whatever order the triangles
        // happen to be in.
        material.double_sided = false;
        material.cull_mode = Some(Face::Back);
    }
}

/// How see-through the actor this surface belongs to is, if it is one at all.
///
/// The same walk up to an ancestor as [`in_a_scene`], asking a different
/// question of it.
fn translucency(
    entity: Entity,
    hierarchy: &Query<&ChildOf>,
    actors: &Query<&Translucent>,
) -> Option<f32> {
    let mut ancestor = entity;
    loop {
        if let Ok(Translucent(opacity)) = actors.get(ancestor) {
            return Some(*opacity);
        }
        ancestor = hierarchy.get(ancestor).ok()?.parent();
    }
}

/// Moves everything arriving out of a glTF onto [`N64Material`].
///
/// Bevy's glTF loader only makes standard materials, so the choice is between
/// converting after the fact and writing a loader. This is the former: it reads
/// the handful of fields the console's pipeline had an equivalent for and drops
/// the rest, which is most of a `StandardMaterial`.
///
/// Only scene contents are touched. The flat sheets this port draws itself --
/// the water, the whistle ring, the shadows -- are spawned with their materials
/// already set to what they want and are left alone, which is why this walks up
/// to a [`WorldAssetRoot`] rather than converting every mesh it can see.
#[allow(clippy::too_many_arguments)]
pub fn convert(
    mut commands: Commands,
    lighting: Res<N64Lighting>,
    mut converted: ResMut<Converted>,
    standard: Res<Assets<StandardMaterial>>,
    images: Res<Assets<Image>>,
    mut n64: ResMut<Assets<N64Material>>,
    hierarchy: Query<&ChildOf>,
    scenes: Query<(), With<WorldAssetRoot>>,
    all: Query<&MeshMaterial3d<StandardMaterial>>,
    surfaces: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        Added<MeshMaterial3d<StandardMaterial>>,
    >,
    // Surfaces held back because their texture had not arrived. `Added` fires
    // once and never again, so a surface skipped without this is a surface left
    // on a `StandardMaterial` for the rest of the run.
    mut waiting: Local<Vec<Entity>>,
) {
    let held: Vec<Entity> = waiting.drain(..).collect();
    let again = held
        .into_iter()
        .filter_map(|entity| all.get(entity).ok().map(|handle| (entity, handle)));
    for (entity, handle) in surfaces.iter().chain(again) {
        if !in_a_scene(entity, &hierarchy, &scenes) {
            continue;
        }
        // The cache first, and the texture only if it misses. Every instance
        // of a scene shares one `StandardMaterial`, so asking `drawn_as` before
        // looking here read the same picture once per *entity* wearing it: the
        // castle's fifty-odd trees and pipes between them scanned 27.6 million
        // texels at level load to answer four questions. It is the same answer
        // every time -- the material is the thing being classified.
        let replacement = match converted.0.get(&handle.0.id()) {
            Some(replacement) => replacement.clone(),
            None => {
                let Some(source) = standard.get(&handle.0) else {
                    continue;
                };
                let Some(alpha_mode) = drawn_as(source, &images) else {
                    waiting.push(entity);
                    continue;
                };
                let replacement = n64.add(translate(source, &lighting, alpha_mode));
                converted.0.insert(handle.0.id(), replacement.clone());
                replacement
            }
        };
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(replacement));
    }
}

/// Alphas at or below the first count as nothing and at or above the second as
/// everything. Wide enough to absorb the rounding of a texture that has been
/// through a resize, and nowhere near wide enough to swallow a sheet of glass.
const CLEAR: u8 = 8;
const OPAQUE: u8 = 248;

/// The share of half-transparent texels a picture is allowed before it stops
/// being a cutout and starts being something you can see through.
const CUTOUT: f32 = 0.05;

/// Which pass this material belongs in, or `None` while its texture is still
/// loading.
///
/// **A glTF that says `BLEND` is not necessarily asking for blending.** The
/// tree, the goomba's whites, the scuttlebug's lights and the warp pipe's rim
/// all come out of Blender that way, and every one of them is a cutout: a
/// picture whose alpha is nothing or everything and never in between, which
/// blending draws exactly as a mask would. What the blend costs is the thing
/// that is not free -- a blended surface writes no depth and is ordered against
/// the other transparent things in the scene by the distance to its origin, one
/// object at a time. So a tree standing near the moat is drawn, and then the
/// moat is drawn over it, and a triangle of lake appears in the middle of the
/// canopy; walk twenty metres and the two swap back. That is the trees "showing
/// what is behind them", and it is the same defect wherever two of these
/// overlap.
///
/// A mask has none of that: it goes in the opaque pass, writes depth, and is
/// resolved per pixel by the depth buffer like everything else. It is also what
/// the console did -- the combiner had one bit of alpha for this and the
/// hardware could not have sorted anything anyway.
///
/// So the texture is asked rather than the flag believed. A material whose
/// picture is genuinely translucent -- the castle's one such surface, which
/// tops out at an alpha of 163 and has no solid texel anywhere in it -- is left
/// blended, because for that one the blend is the point.
fn drawn_as(source: &StandardMaterial, images: &Assets<Image>) -> Option<AlphaMode> {
    if !matches!(source.alpha_mode, AlphaMode::Blend) {
        return Some(source.alpha_mode);
    }
    // A tint that is itself see-through settles it without looking at the
    // picture. The rule below is about pictures whose *alpha* is a shape, and
    // the alpha of one of these is beside the point: [`soften`] writes a
    // translucent tint onto a material whose texture is solid everywhere, and
    // asking `cutout` about that texture gets the answer for a surface nobody
    // is drawing.
    if source.base_color.alpha() < 1.0 {
        return Some(AlphaMode::Blend);
    }
    // A blend with no texture at all is a flat tint, and its alpha is the
    // material's own. Nothing to look at, and nothing to promote.
    let Some(texture) = source.base_color_texture.as_ref() else {
        return Some(source.alpha_mode);
    };
    let image = images.get(texture)?;
    Some(match cutout(image) {
        true => AlphaMode::Mask(0.5),
        false => AlphaMode::Blend,
    })
}

/// Is this picture's alpha only ever nothing or everything?
///
/// Answered on the bytes rather than by sampling, because it is asked once per
/// glTF material -- eleven times in the whole game -- and cached in
/// [`Converted`] afterwards. A format this cannot read says no, which leaves the
/// material exactly as the file asked for it.
fn cutout(image: &Image) -> bool {
    if !matches!(
        image.texture_descriptor.format,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    ) {
        return false;
    }
    let Some(data) = image.data.as_ref() else {
        return false;
    };
    let mut texels = 0usize;
    let mut middling = 0usize;
    for texel in data.chunks_exact(4) {
        texels += 1;
        if (CLEAR..OPAQUE).contains(&texel[3]) {
            middling += 1;
        }
    }
    texels > 0 && (middling as f32) < texels as f32 * CUTOUT
}

/// Is this surface part of a loaded scene, rather than something the port
/// spawned directly? Answered by walking up to the root the scene was loaded
/// into, the same way [`crate::billboard::two_sided`] finds the actor a
/// billboard quad belongs to.
fn in_a_scene(
    entity: Entity,
    hierarchy: &Query<&ChildOf>,
    scenes: &Query<(), With<WorldAssetRoot>>,
) -> bool {
    let mut ancestor = entity;
    loop {
        if scenes.contains(ancestor) {
            return true;
        }
        let Ok(parent) = hierarchy.get(ancestor) else {
            return false;
        };
        ancestor = parent.parent();
    }
}

/// One standard material's worth of what the console could express.
///
/// `unlit` is the load-bearing one. Bevy's glTF loader sets it from
/// `KHR_materials_unlit`, which `tools/convert_level.py` writes onto every
/// castle material -- that mesh's lighting was resolved offline and baked into
/// its vertex colours, so lighting it a second time here would light it twice.
/// The actors carry no such flag and are lit live.
///
/// `alpha_mode` is passed in rather than read off the source because it is not
/// always the source's: see [`drawn_as`].
pub fn translate(
    source: &StandardMaterial,
    lighting: &N64Lighting,
    alpha_mode: AlphaMode,
) -> N64Material {
    let lit = if source.unlit { 0.0 } else { 1.0 };
    N64Material {
        uniform: N64Uniform {
            base_color: LinearRgba::from(source.base_color).to_vec4(),
            light: lighting.key.extend(lit),
            ambient: lighting.ambient.extend(0.0),
            to_light: lighting.to_light.extend(0.0),
            daylight: lighting.daylight.extend(0.0),
            alpha_cutoff: match alpha_mode {
                AlphaMode::Mask(cutoff) => cutoff,
                _ => 0.0,
            },
            // Everything arriving out of a glTF stands in the world, so the
            // haze closes over it like anything else.
            fogged: 1.0,
        },
        base_color_texture: source.base_color_texture.clone(),
        alpha_mode,
        double_sided: source.cull_mode.is_none(),
        shading: lighting.shading,
    }
}

/// Pushes a changed [`N64Lighting`] into every material already made.
///
/// The light lives in each material's own uniform rather than in a bind group
/// of its own, which is what keeps the shader down to one buffer and no light
/// list to walk. The cost is this: moving the sun means rewriting every
/// material, so it is only done on the frames the resource actually changes.
pub fn relight(lighting: Res<N64Lighting>, mut materials: ResMut<Assets<N64Material>>) {
    if !lighting.is_changed() || lighting.is_added() {
        return;
    }
    let ids: Vec<_> = materials.ids().collect();
    for id in ids {
        let Some(mut material) = materials.get_mut(id) else {
            continue;
        };
        // The alpha of `light` is whether this surface is lit at all, which is
        // the material's own business and not the sun's.
        let lit = material.uniform.light.w;
        material.uniform.light = lighting.key.extend(lit);
        material.uniform.ambient = lighting.ambient.extend(0.0);
        material.uniform.to_light = lighting.to_light.extend(0.0);
        // Written whatever the surface is. A lit one never reads it and a
        // luminous one is told not to, so `light.a` above stays the one place
        // that decides who gets dimmed and this stays an unconditional write.
        material.uniform.daylight = lighting.daylight.extend(0.0);
        // Changing this changes the pipeline the surface is drawn with, not
        // just its uniform. Writing it through `get_mut` is what makes that
        // happen: the material asset counts as changed, and Bevy re-specialises
        // every entity wearing it on the next frame.
        material.shading = lighting.shading;
    }
}

/// Everything this material needs registered before a frame is drawn.
///
/// A plugin rather than a run of calls in `main` because embedding the shader
/// takes the `App` itself rather than returning to the builder chain, and
/// because [`MaterialPlugin`] has a render-world half that must not be added
/// twice.
pub struct N64Plugin;

impl Plugin for N64Plugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "n64.wgsl");
        app.init_resource::<N64Lighting>()
            .init_resource::<Converted>()
            .add_plugins(MaterialPlugin::<N64Material>::default());
    }
}

/// The swap runs where [`crate::billboard::two_sided`] leaves off, and that
/// order is load-bearing: `two_sided` writes `cull_mode` onto the *standard*
/// material of every billboarded quad, and this reads it a moment later to
/// decide the same thing on the pipeline. Run the other way round, every tree
/// and every scuttlebug's eyes would be culled from half the angles they are looked
/// at from.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (soften, convert, relight).chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard(unlit: bool) -> StandardMaterial {
        StandardMaterial {
            base_color: Color::srgb(1.0, 0.5, 0.25),
            unlit,
            ..default()
        }
    }

    /// The castle's baked vertex colours must not be lit a second time, and the
    /// actors must be lit at all. One flag decides both, so it is worth pinning
    /// which way round it goes.
    #[test]
    fn only_unbaked_surfaces_take_light() {
        let lighting = N64Lighting::default();
        assert_eq!(translate(&standard(false), &lighting, AlphaMode::Opaque).uniform.light.w, 1.0);
        assert_eq!(translate(&standard(true), &lighting, AlphaMode::Opaque).uniform.light.w, 0.0);
    }

    /// The display option reaches the *pipeline*, not just the uniform, and
    /// only for surfaces the choice can be seen on.
    ///
    /// The pinning of unlit surfaces is the half worth a test: they are drawn
    /// by the same shader either way, and letting the key differ would compile
    /// the castle and every impostor sheet a second time -- a hitch, on the
    /// frame the player is looking at the menu row that caused it, for no
    /// change to a single pixel.
    #[test]
    fn only_surfaces_that_take_light_change_pipeline_with_the_setting() {
        let per_pixel = N64Lighting {
            shading: Shading::Pixel,
            ..N64Lighting::default()
        };
        let vertex = N64Lighting::default();
        let key = |material: &N64Material| N64MaterialKey::from(material).per_pixel;

        assert!(!key(&translate(&standard(false), &vertex, AlphaMode::Opaque)));
        assert!(key(&translate(&standard(false), &per_pixel, AlphaMode::Opaque)));
        assert!(
            !key(&translate(&standard(true), &per_pixel, AlphaMode::Opaque)),
            "a surface whose colour is already baked was given a second pipeline \
             to draw the same picture with"
        );
    }

    /// A changed setting has to reach the materials that already exist, which
    /// is every surface in a level that is already up. [`relight`] is what
    /// carries it, and it is also what marks the assets changed so Bevy
    /// re-specialises the meshes wearing them.
    #[test]
    fn changing_the_setting_relights_the_world_already_drawn() {
        // An app rather than a bare world and one run of the system: `relight`
        // deliberately does nothing on the frame the resource *arrives*, so
        // proving it acts on a change means there being an earlier frame for
        // the change to be against.
        let mut app = App::new();
        app.init_resource::<Assets<N64Material>>()
            .init_resource::<N64Lighting>()
            .add_systems(Update, relight);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<N64Material>>()
            .add(translate(
                &standard(false),
                &N64Lighting::default(),
                AlphaMode::Opaque,
            ));
        app.update();

        app.world_mut().resource_mut::<N64Lighting>().shading = Shading::Pixel;
        app.update();

        let materials = app.world().resource::<Assets<N64Material>>();
        let material = materials.get(&handle).expect("the material went missing");
        assert_eq!(material.shading, Shading::Pixel);
        assert!(N64MaterialKey::from(material).per_pixel);
    }

    /// The base-colour picture of the material called `name` in a shipped glTF,
    /// as an `Image` the way the loader would have handed one over.
    ///
    /// Read straight out of the file rather than through the asset server,
    /// which needs a render world this test does not have. The GLB layout is a
    /// twelve-byte header and then length-tagged chunks: JSON first, then the
    /// binary the images are packed into.
    fn baked_picture(file: &str, name: &str) -> Image {
        use bevy::asset::RenderAssetUsages;
        use bevy::render::render_resource::{Extent3d, TextureDimension};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let bytes = std::fs::read(root.join(file)).expect("missing glb");
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&bytes[20..20 + json_len]).expect("bad glb json");
        let bin_at = 20 + json_len;
        let bin_len = u32::from_le_bytes(bytes[bin_at..bin_at + 4].try_into().unwrap()) as usize;
        let bin = &bytes[bin_at + 8..bin_at + 8 + bin_len];

        let material = json["materials"]
            .as_array()
            .expect("no materials")
            .iter()
            .find(|material| material["name"] == name)
            .unwrap_or_else(|| panic!("{file} has no material called {name}"));
        let texture = material["pbrMetallicRoughness"]["baseColorTexture"]["index"]
            .as_u64()
            .expect("no base colour texture") as usize;
        let source = json["textures"][texture]["source"].as_u64().unwrap() as usize;
        let view = &json["images"][source]["bufferView"];
        let view = &json["bufferViews"][view.as_u64().unwrap() as usize];
        let from = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let to = from + view["byteLength"].as_u64().unwrap() as usize;
        let decoded = image::load_from_memory(&bin[from..to])
            .expect("the packed picture is not an image")
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            decoded.into_raw(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        )
    }

    /// The rule that decides which pass a `BLEND` material lands in, pinned
    /// against the two pictures in the game that fall either side of it.
    ///
    /// The tree is why this exists. Blender writes it out as `BLEND`, which
    /// puts it in the transparent queue -- no depth write, and ordered against
    /// the moat one whole object at a time -- so a lake triangle lands in the
    /// middle of its canopy and comes and goes as the camera moves. Its picture
    /// is a cutout: a fifth of a per cent of it is neither solid nor invisible,
    /// which is the rim left by the resize and nothing anybody authored.
    ///
    /// The castle's one genuinely see-through surface is the other side of the
    /// line and has to stay where it is. Its picture has no solid texel in it
    /// at all -- the whole thing tops out at an alpha of 163 -- and drawing it
    /// as a mask would turn a translucent surface opaque.
    #[test]
    fn a_blend_whose_picture_is_a_cutout_is_drawn_as_a_mask() {
        for (file, name) in [
            ("actors/tree.glb", "tree_seg3_texture_0302DE28"),
            ("actors/tree.glb", "tree_seg3_texture_0302EE28"),
            ("actors/goomba.glb", "goomba_seg8_texture_08019530.002"),
            ("actors/warp_pipe.glb", "warp_pipe_seg3_lights_030079E8"),
        ] {
            assert!(
                cutout(&baked_picture(file, name)),
                "{name} in {file} is a cutout and is being drawn blended, which is \
                 a surface the depth buffer cannot order"
            );
        }
        let glass = baked_picture("bevy/castle.glb", "outside_0900BC00");
        assert!(
            !cutout(&glass),
            "the castle's translucent surface was taken for a cutout, which draws \
             it solid"
        );
    }

    /// [`soften`] writes a see-through tint onto a material whose picture is
    /// solid, so the rule that classifies a blend has to ask the tint first.
    /// Asking the picture would promote it to a mask, and a mask draws it
    /// exactly as solid as it was before.
    #[test]
    fn a_see_through_tint_is_blended_whatever_its_picture_says() {
        let mut images = Assets::<Image>::default();
        let picture = images.add(baked_picture("actors/slime.glb", "Slime_Detailed"));
        let mut material = StandardMaterial {
            alpha_mode: AlphaMode::Blend,
            base_color_texture: Some(picture),
            ..standard(false)
        };
        assert_eq!(
            drawn_as(&material, &images),
            Some(AlphaMode::Mask(0.5)),
            "the slime's own picture is solid, so untinted it belongs in the \
             opaque pass -- if it does not, this test is no longer testing the \
             case it was written for"
        );
        material.base_color.set_alpha(0.8);
        assert_eq!(
            drawn_as(&material, &images),
            Some(AlphaMode::Blend),
            "a see-through actor was moved into the opaque pass, which draws it solid"
        );
    }

    /// What [`soften`] leans on in the art. It fades everything on the actor
    /// that is not a cutout, which fades the slime's body and leaves its eyes
    /// to be seen through it -- and that is only true while the eyes are the
    /// cutout and the body is not. A re-export that wrote the eyes out as a
    /// plain blend would fade them along with the body.
    #[test]
    fn the_slimes_eyes_are_the_cutout_and_its_body_is_not() {
        let modes = alpha_modes("actors/slime.glb");
        assert_eq!(modes.get("Slime_Eyes").map(String::as_str), Some("MASK"));
        assert_eq!(modes.get("Slime_Detailed").map(String::as_str), Some("OPAQUE"));
    }

    /// The alpha mode each material in a glTF was written out with, defaulting
    /// the way the format does when the file leaves it out.
    fn alpha_modes(file: &str) -> std::collections::HashMap<String, String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let bytes = std::fs::read(root.join(file)).expect("missing glb");
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&bytes[20..20 + json_len]).expect("bad glb json");
        json["materials"]
            .as_array()
            .expect("no materials")
            .iter()
            .map(|material| {
                (
                    material["name"].as_str().unwrap_or_default().to_owned(),
                    material["alphaMode"].as_str().unwrap_or("OPAQUE").to_owned(),
                )
            })
            .collect()
    }

    /// A `BLEND` with no picture to look at keeps the pass the file asked for,
    /// and everything that was not a blend is left alone entirely.
    #[test]
    fn only_a_blend_with_a_cutout_picture_changes_pass() {
        let images = Assets::<Image>::default();
        for mode in [
            AlphaMode::Opaque,
            AlphaMode::Mask(0.25),
            AlphaMode::Blend,
        ] {
            let material = StandardMaterial {
                alpha_mode: mode,
                ..standard(false)
            };
            assert_eq!(
                drawn_as(&material, &images),
                Some(mode),
                "a material with no base colour texture was moved to another pass"
            );
        }
    }

    /// A masked material keeps its cutoff, and everything else asks for no
    /// discard at all rather than for a cutoff of zero that happens to behave.
    #[test]
    fn only_masked_surfaces_carry_a_cutoff() {
        let lighting = N64Lighting::default();
        let masked = StandardMaterial {
            alpha_mode: AlphaMode::Mask(0.4),
            ..standard(false)
        };
        assert_eq!(translate(&masked, &lighting, masked.alpha_mode).uniform.alpha_cutoff, 0.4);
        assert_eq!(
            translate(&standard(false), &lighting, AlphaMode::Opaque).uniform.alpha_cutoff,
            0.0
        );
    }

    /// The whole point of the thing: a surface facing the light is brighter
    /// than one facing away, and one facing away still gets the ambient rather
    /// than going black. This is the shader's own sum, which cannot be run here
    /// without a GPU, so it is the arithmetic that is checked.
    #[test]
    fn a_lit_surface_is_brighter_facing_the_light_than_away_from_it() {
        let lighting = N64Lighting::default();
        let shade =
            |normal: Vec3| lighting.ambient + lighting.key * normal.dot(lighting.to_light).max(0.0);
        let towards = shade(lighting.to_light);
        let away = shade(-lighting.to_light);
        assert!(
            towards.min_element() > away.max_element(),
            "facing the light gives {towards:?} against {away:?} facing away"
        );
        assert!(
            away.min_element() > 0.0,
            "a surface facing away from the light went black at {away:?}"
        );
        assert!(
            towards.max_element() > 1.0,
            "nothing in the world is ever a highlight: the brightest is {towards:?}"
        );
    }

    /// The direction is a direction. An unnormalised one would scale every
    /// cosine in the world by its length.
    #[test]
    fn the_key_light_points_somewhere_definite() {
        let lighting = N64Lighting::default();
        assert!((lighting.to_light.length() - 1.0).abs() < 1e-5);
        assert!(
            lighting.to_light.y > 0.5,
            "the sun is not above the world: {:?}",
            lighting.to_light
        );
    }

    /// Two surfaces sharing a glTF material share one converted material, which
    /// is what keeps the castle's forty-five from becoming one per drawn chunk.
    #[test]
    fn a_shared_source_material_is_converted_once() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<N64Material>()
            .init_resource::<N64Lighting>()
            .init_resource::<Converted>();
        let source = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(standard(false));
        let world = app.world_mut();
        let root = world.spawn(WorldAssetRoot(Handle::default())).id();
        let surfaces: Vec<_> = (0..3)
            .map(|_| {
                world
                    .spawn((MeshMaterial3d(source.clone()), ChildOf(root)))
                    .id()
            })
            .collect();
        world
            .run_system_once(convert)
            .expect("convert could not run");
        assert_eq!(
            world.resource::<Assets<N64Material>>().len(),
            1,
            "three surfaces on one glTF material made more than one of ours"
        );
        for surface in surfaces {
            assert!(
                world.get::<MeshMaterial3d<N64Material>>(surface).is_some(),
                "a surface was left on the standard material"
            );
        }
    }

    /// Draws a frame with a real GPU behind it, and reports what the driver
    /// made of the shader.
    ///
    /// This is the only test here that can fail on the shader itself, and it is
    /// worth its weight: WGSL is compiled when the first thing using it is
    /// drawn, so a mistake in it is not a build error but a silent black screen
    /// -- and in the Windows build, where there is no console attached, the
    /// error goes to a log nobody can see. Bevy does not treat a failed
    /// pipeline as fatal either, so what is checked is the pipeline cache
    /// itself rather than whether the app fell over.
    ///
    /// No window: an offscreen image is the render target and the windowing
    /// plugin is turned off, so this never opens anything on the desktop.
    ///
    /// The three meshes are the three shapes the shader is specialised into.
    /// Bevy compiles a separate pipeline per combination of vertex attributes,
    /// so a mistake behind `#ifdef SKINNED` -- the branch every actor in this
    /// game takes and the castle does not -- would go unseen with only one.
    ///
    /// Each of them is drawn twice, once per [`Shading`], because the two are
    /// separate pipelines built from separate preprocessed source. The
    /// per-pixel one is the half a player only reaches through a menu row, so
    /// without this a mistake behind `#ifdef N64_PER_PIXEL_LIGHT` would ship
    /// and turn the world black for whoever tried the option.
    #[test]
    fn the_shader_compiles_on_a_real_renderer() {
        use bevy::{
            asset::RenderAssetUsages,
            camera::RenderTarget,
            core_pipeline::tonemapping::Tonemapping,
            mesh::{
                skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
                VertexAttributeValues,
            },
            render::{
                render_resource::{
                    CachedPipelineState, Extent3d, PipelineCache, PipelineDescriptor,
                    TextureDimension, TextureFormat, TextureUsages,
                },
                RenderApp,
            },
            window::ExitCondition,
        };

        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .build()
                // No event loop, so nothing is opened and no display is needed.
                .disable::<bevy::winit::WinitPlugin>()
                // The pipelined renderer hands the render world off to a thread
                // of its own, and this needs to read the pipeline cache out of
                // it afterwards. The game keeps it; only this test does not.
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                // Built rather than queued, so a pipeline that fails has failed
                // by the time the frame returns instead of some frames later.
                .set(bevy::render::RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                }),
        )
        .add_plugins(N64Plugin);

        // The target. Small: nothing here looks at the pixels, only at whether
        // the pipeline that would have drawn them was built.
        let mut target = Image::new_fill(
            Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Bgra8UnormSrgb,
            RenderAssetUsages::default(),
        );
        target.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT;
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);

        let materials: Vec<_> = [Shading::Vertex, Shading::Pixel]
            .map(|shading| {
                let lighting = N64Lighting {
                    shading,
                    ..N64Lighting::default()
                };
                app.world_mut()
                    .resource_mut::<Assets<N64Material>>()
                    .add(translate(&standard(false), &lighting, AlphaMode::Opaque))
            })
            .to_vec();

        // A plain triangle, one with its own COLOR_0 -- which is how the
        // castle's baked lighting arrives -- and one bound to a skeleton.
        let plain = triangle(false);
        let coloured = triangle(true);
        let mut skinned = triangle(false);
        skinned.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(vec![[0u16, 0, 0, 0]; 3]),
        );
        skinned.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_WEIGHT,
            vec![[1.0f32, 0.0, 0.0, 0.0]; 3],
        );

        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let (plain, coloured, skinned) =
            (meshes.add(plain), meshes.add(coloured), meshes.add(skinned));
        let bindposes = app
            .world_mut()
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY]));

        let world = app.world_mut();
        let joint = world.spawn(Transform::default()).id();
        for material in &materials {
            for mesh in [&plain, &coloured] {
                world.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_xyz(0.0, 0.0, 0.0),
                ));
            }
            world.spawn((
                Mesh3d(skinned.clone()),
                MeshMaterial3d(material.clone()),
                SkinnedMesh {
                    inverse_bindposes: bindposes.clone(),
                    joints: vec![joint],
                },
                Transform::default(),
            ));
        }
        world.spawn((
            Camera3d::default(),
            RenderTarget::Image(target.into()),
            // Both of the shader's optional stages: the fog the game's camera
            // carries, and the display transform a non-HDR camera goes through.
            Tonemapping::None,
            crate::water::air_fog(),
            Transform::from_xyz(0.0, 0.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));

        // `run` is what normally does these two, and this drives the loop by
        // hand: without them the renderer's resources never reach the main
        // world and every render system fails on the first frame.
        app.finish();
        app.cleanup();
        // Enough frames for the material to reach the render world and be drawn
        // from there.
        for _ in 0..8 {
            app.update();
        }

        // Which of the pipelines in the cache are this material's, by the
        // shader they were built from. Without this the test would still pass
        // if the meshes above stopped being drawn at all, and would then be
        // checking nothing.
        let shader = app
            .world()
            .resource::<AssetServer>()
            .load::<bevy::shader::Shader>(SHADER)
            .id();
        let render = app.sub_app(RenderApp);
        let cache = render.world().resource::<PipelineCache>();
        let ours = cache
            .pipelines()
            .filter(|pipeline| match &pipeline.descriptor {
                PipelineDescriptor::RenderPipelineDescriptor(descriptor) => {
                    descriptor.vertex.shader.id() == shader
                }
                PipelineDescriptor::ComputePipelineDescriptor(_) => false,
            });

        let mut built = 0;
        let mut broken = Vec::new();
        for pipeline in ours {
            match &pipeline.state {
                CachedPipelineState::Ok(_) => built += 1,
                CachedPipelineState::Err(error) => broken.push(format!("{error}")),
                _ => {}
            }
        }
        assert!(
            broken.is_empty(),
            "the shader did not compile:\n{broken:#?}"
        );
        assert!(
            built >= 6,
            "only {built} of this material's pipelines were built, and there are six \
             to build: three vertex layouts, each lit per vertex and per pixel"
        );
    }

    /// One triangle facing the camera, optionally carrying a vertex colour.
    fn triangle(coloured: bool) -> Mesh {
        use bevy::{
            asset::RenderAssetUsages,
            mesh::{Indices, PrimitiveTopology},
        };

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]],
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 3]);
        if coloured {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0, 1.0, 1.0, 1.0]; 3]);
        }
        mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
        mesh
    }

    /// The sheets this port draws itself are not scene contents and keep the
    /// materials they were spawned with. Converting them would take the water
    /// off the material `water::drift` and `water::camera_medium` expect.
    #[test]
    fn a_surface_outside_a_scene_is_left_alone() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<N64Material>()
            .init_resource::<N64Lighting>()
            .init_resource::<Converted>();
        let source = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(standard(true));
        let world = app.world_mut();
        let sheet = world.spawn(MeshMaterial3d(source)).id();
        world
            .run_system_once(convert)
            .expect("convert could not run");
        assert!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(sheet)
                .is_some(),
            "the water sheet was dragged onto the N64 material"
        );
        assert_eq!(world.resource::<Assets<N64Material>>().len(), 0);
    }
}
