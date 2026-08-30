// The pooled HDR glow worn by every nuclonium orb and stellarator mote.
//
// The CPU writes all cards into one world-space mesh. This shader therefore
// has no particle list to walk and no light to evaluate: it turns the vertex
// colour into an additive radial falloff whose values deliberately pass 1.0,
// giving the camera's bloom pass something to scatter.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
}

struct GlowMaterial {
    // x: halo energy, y: hot-centre energy, z: core radius, w: falloff power.
    shape: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GlowMaterial;

struct GlowVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> GlowVertexOutput {
    var out: GlowVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = position_world_to_clip(world_position.xyz);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#else
    out.uv = vec2<f32>(0.5, 0.5);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#else
    out.color = vec4<f32>(1.0);
#endif
    return out;
}

@fragment
fn fragment(in: GlowVertexOutput) -> @location(0) vec4<f32> {
    let radius = length(in.uv * 2.0 - vec2<f32>(1.0));
    if radius >= 1.0 {
        discard;
    }

    let halo = pow(max(1.0 - radius, 0.0), material.shape.w);
    let core = 1.0 - smoothstep(material.shape.z * 0.72, material.shape.z, radius);
    let tint = in.color.rgb;
    let hot = mix(vec3<f32>(1.0), tint, 0.18);
    let energy = tint * (halo * material.shape.x) + hot * (core * material.shape.y);

    // Alpha zero with Bevy's premultiplied/additive pipeline means src + dst.
    // The radial falloff is already multiplied into `energy` above.
    return vec4<f32>(energy, 0.0);
}
