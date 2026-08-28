// Vertex lighting, the way the console did it.
//
// The N64's RSP lit a *vertex*, not a pixel: it took the vertex normal, took
// the cosine against one or two fixed light directions, added an ambient term
// and wrote the result into the vertex's colour. The rasteriser then Gouraud
// interpolated that colour across the triangle and the combiner multiplied it
// by the texture. There was no per-pixel normal, no specular lobe, and no
// shadow map anywhere in the pipeline -- shadows were separate geometry laid on
// the floor, which is what `shadow.rs` draws.
//
// So this shader is deliberately not a cheaper PBR. The lighting is finished
// before rasterisation, which is why what crosses from the vertex stage to the
// fragment stage below is a colour rather than a normal. On geometry this
// coarse that is a visible difference and not an optimisation: the shading
// breaks along the edges of the model's own facets instead of flowing smoothly
// over them.
//
// `N64_PER_PIXEL_LIGHT` is the other half of the display option that lets you
// see that for yourself. With it set, the same key, ambient and direction are
// resolved per fragment instead: the normal crosses to the fragment stage and
// the cosine is taken there, so the facets round off and the model is shaded
// the way a machine with a per-pixel pipeline would have shaded it. It is one
// shader with two pipelines rather than one pipeline with a branch -- what
// crosses the stage boundary differs, so the console's path pays nothing for
// the option existing. `n64.rs` decides which, per material.
//
// The vertex stage is Bevy's own `mesh.wgsl` with the lighting folded in, so
// skinning is done the same way the standard material does it -- every actor in
// this game is a skinned mesh and the castle is the only thing that is not.
// Morph targets are the one part of `mesh.wgsl` left out: nothing this game
// loads has any (`billboard.rs`'s tests read the same GLBs), and carrying the
// block would mean carrying `bevy_pbr::morph` for geometry that never uses it.

#import bevy_pbr::{
    mesh_functions,
    skinning,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::view,
}

// The fog uniform only exists when the view has fog on it, so both the import
// and the use of it are behind the same flag Bevy sets for that.
#ifdef DISTANCE_FOG
#import bevy_pbr::{
    fog::linear_fog,
    mesh_view_bindings::fog,
    mesh_view_types::FOG_MODE_LINEAR,
}
#endif

#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::tone_mapping
#endif

struct N64Material {
    // Tint applied to the texture, straight from the glTF material.
    base_color: vec4<f32>,
    // rgb: the key light's colour. a: how this surface is shaded.
    //    1: lit here, from the three terms below.
    //    0: lit offline -- the whole of the castle, whose RSP lighting was
    //       resolved into its vertex colours, and the impostor sheets, which
    //       are photographs of lit models. Their colour is finished, but it
    //       was finished under a noon sun, so `daylight` dims it as the day
    //       turns.
    //   -1: luminous. The sky, which is where the light comes from rather than
    //       something the light falls on, and which is therefore never dimmed.
    light: vec4<f32>,
    // rgb: the ambient term every lit vertex starts from.
    ambient: vec4<f32>,
    // xyz: unit vector from the surface *towards* the key light.
    to_light: vec4<f32>,
    // rgb: how much of the light a baked surface was baked under is left. Read
    // only in the `a == 0` case above.
    daylight: vec4<f32>,
    // Alpha below which a fragment is thrown away, or 0.0 to keep every
    // fragment. This is how the tree cards get their outline.
    alpha_cutoff: f32,
    // 1.0 for a surface in the world and 0.0 for the sky, which is drawn far
    // past where the haze becomes total and would otherwise be a flat screen of
    // fog colour. See `N64Uniform::fogged`.
    fogged: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: N64Material;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;

// What the vertex stage hands the fragment stage.
//
// Deliberately not `forward_io::VertexOutput`: that one carries a world normal
// across for the fragment stage to light with, and in the vertex-lit pipeline
// the lighting is already done. The world position is still carried because the
// fog needs the distance from the camera.
//
// The per-pixel pipeline is the case that does need the normal, and it is the
// only one that carries it -- an interpolator the console's path would spend on
// a value it never reads.
struct N64VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
#ifdef N64_PER_PIXEL_LIGHT
    @location(3) world_normal: vec3<f32>,
#endif
}

// `ambient + key * cos`, the one lighting equation this game has, wherever it
// is being evaluated. `normal` is expected normalised; a surface that takes no
// light keeps its own colour and never reaches here.
fn n64_shade(normal: vec3<f32>) -> vec3<f32> {
    // Unclamped on purpose: the RSP's combiner saturated the same way, and
    // letting the term pass 1.0 is what puts a highlight on a surface facing
    // the light.
    let lambert = max(dot(normal, material.to_light.xyz), 0.0);
    return material.ambient.rgb + material.light.rgb * lambert;
}

@vertex
fn vertex(vertex: Vertex) -> N64VertexOutput {
    var out: N64VertexOutput;

#ifdef SKINNED
    let world_from_local = skinning::skin_model(
        vertex.joint_indices,
        vertex.joint_weights,
        vertex.instance_index,
    );
#else
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
#endif

#ifdef VERTEX_POSITIONS
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#else
    out.uv = vec2<f32>(0.0, 0.0);
#endif

    // The tint the vertex starts with, before any light reaches it. A mesh's
    // own COLOR_0 is a multiplier here rather than a replacement, which is what
    // makes the castle work: its colours *are* its lighting.
    var tint = material.base_color;
#ifdef VERTEX_COLORS
    tint = tint * vertex.color;
#endif

    // The normal, in world space, however this mesh is posed. Wanted by both
    // pipelines: one lights with it here and the other sends it across.
#ifdef VERTEX_NORMALS
#ifdef SKINNED
    let world_normal = skinning::skin_normals(world_from_local, vertex.normal);
#else
    let world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif
#else
    // A mesh with no normals cannot be lit by either pipeline. Zero rather
    // than a made-up direction, because zero is what the fragment stage tests
    // for -- see the length check down there.
    let world_normal = vec3<f32>(0.0, 0.0, 0.0);
#endif

#ifdef N64_PER_PIXEL_LIGHT
    // Nothing is resolved here: the colour handed across is the surface's own,
    // and the light is taken against the interpolated normal per fragment.
    out.world_normal = world_normal;
    out.color = tint;
#else
    // One diffuse light and one ambient, summed at the vertex, and the last
    // this pipeline thinks about lighting.
    //
    // A luminous surface falls through both arms and keeps the 1.0 it starts
    // with, which is what makes the sky the one thing in the world the time of
    // day is not applied to twice.
    var shade = vec3<f32>(1.0, 1.0, 1.0);
    if material.light.a > 0.0 {
        // A mesh with no normals cannot be lit, and keeps its own colour --
        // see the zero written above.
        if dot(world_normal, world_normal) > 0.0 {
            shade = n64_shade(normalize(world_normal));
        }
    } else if material.light.a >= 0.0 {
        // Already lit, somewhere else and at some other hour.
        shade = material.daylight.rgb;
    }
    out.color = vec4<f32>(tint.rgb * shade, tint.a);
#endif
    return out;
}

@fragment
fn fragment(in: N64VertexOutput) -> @location(0) vec4<f32> {
    var tint = in.color;

#ifdef N64_PER_PIXEL_LIGHT
    // Gouraud interpolation carried a *colour* across; this carries the normal
    // and lights here, so the shading follows the interpolated surface instead
    // of the triangle's three corners. The length test is the mesh-has-no-
    // normals case: interpolating zeroes gives a zero vector, and normalizing
    // one of those is a NaN across the whole triangle.
    //
    // Only the lit case ever reaches here: `N64MaterialKey` compiles the
    // per-pixel pipeline for a surface with `light.a > 0.0` and no other, so a
    // baked surface is drawn by the vertex pipeline above whatever the player
    // has the display option set to -- which is where its `daylight` is
    // applied.
    if material.light.a > 0.0 && dot(in.world_normal, in.world_normal) > 0.0 {
        tint = vec4<f32>(tint.rgb * n64_shade(normalize(in.world_normal)), tint.a);
    }
#endif

    // The interpolated vertex colour modulated by the texture: the combiner
    // step, and the last thing the console did before the blender.
    var color = tint * textureSample(base_color_texture, base_color_sampler, in.uv);

    if material.alpha_cutoff > 0.0 && color.a < material.alpha_cutoff {
        discard;
    }

    // Fog is a property of the view rather than of the material, and this game
    // only ever sets the linear falloff -- above water and below it are the
    // same curve with different ends. Anything else is left unfogged rather
    // than silently fogged wrongly -- and so is the sky, which is the far
    // distance rather than something standing in it.
#ifdef DISTANCE_FOG
    if fog.mode == FOG_MODE_LINEAR && material.fogged > 0.0 {
        let distance = length(in.world_position.xyz - view.world_position);
        color = linear_fog(fog, color, distance, vec3<f32>(0.0, 0.0, 0.0));
    }
#endif

    // The camera renders with `Tonemapping::None`, so this is exposure and
    // colour grading and nothing else. It is still applied because the water
    // sheet is a standard material that goes through it, and two surfaces of
    // the same colour must not come out different shades.
#ifdef TONEMAP_IN_SHADER
    color = tone_mapping(color, view.color_grading);
#endif

    return color;
}
