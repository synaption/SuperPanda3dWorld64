// The window in a wall: what the far portal's camera drew, shown where the
// near portal is standing.
//
// The whole trick is that this shader does no projection of its own. A second
// camera has already drawn the world from the eye's *reflection through the
// pair* into a full-screen image, with exactly this camera's field of view and
// exactly this camera's aspect -- so the pixel of that image belonging to any
// point on the portal's surface is the pixel at the same place on the screen.
// The sample is therefore taken by screen position rather than by the quad's
// own texture coordinates, which is what makes the view inside the frame move
// with the player's head instead of being pasted onto the geometry like a
// poster.
//
// `view_transformations::frag_coord_to_uv` is that conversion, and taking it
// from Bevy rather than dividing by hand is what keeps this correct on a
// camera drawing into a sub-viewport of its target -- which is what the
// display module does whenever the internal resolution and the window
// disagree.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::{position_world_to_clip, frag_coord_to_uv},
    mesh_view_bindings::view,
}

struct PortalMaterial {
    // rgb: the ring's colour -- blue or orange, the two ends of the pair.
    // a: how bright the rim burns, which is also what fades a portal in as it
    // is opened.
    rim: vec4<f32>,
    // x: how far in from the edge the rim reaches, in half-widths.
    // y: 1 while there is a picture to show, 0 while the far camera has drawn
    //    nothing yet -- an unpaired gate is an arch with nothing behind it and
    //    is drawn as the frame alone.
    // z: the opening's height over its half-width, which is the one number the
    //    arch's shape needs. See `arch_depth` below.
    // w: nought for an arch and one for a bubble. See `Shape` on the other side
    //    of the bind group.
    shape: vec4<f32>,
}

// How far inside the arch a point is, in half-widths, negative outside it.
//
// **A doorway, not an ellipse.** The opening is two straight jambs rising to a
// semicircular head, which is the shape of every arch anybody has ever walked
// through and reads as one at a glance -- where an ellipse reads as a hole.
// The head's radius is the half-width, so the springing line sits that far
// below the top: everything above it is the semicircle and everything below is
// the gap between the jambs.
//
// Worked in half-widths rather than in the quad's own texture coordinates,
// which is what makes the head *circular* rather than stretched to whatever
// aspect the opening happens to have -- and what makes the frame one thickness
// all the way round instead of thinning at the top.
//
// There is deliberately no bottom edge. A doorway's jambs run into the ground,
// and a distance that counted the sill would draw a bar across it.
fn arch_depth(across: f32, up: f32, height: f32) -> f32 {
    let springing = height - 1.0;
    if up <= springing {
        return 1.0 - abs(across);
    }
    return 1.0 - length(vec2<f32>(across, up - springing));
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: PortalMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var portal_view: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var portal_sampler: sampler;

struct PortalVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Where on the gate's own quad or bubble this fragment is, before the gate
    // was put anywhere. An arch is cut out of its quad by texture coordinate;
    // a bubble is cut in half by which side of the gate's plane it is on, and
    // that plane is `z = 0` in exactly this space. Carrying the local position
    // is what lets one shader answer both without knowing where either gate is
    // standing.
    @location(1) local: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) world_position: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> PortalVertexOutput {
    var out: PortalVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = position_world_to_clip(world_position.xyz);
    out.local = vertex.position;
    out.world_position = world_position.xyz;
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#else
    out.world_normal = vec3<f32>(0.0, 0.0, 1.0);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#else
    out.uv = vec2<f32>(0.5, 0.5);
#endif
    return out;
}

// How far inside the bubble's skin a fragment is, and how near its limb.
//
// A bubble has no edge to measure to the way an arch has: its silhouette is
// wherever the surface has turned away from whoever is looking, which moves as
// they walk round it. So the rim is the limb -- one minus how squarely the
// surface faces the eye -- which is the same band of brightness a soap film has
// and needs no distance at all.
//
fn bubble_limb(normal: vec3<f32>, world_position: vec3<f32>) -> f32 {
    let toward_eye = normalize(view.world_position - world_position);
    let squareness = abs(dot(normalize(normal), toward_eye));
    return pow(1.0 - squareness, 2.5);
}

@fragment
fn fragment(in: PortalVertexOutput) -> @location(0) vec4<f32> {
    // One shader, two shapes, and the branch is the whole of the difference:
    // each one works out how near this fragment is to the edge of the opening,
    // and everything below is the same picture and the same rim.
    var edge: f32;
    if material.shape.w > 0.5 {
        // A bubble. Its mesh is already its silhouette and it is a closed
        // surface, so nothing is stamped out of it at all: back-face culling
        // leaves exactly the half of it turned towards whoever is looking,
        // wherever they are standing, which is what makes it read as a ball
        // rather than as a dome with a hollow back.
        edge = bubble_limb(in.world_normal, in.world_position);
    } else {
        // An arch. The quad is a rectangle and the opening is the arch
        // inscribed in it, so the cut is made here rather than in the mesh: one
        // rectangle is one draw call whatever shape is stamped out of it, and
        // the alternative is a fan of triangles that still has a hard edge.
        //
        // `shape.z` is the opening's height in half-widths, so the coordinates
        // handed to `arch_depth` are metres over the half-width -- one scale
        // for both axes, which is what keeps the head round.
        //
        // **The vertical axis is flipped on the way in.** A texture
        // coordinate's `v` runs down the quad -- nought at the top -- and an
        // arch is the one shape in this game that can tell: negate it and the
        // head is at the top where a doorway's is, keep it and the game draws a
        // horseshoe standing on its own round end. Nothing catches that but a
        // picture, because `Mouth::depth` on the other side of the bind group
        // works in the gate's own frame and is right either way.
        let across = in.uv.x * 2.0 - 1.0;
        let up = 1.0 - in.uv.y * 2.0;
        let height = material.shape.z;
        let depth = arch_depth(across, up * height, height);
        if depth <= 0.0 {
            discard;
        }
        edge = 1.0 - smoothstep(0.0, material.shape.x, depth);
    }

    // The picture, taken at this fragment's place on the screen rather than at
    // its place on the quad. See the note at the top of the file.
    let screen = frag_coord_to_uv(in.position.xy);
    let seen = textureSample(portal_view, portal_sampler, screen).rgb;

    // The rim: a band of the gate's own colour burnt around the inside of the
    // opening, which is what marks the threshold once the picture inside it is
    // a perfectly ordinary view of somewhere else. Above one so that the
    // camera's bloom pass spills it onto the air, the way every other emissive
    // thing in this game is made to glow.
    //
    // Measured on the distance *into* the opening rather than on a radius, so
    // on an arch it is one thickness the whole way round -- up the jambs and
    // over the head alike -- and follows the corner where the two meet. On a
    // bubble it is the limb instead; see [`bubble_limb`].
    let rim = material.rim.rgb * (edge * material.rim.a);

    // With nothing on the far side there is no picture, only the ring: an
    // unpaired portal reads as a lit hoop stuck to the wall rather than as a
    // window onto black.
    let inside = seen * material.shape.y;
    return vec4<f32>(inside * (1.0 - edge) + rim, 1.0);
}
