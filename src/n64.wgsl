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

// One small moving light. See `n64.rs`'s `Lamp` for what fills these in and
// why they cannot live in the uniform above with the sun.
struct N64Lamp {
    // xyz: where it is, in world space. w: how far it reaches, in metres. A
    // reach of zero is an empty slot, which is why there is no count to carry.
    at: vec4<f32>,
    // rgb: what it adds to a surface at its middle.
    glow: vec4<f32>,
}

struct N64Lamplight {
    // One sphere holding every lamp there is: xyz its middle, w how far it
    // goes. See `n64.rs`'s `Lamplight::bounds` -- this is the one compare that
    // stands in front of the loop below.
    bounds: vec4<f32>,
    lamps: array<N64Lamp, #{N64_LAMPS}>,
}

// How the round world is drawn flat under the player's feet. One per frame,
// shared by every material the way the lamps are; `flatten.rs` measures it,
// and `Curve::bend` over there is the reference for `n64_unbend` below -- the
// two must stay the same function, and only that one has tests.
struct N64Curve {
    // xyz: the ridden world's centre as drawn this frame. w: its sea radius.
    home: vec4<f32>,
    // xyz: unit up under the player's drawn feet -- the map's axis.
    // w: the grip, 0 for the true sphere through 1 for the full map.
    zenith: vec4<f32>,
    // x: metres above sea level where a vertex starts to slip off the map.
    // y: where it is free of it. zw: padding.
    band: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: N64Material;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
// Every material in the world binds the *same* buffer here. It is rewritten
// once a frame; no material is touched when a light moves.
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<storage, read> lamplight: N64Lamplight;
// And the same again for the flattening: one shared buffer, rewritten once a
// frame by `flatten::chart`, read by every vertex in the world.
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<storage, read> curve: N64Curve;

// What the vertex stage hands the fragment stage.
//
// Deliberately not `forward_io::VertexOutput`: that one carries a world normal
// across for the fragment stage to light with, and in the vertex-lit pipeline
// the lighting is already done. The world position is still carried because the
// fog needs the distance from the camera.
//
// The normal and the surface's own unlit colour cross as well, and both are
// there for the lamps rather than for the key light.
//
// **The lamps are resolved per fragment even in the console's own pipeline,
// and that is not a compromise on the look.** A lamp reaches a couple of
// metres. The castle grounds are one mesh whose triangles are tens of metres
// across, so a lamp evaluated at their corners is a lamp evaluated nowhere
// near itself: the ball lies in the middle of a triangle and lights none of
// it. Vertex lighting works for the sun because the sun is the same everywhere
// on that triangle, and fails for a lamp for exactly the reason the sun works.
//
// So the key light is still Gouraud and still breaks along the facets, and the
// lamps -- which the console had no equivalent of at all -- are worked out
// where they can be seen. The unlit colour is what makes that addition exact:
// a surface ends up at `base * (shade + lamps) * texture`, and `color` already
// carries `base * shade` with the shade interpolated per vertex, so the second
// term needs `base` on its own. With no lamp in reach this is the same
// arithmetic and the same picture it always was.
struct N64VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_normal: vec3<f32>,
    @location(4) unlit: vec3<f32>,
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

// What the small moving lights add at `world_position`.
//
// The same equation as the sun, once per lamp, with two differences: the
// direction is towards a point rather than fixed, and it falls off. Summed and
// *added* to whatever the surface was already shaded by, so a lamp can only
// ever make a surface brighter -- which is what light does and what the
// combiner could express.
//
// `normal` may be the zero vector, which is a mesh with no normals on it. Such
// a surface takes the lamp's light without a direction rather than none of it:
// the alternative is a wall that is lit and a floor beside it that is not, for
// a reason nothing in the picture explains.
fn n64_lamplight(world_position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    // Nothing in the world can reach this fragment, so do not spend two
    // thousand tests proving it. Squared, so it costs a dot product and no
    // square root, and it is also the empty case: with no lamps the radius is
    // zero and every fragment leaves here.
    let away = lamplight.bounds.xyz - world_position;
    if dot(away, away) >= lamplight.bounds.w * lamplight.bounds.w {
        return sum;
    }
    let oriented = dot(normal, normal) > 0.0;
    for (var slot = 0u; slot < #{N64_LAMPS}u; slot += 1u) {
        let lamp = lamplight.lamps[slot];
        let reach = lamp.at.w;
        // `break`, not `continue`, and that is load-bearing: `n64::nearest`
        // fills the slots from the front and leaves the rest at zero, so the
        // first empty one means there are no more. It is what makes this loop
        // cost one compare on a screen with nothing glowing on it, rather than
        // sixteen -- which is most frames of most sessions.
        if reach <= 0.0 {
            break;
        }
        let toward = lamp.at.xyz - world_position;
        // Compared squared, so the reject path costs a dot product and no
        // square root. This is the path nearly every lamp takes for nearly
        // every fragment -- a lamp reaches a couple of metres and the world is
        // hundreds across -- so it is the one worth being cheap.
        let span = dot(toward, toward);
        if span >= reach * reach {
            continue;
        }
        let apart = sqrt(span);
        // Squared rather than linear, because that is the shape light falling
        // away from a point has -- but bounded, reaching exactly nothing at
        // `reach` rather than merely nearly nothing. A lamp dropped from the
        // list has to be dropped from a surface that was already unlit by it,
        // or leaving the list is a visible step.
        let left = 1.0 - apart / reach;
        var facing = 1.0;
        if oriented {
            let lambert = max(dot(normalize(normal), toward / max(apart, 1e-4)), 0.0);
            // Not all of it is direct. A quarter is kept whichever way the
            // surface faces, standing in for the light that would have come
            // back off everything else in the room -- without it the side of a
            // wall away from a ball goes to nothing while the ground under the
            // ball is bright, which reads as a spotlight rather than a glow.
            let bounce = 0.25;
            facing = bounce + (1.0 - bounce) * lambert;
        }
        sum += lamp.glow.rgb * (left * left * facing);
    }
    return sum;
}

// Where the flattening draws a true world position: `flatten.rs`'s
// `Curve::bend`, line for line. A vertex is measured off the ridden world's
// centre -- its height above sea level and its polar angle from the zenith
// under the player -- and put down on the tangent plane instead: its geodesic
// distance `R·θ` out along the direction it lies in, its height straight up.
// That is the azimuthal equidistant map, and it is chosen because it keeps
// walking distances and directions from the player exact, so the near field
// never swims and a thing due east is drawn due east.
//
// The horizontal direction comes out of `dir - up·cosθ`, whose length is
// exactly sinθ, so the unrolling multiplies by θ/sinθ: 1 under the player's
// feet, growing gently with distance, and clamped at the antipode where it is
// genuinely singular -- uncapped, a triangle with a vertex in the last
// fraction of a degree is flung millions of metres and smeared across the sky.
//
// The grip blends the true position towards the map, faded per vertex over
// the altitude band, which is what keeps the sun, the other planet and the
// rest of the true sky exactly where they are.
fn n64_unbend(world: vec3<f32>) -> vec3<f32> {
    let grip = curve.zenith.w;
    if grip <= 0.0 {
        return world;
    }
    let centre = curve.home.xyz;
    let radius = curve.home.w;
    let up = curve.zenith.xyz;
    let arm = world - centre;
    let reach = length(arm);
    if reach < 1e-3 {
        return world;
    }
    let height = reach - radius;
    let tall = grip * (1.0 - smoothstep(curve.band.x, curve.band.y, height));
    if tall <= 0.0 {
        return world;
    }
    let dir = arm / reach;
    let cos_polar = clamp(dot(dir, up), -1.0, 1.0);
    let level = dir - up * cos_polar;
    let sin_polar = length(level);
    // atan2 of the parts, never acos of the dot: near the zenith -- the
    // player himself -- acos divides the dot's f32 noise by a near-zero sine
    // and `stretch` divides by it again, which shredded the player's model
    // into half-metre shrapnel. atan2 reads the angle off the sine, so the
    // ratio below shares its errors top and bottom and cancels them. See
    // `Curve::bend`.
    let polar = atan2(sin_polar, cos_polar);
    // And past the horizon band the far side settles back onto the true
    // sphere, below the flattened plane where the nearer ground hides it --
    // see `HORIZON` in flatten.rs.
    let share = tall * (1.0 - smoothstep(curve.band.z, curve.band.w, polar));
    if share <= 0.0 {
        return world;
    }
    var stretch = 1.0;
    if sin_polar > 1e-6 {
        stretch = min(polar / sin_polar, 8.0);
    }
    let flat = centre + up * (radius + height) + level * (radius * stretch);
    return mix(world, flat, share);
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
    // The flattening bends only the *clip* position. `out.world_position`
    // stays true on purpose: the key light, the lamps and the fog all read it
    // in the fragment stage, and lighting the true sphere is what keeps the
    // terminator where the sky says the sun is and a lamp's glow on the
    // ground its ball is truly lying on. The map moves where a triangle lands
    // on the glass and changes nothing about its colour -- so no normal needs
    // reconstructing either.
    //
    // The sky is the one opt-out, and it already carries the flag: `fogged`
    // is zero exactly for the shells that *are* the distance rather than
    // standing in it, and bending the picture of the sky by the map of the
    // ground would fold the stars at the horizon.
    var drawn = out.world_position.xyz;
    if material.fogged > 0.0 {
        drawn = n64_unbend(drawn);
    }
    out.position = position_world_to_clip(drawn);
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

    // Wanted by the fragment stage whichever pipeline this is: see the struct.
    out.world_normal = world_normal;
    out.unlit = tint.rgb;

#ifdef N64_PER_PIXEL_LIGHT
    // Nothing is resolved here: the colour handed across is the surface's own,
    // and the light is taken against the interpolated normal per fragment.
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

    // And the lamps, on top of whatever the surface was already shaded by --
    // including the baked surfaces, which is the case that matters most: the
    // castle and its grounds are what a ball of nuclonium is lying *on*. Their
    // vertex colours were baked under a sun that knew nothing about it, and a
    // light added afterwards is exactly what the combiner could still do to
    // them.
    //
    // Not the luminous case, which is the sky. The sky is where light comes
    // from rather than something light falls on.
    if material.light.a >= 0.0 {
        let lamps = n64_lamplight(in.world_position.xyz, in.world_normal);
        tint = vec4<f32>(tint.rgb + in.unlit * lamps, tint.a);
    }

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
