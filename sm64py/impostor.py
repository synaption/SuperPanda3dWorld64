"""Drawing a crowd of one enemy as instanced billboard sprites.

The counterpart to tools/bake_impostor.py. That renders a model to a grid of
angle-by-frame sprites; this draws thousands of them as a single instanced quad
and does the two things a sprite needs -- turn to face the camera, and pick the
cell for its heading and its point in the walk cycle -- on the GPU.

The whole set of one enemy type is one draw call. Per instance the CPU writes
four floats into a texture -- where it is, and which cell -- and the vertex
shader expands each into a camera-facing quad. Nothing here is a NodePath per
object, so the count is bounded by fill rate and by how fast the per-instance
floats can be gathered, not by the node and draw-call overhead that caps the
skinned actors at a few dozen.

Positions arrive in the game's own coordinates and are converted here, the same
way ObjectRenderer converts them, so callers hand over objects untouched.
"""

import json
import math
import os

import numpy as np

from panda3d.core import (Geom, GeomNode, GeomTriangles, GeomVertexData,
                          GeomVertexFormat, GeomVertexWriter,
                          OmniBoundingVolume, SamplerState, Shader, Texture)

from .math_util import to_panda

# gl_InstanceID is core from GLSL 1.40; 150 is what the rest of a modern Panda
# build offers, and gives texelFetch for reading the per-instance table.
VERTEX_SHADER = """#version 150
uniform mat4 p3d_ViewProjectionMatrix;
uniform vec3 camPos;
uniform sampler2D instTex;
uniform float filmSize;
uniform float drawScale;
uniform float footV;
uniform int cols;
uniform int rows;
in vec4 p3d_Vertex;      // x = corner across (-0.5..0.5), y = corner up (0..1)
out vec2 texcoord;

void main() {
    vec4 d = texelFetch(instTex, ivec2(gl_InstanceID, 0), 0);
    vec3 center = d.xyz;          // feet, in Panda world space
    int ci = int(d.w + 0.5);      // which atlas cell
    int row = ci / cols;
    int col = ci - row * cols;

    float ax = p3d_Vertex.x;
    float ay = p3d_Vertex.y;

    // Stand the quad upright and swing it about the vertical to face the
    // camera -- an axis billboard, like the trees, so a steep look does not
    // tip the sprite onto its back.
    vec3 up = vec3(0.0, 0.0, 1.0);
    vec3 toCam = camPos - center;
    toCam.z = 0.0;
    toCam = normalize(toCam);
    vec3 right = normalize(cross(up, toCam));

    float s = filmSize * drawScale;
    vec3 world = center + right * (ax * s) + up * ((ay - footV) * s);
    gl_Position = p3d_ViewProjectionMatrix * vec4(world, 1.0);

    float u = (float(col) + (ax + 0.5)) / float(cols);
    // Panda's texture coordinates start at the bottom of the atlas, as does
    // the quad.  Keeping both directions aligned preserves the bake's upright
    // pixels; reversing `ay` here turned every enemy upside down.
    float v = (float(row) + ay) / float(rows);
    texcoord = vec2(u, v);
}
"""

FRAGMENT_SHADER = """#version 150
uniform sampler2D atlas;
in vec2 texcoord;
out vec4 fragColor;

void main() {
    vec4 c = texture(atlas, texcoord);
    // Alpha test rather than blending, so the sprites need no back-to-front
    // sort and write depth like solid geometry -- which is what lets there be
    // thousands of them without a per-frame ordering pass.
    if (c.a < 0.5) discard;
    fragColor = c;
}
"""


def _quad():
    """A unit quad carrying its corner in (x across, y up)."""
    fmt = GeomVertexFormat.get_v3()
    vdata = GeomVertexData("impostor_quad", fmt, Geom.UH_static)
    vdata.set_num_rows(4)
    writer = GeomVertexWriter(vdata, "vertex")
    for x, y in ((-0.5, 0.0), (0.5, 0.0), (0.5, 1.0), (-0.5, 1.0)):
        writer.add_data3(x, y, 0.0)
    tris = GeomTriangles(Geom.UH_static)
    tris.add_vertices(0, 1, 2)
    tris.add_vertices(0, 2, 3)
    geom = Geom(vdata)
    geom.add_primitive(tris)
    node = GeomNode("impostor")
    node.add_geom(geom)
    return node


class ImpostorField:
    """One enemy type, drawn as an instanced billboard for every live object.

    `angle_offset` lines the runtime heading up with the bake's: the sprite for
    an object is the cell whose baked heading matches the object's heading as
    seen from the camera. The bake looks along one fixed direction and turns the
    model; this turns that around, so the offset and sign are read off a known
    pose once (see tools/check_impostors.py) rather than derived through two
    coordinate conventions.
    """

    angle_offset = 180.0
    angle_sign = 1.0

    def __init__(self, meta_path, parent, draw_scale=1.0, capacity=1024):
        with open(meta_path) as fh:
            self.meta = json.load(fh)
        self.cols = int(self.meta["cols"])
        self.rows = int(self.meta["rows"])
        self.frames = int(self.meta["frames"])
        self.angles = int(self.meta["angles"])
        self.draw_scale = draw_scale

        self.node = parent.attach_new_node(_quad())
        self.node.set_shader(Shader.make(Shader.SL_GLSL,
                                         VERTEX_SHADER, FRAGMENT_SHADER))
        # Baked with the scene's light already in it, so light it no further.
        self.node.set_light_off()
        # The instances stand wherever their table puts them, all over the
        # level, but the node itself sits at the origin with a point for a
        # bound. Left to that bound Panda culls the whole field the moment the
        # origin leaves the frustum -- the crowd blinks out whenever the camera
        # looks away from world centre. An omni bound says "always visible" so
        # every instance is considered every frame; set_final stops the cull
        # from descending past it.
        self.node.node().set_bounds(OmniBoundingVolume())
        self.node.node().set_final(True)
        self.node.set_two_sided(True)

        atlas = self._load_atlas(os.path.join(
            os.path.dirname(meta_path), self.meta["atlas"]))
        self.node.set_shader_input("atlas", atlas)
        self.node.set_shader_input("filmSize", float(self.meta["film"]))
        self.node.set_shader_input("drawScale", float(draw_scale))
        self.node.set_shader_input("footV", float(self.meta["foot_v"]))
        self.node.set_shader_input("cols", self.cols)
        self.node.set_shader_input("rows", self.rows)

        self.capacity = 0
        self._grow(capacity)
        self.count = 0

    def _load_atlas(self, path):
        from panda3d.core import Filename
        tex = Texture("atlas")
        tex.read(Filename.from_os_specific(os.path.abspath(path)))
        # Nearest keeps one cell's pixels from bleeding into its neighbour's
        # across the packed grid, and keeps the retro edge the sprites are.
        tex.set_magfilter(SamplerState.FT_nearest)
        tex.set_minfilter(SamplerState.FT_nearest)
        tex.set_wrap_u(SamplerState.WM_clamp)
        tex.set_wrap_v(SamplerState.WM_clamp)
        return tex

    def _grow(self, capacity):
        """Size the per-instance table to hold at least `capacity` sprites."""
        capacity = max(capacity, 1)
        self.capacity = capacity
        self._data = np.zeros((capacity, 4), dtype=np.float32)
        self._tex = Texture("impostor_data")
        self._tex.setup_2d_texture(capacity, 1, Texture.T_float,
                                   Texture.F_rgba32)
        self._tex.set_magfilter(SamplerState.FT_nearest)
        self._tex.set_minfilter(SamplerState.FT_nearest)
        self.node.set_shader_input("instTex", self._tex)

    def update(self, objects, cam_game):
        """Place a sprite for every live object, and pick each one's cell.

        `objects` is this type's share of the world; `cam_game` the camera in
        game coordinates, for working out which side of each object it sees. The
        walk-cycle frame is read off each object's own `timer`.
        """
        live = [o for o in objects if o.active]
        n = len(live)
        if n == 0:
            self.count = 0
            self.node.set_instance_count(0)
            self.node.hide()
            return
        if n > self.capacity:
            self._grow(1 << (n - 1).bit_length())

        # Gathered into columns and turned into cells with numpy rather than
        # object by object: the per-frame Python cost is one pass to read the
        # positions, and the arithmetic on them is vectorised.
        # draw_pos, not pos: the interpolated render position, so a crowd on a
        # reduced level-of-detail tick rate slides smoothly rather than jumping.
        px = np.fromiter((o.draw_pos[0] for o in live), np.float32, n)
        py = np.fromiter((o.draw_pos[1] for o in live), np.float32, n)
        pz = np.fromiter((o.draw_pos[2] for o in live), np.float32, n)
        yaw = np.fromiter((o.draw_yaw_degrees for o in live),
                          np.float32, n)
        # Enemies carry one clip; step its frames off their own timer so a field
        # of them is not marching in lockstep. `timer` is per object.
        timer = np.fromiter((getattr(o, "timer", 0) for o in live),
                            np.int64, n)

        cx, cy, cz = to_panda(px, py, pz)

        # Which way the camera sees the object, and hence which baked heading
        # presents the same face. Measured in the horizontal plane only.
        cam = to_panda(*cam_game)
        azimuth = np.degrees(np.arctan2(cam[0] - cx, cam[1] - cy))
        # The camera's bearing adds to the heading, it does not subtract: the
        # baked row is the object's facing seen *from* the camera, so turning
        # the object and orbiting the camera move the chosen cell the same way.
        # Head on, azimuth is 180 and its sign cannot be told apart -- which is
        # why a straight-ahead calibration looks right either way and the
        # mirror only shows once an object is off to the side.
        rel = self.angle_sign * yaw + azimuth + self.angle_offset
        row = np.mod(np.round(rel * self.angles / 360.0), self.angles).astype(
            np.int64)
        col = np.mod(timer, self.frames)
        cell = (row * self.cols + col).astype(np.float32)

        self._data[:n, 0] = cx
        self._data[:n, 1] = cy
        self._data[:n, 2] = cz
        self._data[:n, 3] = cell
        self._tex.set_ram_image_as(self._data.tobytes(), "RGBA")

        self.node.set_shader_input("camPos", tuple(float(v) for v in cam))
        self.node.set_instance_count(n)
        self.node.show()
        self.count = n


class ImpostorSet:
    """Every impostor-drawn enemy type, one field each."""

    def __init__(self, impostor_dir, parent, models):
        """`models` maps a model name to the draw scale its objects use."""
        self.parent = parent
        self.fields = {}
        for model, draw_scale in models.items():
            meta = os.path.join(impostor_dir, model + ".json")
            if os.path.exists(meta):
                self.fields[model] = ImpostorField(meta, parent, draw_scale)

    def handles(self, model):
        return model in self.fields

    def update(self, object_set, cam_game):
        by_model = {model: [] for model in self.fields}
        for obj in object_set.objects:
            bucket = by_model.get(obj.model)
            if bucket is not None:
                bucket.append(obj)
        drawn = 0
        for model, field in self.fields.items():
            field.update(by_model[model], cam_game)
            drawn += field.count
        return drawn
