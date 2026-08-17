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
    // rgb: the key light's colour. a: 1.0 when this surface is lit at all, and
    // 0.0 when its colour is already baked into its vertices -- which is the
    // whole of the castle, whose original RSP lighting was resolved offline.
    light: vec4<f32>,
    // rgb: the ambient term every lit vertex starts from.
    ambient: vec4<f32>,
    // xyz: unit vector from the surface *towards* the key light.
    to_light: vec4<f32>,
    // Alpha below which a fragment is thrown away, or 0.0 to keep every
    // fragment. This is how the tree cards get their outline.
    alpha_cutoff: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: N64Material;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;

// What the vertex stage hands the fragment stage.
//
// Deliberately not `forward_io::VertexOutput`: that one carries a world normal
// across for the fragment stage to light with, and here the lighting is already
// done. The world position is still carried because the fog needs the distance
// from the camera.
struct N64VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
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

    // One diffuse light and one ambient, summed at the vertex. Unclamped on
    // purpose: the RSP's combiner saturated the same way, and letting the term
    // pass 1.0 is what puts a highlight on a surface facing the light.
    var shade = vec3<f32>(1.0, 1.0, 1.0);
#ifdef VERTEX_NORMALS
    if material.light.a > 0.0 {
#ifdef SKINNED
        let world_normal = skinning::skin_normals(world_from_local, vertex.normal);
#else
        let world_normal = mesh_functions::mesh_normal_local_to_world(
            vertex.normal,
            vertex.instance_index,
        );
#endif
        let lambert = max(dot(normalize(world_normal), material.to_light.xyz), 0.0);
        shade = material.ambient.rgb + material.light.rgb * lambert;
    }
#endif

    out.color = vec4<f32>(tint.rgb * shade, tint.a);
    return out;
}

@fragment
fn fragment(in: N64VertexOutput) -> @location(0) vec4<f32> {
    // The interpolated vertex colour modulated by the texture: the combiner
    // step, and the last thing the console did before the blender.
    var color = in.color * textureSample(base_color_texture, base_color_sampler, in.uv);

    if material.alpha_cutoff > 0.0 && color.a < material.alpha_cutoff {
        discard;
    }

    // Fog is a property of the view rather than of the material, and this game
    // only ever sets the linear falloff -- above water and below it are the
    // same curve with different ends. Anything else is left unfogged rather
    // than silently fogged wrongly.
#ifdef DISTANCE_FOG
    if fog.mode == FOG_MODE_LINEAR {
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
