"""Build Panda3D geometry from the converted level data."""

import json
import os

import numpy as np
from panda3d.core import (
    Geom,
    GeomNode,
    GeomTriangles,
    GeomVertexData,
    GeomVertexFormat,
    GeomVertexWriter,
    NodePath,
    SamplerState,
    Texture,
    TextureStage,
    TransparencyAttrib,
)

from .math_util import to_panda

# Render layers, in the order the geo layout draws them.
LAYER_ORDER = [
    "OPAQUE",
    "OPAQUE_DECAL",
    "ALPHA",
    "TRANSPARENT",
    "TRANSPARENT_DECAL",
]

_TRANSPARENT_LAYERS = {"ALPHA", "TRANSPARENT", "TRANSPARENT_DECAL"}

_WRAP_MODES = {
    "wrap": Texture.WM_repeat,
    "mirror": Texture.WM_mirror,
    "clamp": Texture.WM_clamp,
}

_texture_cache = {}

PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


def resolve_asset(path):
    """Turn a stored texture path into one that exists on this machine.

    Converter output records project-relative paths so the same assets work
    whether they were built under WSL and are being run from Windows or the
    other way round.
    """
    if not path:
        return None
    if os.path.isabs(path):
        return path
    return os.path.join(PROJECT_ROOT, path.replace("/", os.sep))


def _load_texture(path, wrap_s, wrap_t):
    key = (path, wrap_s, wrap_t)
    texture = _texture_cache.get(key)
    if texture is not None:
        return texture

    from panda3d.core import Filename, TexturePool

    texture = TexturePool.load_texture(Filename.from_os_specific(path))
    if texture is None:
        return None

    texture.set_wrap_u(_WRAP_MODES.get(wrap_s, Texture.WM_repeat))
    texture.set_wrap_v(_WRAP_MODES.get(wrap_t, Texture.WM_repeat))
    # The HD art is high resolution; trilinear keeps distant ground calm.
    texture.set_minfilter(SamplerState.FT_linear_mipmap_linear)
    texture.set_magfilter(SamplerState.FT_linear)
    texture.set_anisotropic_degree(4)

    _texture_cache[key] = texture
    return texture


# sRGB texture formats and the plain equivalents to swap them for.
_LINEAR_EQUIVALENT = {
    Texture.F_srgb: Texture.F_rgb,
    Texture.F_srgb_alpha: Texture.F_rgba,
}


def use_linear_textures(node):
    """Strip sRGB decoding from a node's textures.

    Panda3D's glTF loader flags baseColorTexture as sRGB, which the spec asks
    for. Nothing here re-encodes though, and every other texture in the project
    is loaded raw, so that decode gets applied once and never undone.

    It shows up as a colour split rather than an overall shift: a material's
    baseColorFactor is used as written while its texture is darkened, so
    Mario's composited face rendered orange (253, 136, 49) next to the
    untextured parts beside it at the intended (254, 193, 121) -- which is
    exactly sRGB-to-linear applied once.
    """
    for texture in node.find_all_textures():
        replacement = _LINEAR_EQUIVALENT.get(texture.get_format())
        if replacement is not None:
            texture.set_format(replacement)


def use_mipmaps(node):
    """Give a node's textures the same filtering the level geometry gets.

    Textures that arrive embedded in a .glb keep the loader's defaults, which
    means no mipmaps: Mario's face and buttons are 512x512 drawn over a few
    dozen pixels, so minification samples them almost at random and the detail
    crawls as he moves. Requesting a mipmap minfilter is enough -- Panda3D
    generates the chain on upload.
    """
    for texture in node.find_all_textures():
        texture.set_minfilter(SamplerState.FT_linear_mipmap_linear)
        texture.set_magfilter(SamplerState.FT_linear)
        texture.set_anisotropic_degree(4)


def preload(node, gsg):
    """Upload a node's textures and vertex buffers before gameplay starts.

    Panda3D prepares a texture the first frame it is actually drawn, so
    scenery that only becomes visible when the camera swings pays its upload
    cost mid-gameplay as a dropped frame. Doing it up front costs well under a
    millisecond here and removes that class of hitch entirely.
    """
    node.prepare_scene(gsg)


def _decode_normals(raw):
    """Vertex bytes are signed normals when G_LIGHTING is on."""
    signed = raw.astype(np.int16)
    signed[signed > 127] -= 256
    normals = signed[:, :3].astype(np.float32) / 127.0
    lengths = np.linalg.norm(normals, axis=1, keepdims=True)
    lengths[lengths < 1e-6] = 1.0
    return normals / lengths


def _build_geom(positions, colors, uvs, triangles, lighting,
                coordinate_transform=to_panda):
    if lighting:
        fmt = GeomVertexFormat.get_v3n3c4t2()
    else:
        fmt = GeomVertexFormat.get_v3c4t2()

    vdata = GeomVertexData("level", fmt, Geom.UH_static)
    vdata.set_num_rows(len(positions))

    vertex = GeomVertexWriter(vdata, "vertex")
    color = GeomVertexWriter(vdata, "color")
    texcoord = GeomVertexWriter(vdata, "texcoord")
    normal = GeomVertexWriter(vdata, "normal") if lighting else None

    normals = _decode_normals(colors) if lighting else None

    for i, ((x, y, z), (u, v)) in enumerate(zip(positions, uvs)):
        px, py, pz = coordinate_transform(float(x), float(y), float(z))
        vertex.add_data3(px, py, pz)
        # Converter output keeps the N64's top-left UV origin; Panda3D's own
        # texture coordinates start at the bottom-left, so V flips here.
        texcoord.add_data2(float(u), 1.0 - float(v))

        if lighting:
            nx, ny, nz = coordinate_transform(*normals[i])
            normal.add_data3(nx, ny, nz)
            # Lighting supplies the shade; alpha still comes from the vertex.
            color.add_data4(1.0, 1.0, 1.0, colors[i][3] / 255.0)
        else:
            r, g, b, a = colors[i]
            color.add_data4(r / 255.0, g / 255.0, b / 255.0, a / 255.0)

    prim = GeomTriangles(Geom.UH_static)
    for a, b, c in triangles:
        prim.add_vertices(int(a), int(b), int(c))
    prim.close_primitive()

    geom = Geom(vdata)
    geom.add_primitive(prim)
    return geom


def load_level_geometry(npz_path, materials_path=None, name="castle_grounds",
                        coordinate_transform=to_panda):
    """Load a parsed level mesh into a NodePath, one child per material group.

    Groups are kept separate so each can carry its own texture and render
    state; the ordering is also what lets transparent layers composite over
    the opaque ones.
    """
    data = np.load(npz_path)
    positions = data["positions"]
    colors = data["colors"]
    uvs = data["uvs"] if "uvs" in data else np.zeros((len(positions), 2), np.float32)
    triangles = data["triangles"]

    if materials_path is None:
        materials_path = os.path.splitext(npz_path)[0] + "_materials.json"

    if os.path.exists(materials_path):
        with open(materials_path, "r", encoding="utf-8") as fh:
            groups = json.load(fh)
    else:
        groups = [{
            "texture": None, "image": None, "layer": "OPAQUE",
            "lighting": False, "cull": True, "wrap_s": "wrap", "wrap_t": "wrap",
            "first": 0, "count": len(triangles),
        }]

    root = NodePath(name)

    ordered = sorted(
        groups,
        key=lambda g: LAYER_ORDER.index(g["layer"]) if g["layer"] in LAYER_ORDER else 99,
    )

    for index, group in enumerate(ordered):
        first, count = group["first"], group["count"]
        if count <= 0:
            continue

        tris = triangles[first:first + count]
        used = np.unique(tris)
        remap = np.zeros(len(positions), dtype=np.int32)
        remap[used] = np.arange(len(used))

        lighting = bool(group.get("lighting"))
        geom = _build_geom(
            positions[used], colors[used], uvs[used], remap[tris], lighting,
            coordinate_transform,
        )

        node = GeomNode(f"{group['layer']}_{index}")
        node.add_geom(geom)
        np_group = root.attach_new_node(node)

        image = resolve_asset(group.get("image"))
        if image and os.path.exists(image):
            texture = _load_texture(image, group.get("wrap_s", "wrap"),
                                    group.get("wrap_t", "wrap"))
            if texture is not None:
                np_group.set_texture(TextureStage.get_default(), texture)

        if not lighting:
            np_group.set_light_off()

        if not group.get("cull", True):
            np_group.set_two_sided(True)

        if group["layer"] in _TRANSPARENT_LAYERS:
            np_group.set_transparency(TransparencyAttrib.M_alpha)
            np_group.set_bin("transparent", 10 + index)
        if group["layer"].endswith("DECAL"):
            np_group.set_depth_offset(1)

        np_group.set_tag("texture", group.get("texture") or "")
        np_group.set_tag("layer", group["layer"])

    return root


# The water surface texture, and how the original animates it.
WATER_TEXTURE = os.path.join(
    "reference", "RENDER96-HD-TEXTURE-PACK", "gfx", "textures", "segment2",
    "segment2.11C58.rgba16.png")

# Alpha the moving-texture data gives the water quads (0x96 of 0xFF).
WATER_ALPHA = 0x96 / 255.0

# How many times the texture repeats across a water box. The original sizes
# its UVs per quad rather than per box; repeating on a fixed world scale keeps
# the wave size consistent whatever the box measures.
WATER_UV_SCALE = 1.0 / 2048.0

# How fast the surface drifts, in world units per second, and which way.
#
# Expressed as a speed across the world rather than as a spin, because the two
# are not interchangeable here. Rotating the UVs moves every point by its
# distance from the centre of rotation, so one corner of a 15000-unit water box
# crawls while the opposite corner races -- and the centre is wherever UV
# (0.5, 0.5) happens to land, which for these boxes is off in a corner rather
# than the middle. That drove the moat at 1531-2429 units/sec against Mario's
# 960-unit/sec sprint. Translating instead moves the whole sheet at one honest,
# checkable speed. The two bodies drift apart so they do not read as one sheet.
WATER_DRIFT_SPEED = 25.0
WATER_DRIFT_DIRECTION = ((0.60, 0.80), (-0.80, 0.60))


def build_water_surface(surfaces, name="water", coordinate_transform=to_panda):
    """Build a quad for each water box, ready to have its UVs animated.

    Water is not part of the level mesh -- it is the axis-aligned boxes the
    collision data carries, drawn as a flat sheet at each box's height. The
    returned node has one child per box, each tagged with the rotation rate to
    spin its texture at.
    """
    root = NodePath(name)
    texture = None
    image = resolve_asset(WATER_TEXTURE)
    if image and os.path.exists(image):
        texture = _load_texture(image, "wrap", "wrap")

    for i, box in enumerate(surfaces.water_boxes):
        _, x1, z1, x2, z2, y = (int(v) for v in box)
        lo_x, hi_x = min(x1, x2), max(x1, x2)
        lo_z, hi_z = min(z1, z2), max(z1, z2)

        corners = [(lo_x, lo_z), (hi_x, lo_z), (hi_x, hi_z), (lo_x, hi_z)]
        positions = np.array([(x, y, z) for x, z in corners], dtype=np.float32)
        uvs = np.array([(x * WATER_UV_SCALE, z * WATER_UV_SCALE)
                        for x, z in corners], dtype=np.float32)
        colors = np.full((4, 4), 255, dtype=np.uint8)
        colors[:, 3] = int(WATER_ALPHA * 255)
        triangles = np.array([(0, 1, 2), (0, 2, 3)], dtype=np.int32)

        geom = _build_geom(positions, colors, uvs, triangles, lighting=False,
                           coordinate_transform=coordinate_transform)
        node = GeomNode(f"{name}_{i}")
        node.add_geom(geom)
        quad = root.attach_new_node(node)

        if texture is not None:
            quad.set_texture(TextureStage.get_default(), texture)
        quad.set_transparency(TransparencyAttrib.M_alpha)
        quad.set_light_off()
        # Seen from underneath as well, which is most of the time while swimming.
        quad.set_two_sided(True)
        # Drawn after the opaque world so it composites over the lakebed.
        quad.set_bin("transparent", 40 + i)
        quad.set_tag("water_box", str(i))
        direction = WATER_DRIFT_DIRECTION[i % len(WATER_DRIFT_DIRECTION)]
        quad.set_tag("drift", f"{direction[0]},{direction[1]}")

    return root


def animate_water(node, elapsed):
    """Drift each water quad's texture. Call once per frame with the clock."""
    stage = TextureStage.get_default()
    # World units the sheet has travelled, converted into texture repeats.
    distance = WATER_DRIFT_SPEED * elapsed * WATER_UV_SCALE
    for quad in node.get_children():
        tag = quad.get_tag("drift")
        if not tag:
            continue
        dx, dy = (float(v) for v in tag.split(","))
        quad.set_tex_offset(stage, dx * distance, dy * distance)


def load_collision_geometry(npz_path, name="collision",
                            coordinate_transform=to_panda):
    """Build a debug mesh of the collision triangles.

    Useful as an overlay: it shows exactly what the physics sees, which is
    not always what the visual mesh shows.
    """
    from .surfaces import SurfaceSet

    data = np.load(npz_path)
    surfaces = SurfaceSet(
        data["vertices"], data["tri_verts"], data["tri_type"], data["tri_force"]
    )

    positions, colors, triangles = [], [], []

    for i in range(len(surfaces.type)):
        if surfaces.degenerate[i]:
            continue
        base = len(positions)
        for v in (surfaces.v1[i], surfaces.v2[i], surfaces.v3[i]):
            positions.append(tuple(int(c) for c in v))
            if surfaces.is_floor[i]:
                colors.append((60, 200, 90, 255))
            elif surfaces.is_ceil[i]:
                colors.append((200, 90, 60, 255))
            else:
                colors.append((80, 120, 220, 255))
        triangles.append((base, base + 1, base + 2))

    geom = _build_geom(
        np.array(positions, dtype=np.float32),
        np.array(colors, dtype=np.uint8),
        np.zeros((len(positions), 2), dtype=np.float32),
        np.array(triangles, dtype=np.int32),
        lighting=False,
        coordinate_transform=coordinate_transform,
    )
    node = GeomNode(name)
    node.add_geom(geom)
    result = NodePath(node)
    result.set_two_sided(True)
    result.set_light_off()
    return result
