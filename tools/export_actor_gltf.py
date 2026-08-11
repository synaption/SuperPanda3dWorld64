"""Export a decomp actor -- geometry, skeleton and animations -- as a .glb.

SM64 actors are rigidly segmented rather than smooth-skinned: each body part is
its own display list authored in its joint's local space.  That makes the
conversion exact rather than approximate -- every vertex binds to exactly one
joint with weight 1.0, which is precisely what the original hardware did when
it multiplied each part's vertices by that joint's matrix.

The joint order is not stored anywhere.  It is the order animated parts are
visited walking the geo layout depth-first, and the animation index table is
read in lockstep with that walk.  The exporter cross-checks the two: if the
joint count from the hierarchy disagrees with the count implied by the
animation tables, the mapping is wrong and it says so.

Usage:
    python3 tools/export_actor_gltf.py --actor mario -o mario.glb
    python3 tools/export_actor_gltf.py --actor mario -o mario.glb --anims all
"""

import argparse
import json
import math
import os
import re
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import rig
import sm64_anim
from geo_layout import animated_parts, build_tree, parse_geo_layouts
from glb import (
    ARRAY_BUFFER,
    ELEMENT_ARRAY_BUFFER,
    FLOAT,
    GLB,
    UNSIGNED_INT,
    UNSIGNED_SHORT,
)
from parse_f3d import Level, MeshBuilder, build_texture_map, resolve_textures

# Commands that place a transform in the chain, and so become joints.
TRANSFORM_KINDS = {
    "GEO_ANIMATED_PART",
    # A billboard carries no transform of its own, but making it a joint is
    # what gives the renderer something it can rotate. Its geometry is skinned,
    # so a node-level billboard effect cannot touch it -- driving the joint can.
    "GEO_BILLBOARD", "GEO_BILLBOARD_WITH_PARAMS",
    "GEO_ROTATION_NODE", "GEO_ROTATION_NODE_WITH_DL",
    "GEO_TRANSLATE_NODE", "GEO_TRANSLATE_NODE_WITH_DL",
    "GEO_TRANSLATE_ROTATE", "GEO_TRANSLATE_ROTATE_WITH_DL",
    "GEO_SCALE", "GEO_SCALE_WITH_DL",
}

# Readable names for the standard 20-part Mario rig, in visit order.
MARIO_JOINT_NAMES = [
    "root", "butt", "torso", "head",
    "shoulder.L", "upper_arm.L", "forearm.L", "hand.L",
    "shoulder.R", "upper_arm.R", "forearm.R", "hand.R",
    "hip.L", "thigh.L", "shin.L", "foot.L",
    "hip.R", "thigh.R", "shin.R", "foot.R",
]

FRAME_RATE = 30.0

# The actor's own geo layout wraps the body in GEO_SCALE(0x00, 16384), and
# 0x10000 means 1.0 -- so Mario is authored at 4x and shrunk to a quarter at
# draw time.  Exporting at 0.25 lands the model in the same units as the
# level and the collision data (~154 units tall).
# Actors whose geo wraps the body in GEO_SCALE(0x00, 16384) are authored at 4x
# and shrink to a quarter at draw time. Actors without it -- the tree is one --
# are already at world scale, and applying the quarter anyway leaves them a
# quarter of the size the level was built around.
ACTOR_SCALE = {"mario": 0.25, "goomba": 1.0, "scuttlebug": 1.0, "tree": 1.0}
DEFAULT_SCALE = 0.25

# Parts the game only draws under some runtime condition, and which therefore
# should not be in the default model. Mario's wings sit under a GEO_ASM hook
# (geo_mario_rotate_wing_cap_wings) that only emits them while the wing cap is
# active; exported unconditionally they stick out of his head at all times.
DEFAULT_EXCLUDE = {"mario": r"cap_wings"}


# -- maths ------------------------------------------------------------------


def binary_to_radians(angle):
    return (angle & 0xFFFF) * (2.0 * math.pi / 65536.0)


def euler_to_matrix(rx, ry, rz):
    """Rotation matrix for binary angles, composed as Rz * Ry * Rx."""
    ax, ay, az = (binary_to_radians(a) for a in (rx, ry, rz))
    sx, cx = math.sin(ax), math.cos(ax)
    sy, cy = math.sin(ay), math.cos(ay)
    sz, cz = math.sin(az), math.cos(az)

    mx = np.array([[1, 0, 0], [0, cx, -sx], [0, sx, cx]], dtype=np.float64)
    my = np.array([[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]], dtype=np.float64)
    mz = np.array([[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]], dtype=np.float64)
    return mz @ my @ mx


def euler_to_quaternion(rx, ry, rz):
    """Quaternion (x, y, z, w) for the same Rz * Ry * Rx composition."""
    ax, ay, az = (binary_to_radians(a) / 2.0 for a in (rx, ry, rz))

    qx = (math.sin(ax), 0.0, 0.0, math.cos(ax))
    qy = (0.0, math.sin(ay), 0.0, math.cos(ay))
    qz = (0.0, 0.0, math.sin(az), math.cos(az))

    def mul(a, b):
        ax_, ay_, az_, aw = a
        bx, by, bz, bw = b
        return (
            aw * bx + ax_ * bw + ay_ * bz - az_ * by,
            aw * by - ax_ * bz + ay_ * bw + az_ * bx,
            aw * bz + ax_ * by - ay_ * bx + az_ * bw,
            aw * bw - ax_ * bx - ay_ * by - az_ * bz,
        )

    return mul(qz, mul(qy, qx))


def compose(translation, rotation, scale=1.0):
    matrix = np.eye(4)
    matrix[:3, :3] = euler_to_matrix(*rotation) * scale
    matrix[:3, 3] = translation
    return matrix


# -- skeleton ---------------------------------------------------------------


class Joint:
    __slots__ = ("name", "parent", "translation", "rotation", "scale",
                 "animated", "display_lists", "rest_global")

    def __init__(self, name, parent, translation, rotation, scale, animated):
        self.name = name
        self.parent = parent
        self.translation = translation
        self.rotation = rotation
        self.scale = scale
        self.animated = animated
        self.display_lists = []
        self.rest_global = np.eye(4)


def collect_joints(root, names=None):
    """Flatten the geo tree into a joint list, in animation visit order."""
    joints = [Joint("armature", -1, (0.0, 0.0, 0.0), (0, 0, 0), 1.0, False)]

    def walk(node, parent):
        for child in node.children:
            if child.kind in TRANSFORM_KINDS:
                joint = Joint(
                    child.kind, parent, child.translation, child.rotation,
                    child.scale, child.animated,
                )
                joint.display_lists.extend(child.display_lists)
                joints.append(joint)
                walk(child, len(joints) - 1)
            else:
                # Switches, ASM hooks and bare display lists carry no
                # transform: their geometry belongs to the enclosing joint.
                joints[parent].display_lists.extend(child.display_lists)
                walk(child, parent)

    walk(root, 0)

    # Rest-pose global transforms, needed for inverse bind matrices.
    for index, joint in enumerate(joints):
        local = compose(joint.translation, joint.rotation, joint.scale)
        if joint.parent < 0:
            joint.rest_global = local
        else:
            joint.rest_global = joints[joint.parent].rest_global @ local

    if names:
        animated = [j for j in joints if j.animated]
        for joint, name in zip(animated, names):
            joint.name = name
        for index, joint in enumerate(joints):
            if not joint.animated and joint.name in TRANSFORM_KINDS:
                joint.name = f"fixed_{index}"
    else:
        for index, joint in enumerate(joints):
            if joint.name.startswith("GEO_BILLBOARD"):
                # Named so the renderer can find them without a sidecar.
                joint.name = f"billboard_{index}"
            elif joint.name in TRANSFORM_KINDS:
                joint.name = f"joint_{index}"

    return joints


# -- geometry ---------------------------------------------------------------


def extract_meshes(level, joints, exclude=None):
    """Run every joint's display lists, tagging vertices with their joint."""
    builder = MeshBuilder(level)
    builder.begin()

    for index, joint in enumerate(joints):
        if not joint.display_lists:
            continue
        builder.bone = index
        for layer, symbol in joint.display_lists:
            if exclude is not None and exclude.search(symbol):
                continue
            builder._layer = layer.replace("LAYER_", "")
            builder.run(symbol)

    return builder.finish(), builder.vertex_bones


def compute_normals(positions, triangles, existing, lighting_mask):
    """Area-weighted vertex normals, used where the model has none of its own."""
    normals = np.zeros((len(positions), 3), dtype=np.float64)
    v = positions[triangles]
    face = np.cross(v[:, 1] - v[:, 0], v[:, 2] - v[:, 0])

    for i in range(3):
        np.add.at(normals, triangles[:, i], face)

    lengths = np.linalg.norm(normals, axis=1, keepdims=True)
    lengths[lengths < 1e-9] = 1.0
    normals /= lengths

    # Where the model stored real normals, keep them.
    if existing is not None and lighting_mask is not None:
        normals[lighting_mask] = existing[lighting_mask]
    return normals


def decode_stored_normals(colors):
    signed = colors.astype(np.int16)
    signed[signed > 127] -= 256
    normals = signed[:, :3].astype(np.float64) / 127.0
    lengths = np.linalg.norm(normals, axis=1, keepdims=True)
    lengths[lengths < 1e-9] = 1.0
    return normals / lengths


# -- export -----------------------------------------------------------------


def export(actor_dir, anim_dir, reference, hd_pack, out_path, root_layout,
           switch_case=0, scale=DEFAULT_SCALE, animations=None,
           joint_names=None, embed_textures=True, exclude=None):
    layouts = parse_geo_layouts([
        os.path.join(actor_dir, name)
        for name in os.listdir(actor_dir) if name.endswith(".inc.c")
    ])
    if root_layout not in layouts:
        raise SystemExit(f"layout {root_layout!r} not found in {actor_dir}")

    tree = build_tree(layouts, root_layout, switch_case=switch_case)
    joints = collect_joints(tree, joint_names)
    animated = [i for i, j in enumerate(joints) if j.animated]

    level = Level()
    for name in os.listdir(actor_dir):
        if name.endswith(".inc.c"):
            level.add_source(os.path.join(actor_dir, name))

    mesh, vertex_bones = extract_meshes(level, joints, exclude)
    positions = mesh["positions"].astype(np.float64)
    triangles = mesh["triangles"]
    colors = mesh["colors"]
    uvs = mesh["uvs"]
    groups = mesh["groups"]
    vertex_bones = np.array(vertex_bones, dtype=np.int32)

    if len(positions) == 0:
        raise SystemExit("no geometry found -- check the root layout name")

    lighting_mask = np.zeros(len(positions), dtype=bool)
    for group in groups:
        if group["lighting"]:
            tris = triangles[group["first"]:group["first"] + group["count"]]
            lighting_mask[np.unique(tris)] = True

    normals = compute_normals(
        positions, triangles, decode_stored_normals(colors), lighting_mask
    )

    # Bake each part into model space; skinning undoes it via the inverse
    # bind matrix, which is what glTF expects.
    baked = np.empty_like(positions)
    baked_normals = np.empty_like(normals)
    for index in np.unique(vertex_bones):
        matrix = joints[index].rest_global
        mask = vertex_bones == index
        pts = np.c_[positions[mask], np.ones(mask.sum())]
        baked[mask] = (matrix @ pts.T).T[:, :3]
        baked_normals[mask] = (matrix[:3, :3] @ normals[mask].T).T

    baked *= scale
    lengths = np.linalg.norm(baked_normals, axis=1, keepdims=True)
    lengths[lengths < 1e-9] = 1.0
    baked_normals /= lengths

    textures = resolve_textures(build_texture_map(reference), hd_pack) if hd_pack else {}

    glb = GLB()
    _write_scene(glb, joints, animated, baked, baked_normals, colors, uvs,
                 triangles, groups, vertex_bones, scale, textures,
                 embed_textures, animations)
    glb.write(out_path)

    return {
        "joints": len(joints),
        "animated_joints": len(animated),
        "vertices": len(positions),
        "triangles": len(triangles),
        "groups": len(groups),
        "animations": len(animations or {}),
    }


def _write_scene(glb, joints, animated, positions, normals, colors, uvs,
                 triangles, groups, vertex_bones, scale, textures,
                 embed_textures, animations):
    # Joint index -> position within the skin's joint array (all joints).
    joint_nodes = []
    for joint in joints:
        node = {
            "name": joint.name,
            "translation": [float(v * scale) for v in joint.translation],
        }
        quat = euler_to_quaternion(*joint.rotation)
        if any(abs(q) > 1e-9 for q in quat[:3]):
            node["rotation"] = [float(q) for q in quat]
        if abs(joint.scale - 1.0) > 1e-9:
            node["scale"] = [joint.scale] * 3
        joint_nodes.append(node)

    for index, joint in enumerate(joints):
        if joint.parent >= 0:
            joint_nodes[joint.parent].setdefault("children", []).append(index)

    glb.json["nodes"] = joint_nodes

    # -- skin ---------------------------------------------------------------
    inverse_bind = []
    for joint in joints:
        matrix = joint.rest_global.copy()
        matrix[:3, 3] *= scale
        inverse = np.linalg.inv(matrix)
        # glTF matrices are column-major.
        inverse_bind.append(tuple(inverse.T.flatten()))

    ibm = glb.add_array(inverse_bind, FLOAT, "MAT4")

    # -- vertex attributes --------------------------------------------------
    position_accessor = glb.add_array(
        [tuple(p) for p in positions], FLOAT, "VEC3",
        target=ARRAY_BUFFER, with_bounds=True,
    )
    normal_accessor = glb.add_array(
        [tuple(n) for n in normals], FLOAT, "VEC3", target=ARRAY_BUFFER
    )
    uv_accessor = glb.add_array(
        [tuple(t) for t in uvs], FLOAT, "VEC2", target=ARRAY_BUFFER
    )
    color_accessor = glb.add_array(
        [(c[0] / 255.0, c[1] / 255.0, c[2] / 255.0, c[3] / 255.0) for c in colors],
        FLOAT, "VEC4", target=ARRAY_BUFFER,
    )
    joints_accessor = glb.add_array(
        [(int(b), 0, 0, 0) for b in vertex_bones],
        UNSIGNED_SHORT, "VEC4", target=ARRAY_BUFFER,
    )
    weights_accessor = glb.add_array(
        [(1.0, 0.0, 0.0, 0.0)] * len(vertex_bones),
        FLOAT, "VEC4", target=ARRAY_BUFFER,
    )

    attributes = {
        "POSITION": position_accessor,
        "NORMAL": normal_accessor,
        "TEXCOORD_0": uv_accessor,
        "COLOR_0": color_accessor,
        "JOINTS_0": joints_accessor,
        "WEIGHTS_0": weights_accessor,
    }

    # -- materials and primitives ------------------------------------------
    material_cache = {}
    primitives = []

    for group in groups:
        tris = triangles[group["first"]:group["first"] + group["count"]]
        if len(tris) == 0:
            continue

        indices = glb.add_array(
            [int(i) for i in tris.flatten()], UNSIGNED_INT, "SCALAR",
            target=ELEMENT_ARRAY_BUFFER,
        )
        material = _material_for(glb, group, textures, embed_textures,
                                 material_cache)
        primitive = {"attributes": attributes, "indices": indices}
        if material is not None:
            primitive["material"] = material
        primitives.append(primitive)

    glb.json["meshes"] = [{"name": "actor", "primitives": primitives}]
    glb.json["skins"] = [{
        "inverseBindMatrices": ibm,
        "joints": list(range(len(joints))),
        "skeleton": 0,
    }]

    # The skinned mesh has to be a *child* of the skeleton root, not a sibling.
    # Panda3D's glTF loader builds a Character from the joint hierarchy and
    # only pulls geometry into it that sits underneath; as a sibling the mesh
    # loads but never binds, so the Actor animates nothing.
    mesh_node = len(glb.json["nodes"])
    glb.json["nodes"].append({"name": "actor", "mesh": 0, "skin": 0})
    glb.json["nodes"][0].setdefault("children", []).append(mesh_node)
    glb.json["scenes"][0]["nodes"] = [0]

    # -- animations ---------------------------------------------------------
    if animations:
        for name, anim in sorted(animations.items()):
            _write_animation(glb, name, anim, joints, animated, scale)


_composite_cache = {}


def composite_over(image_path, rgb):
    """Flatten a texture onto a solid colour, as a BLEND combiner does.

    Returns PNG bytes with no alpha channel, or None if it cannot be built.
    The N64 lerps the texel over the shade colour inside the polygon, so the
    result is opaque -- baking that here is the only way to say the same thing
    in glTF, where baseColorFactor can only multiply.
    """
    key = (image_path, rgb)
    if key in _composite_cache:
        return _composite_cache[key]

    result = None
    try:
        import io

        from PIL import Image

        with Image.open(image_path) as source:
            texture = source.convert("RGBA")
        backdrop = Image.new("RGBA", texture.size, tuple(rgb) + (255,))
        flattened = Image.alpha_composite(backdrop, texture).convert("RGB")

        buffer = io.BytesIO()
        flattened.save(buffer, format="PNG", optimize=True)
        result = buffer.getvalue()
    except Exception:
        result = None

    _composite_cache[key] = result
    return result


_transparency_cache = {}


def has_transparency(image_path):
    """Whether an image actually contains see-through pixels."""
    if not image_path or not os.path.exists(image_path):
        return False
    if image_path in _transparency_cache:
        return _transparency_cache[image_path]

    result = False
    try:
        from PIL import Image

        with Image.open(image_path) as image:
            if "A" in image.getbands():
                result = image.getchannel("A").getextrema()[0] < 255
    except Exception:
        # Without PIL, fall back on the N64 format in the filename: the 16-bit
        # RGBA format carries one alpha bit, so assume those may be cut out.
        result = ".rgba16." in os.path.basename(image_path)

    _transparency_cache[image_path] = result
    return result


def _material_for(glb, group, textures, embed_textures, cache):
    symbol = group.get("texture")
    light = group.get("light")
    kind = group.get("combiner_kind", "modulate")
    diffuse = group.get("light_diffuse")
    image_path = group.get("image") or textures.get(symbol)

    # A BLEND group flattens its texture onto the light colour, so that pairing
    # identifies the baked image and belongs in the cache key.
    flattened = None
    if kind == "blend" and diffuse and image_path and os.path.exists(image_path):
        flattened = composite_over(image_path, diffuse)

    key = (symbol, group["layer"], group.get("wrap_s"), group.get("wrap_t"),
           light, kind, flattened is not None)
    if key in cache:
        return cache[key]

    # Solid-coloured parts carry their colour on the bound light rather than on
    # their vertices, so it becomes the material's base colour. Once a texture
    # has been flattened onto that colour, applying it again would double it.
    if diffuse and (kind != "blend" or flattened is None):
        base_color = [diffuse[0] / 255.0, diffuse[1] / 255.0,
                      diffuse[2] / 255.0, 1.0]
    else:
        base_color = [1.0, 1.0, 1.0, 1.0]

    material = {
        "name": light or symbol or group["layer"],
        "pbrMetallicRoughness": {
            "baseColorFactor": base_color,
            "metallicFactor": 0.0,
            "roughnessFactor": 1.0,
        },
        "doubleSided": not group.get("cull", True),
    }

    if group["layer"] in ("TRANSPARENT", "TRANSPARENT_DECAL"):
        material["alphaMode"] = "BLEND"
    elif flattened is None and kind != "shade" and has_transparency(image_path):
        # Genuinely cut-out alpha, as opposed to alpha blended onto a shade
        # colour. RGBA5551 carries a single alpha bit, so a mask is exact.
        material["alphaMode"] = "MASK"
        material["alphaCutoff"] = 0.5

    if embed_textures and image_path and os.path.exists(image_path):
        if flattened is not None:
            image = glb.add_image(flattened, name=f"{symbol}_on_{light}")
        else:
            with open(image_path, "rb") as fh:
                image = glb.add_image(fh.read(), name=symbol)

        wrap = {"wrap": 10497, "mirror": 33648, "clamp": 33071}
        glb.json["samplers"].append({
            "wrapS": wrap.get(group.get("wrap_s"), 10497),
            "wrapT": wrap.get(group.get("wrap_t"), 10497),
        })
        glb.json["textures"].append({
            "source": image,
            "sampler": len(glb.json["samplers"]) - 1,
        })
        material["pbrMetallicRoughness"]["baseColorTexture"] = {
            "index": len(glb.json["textures"]) - 1
        }

    glb.json["materials"].append(material)
    index = len(glb.json["materials"]) - 1
    cache[key] = index
    return index


def _write_animation(glb, name, anim, joints, animated, scale):
    frames = anim.frame_count
    times = [f / FRAME_RATE for f in range(frames)]
    time_accessor = glb.add_array(times, FLOAT, "SCALAR", with_bounds=True)

    samples = [anim.sample(f, len(animated)) for f in range(frames)]

    channels = []
    samplers = []

    # Root translation rides on the first animated joint.
    root_node = animated[0]
    translations = []
    for translation, _ in samples:
        base = joints[root_node].translation
        translations.append(tuple(
            float((base[i] + translation[i]) * scale) for i in range(3)
        ))
    samplers.append({
        "input": time_accessor,
        "output": glb.add_array(translations, FLOAT, "VEC3"),
        "interpolation": "LINEAR",
    })
    channels.append({
        "sampler": 0,
        "target": {"node": root_node, "path": "translation"},
    })

    for slot, node_index in enumerate(animated):
        rotations = []
        for _, joint_rotations in samples:
            rotations.append(tuple(
                float(q) for q in euler_to_quaternion(*joint_rotations[slot])
            ))
        samplers.append({
            "input": time_accessor,
            "output": glb.add_array(rotations, FLOAT, "VEC4"),
            "interpolation": "LINEAR",
        })
        channels.append({
            "sampler": len(samplers) - 1,
            "target": {"node": node_index, "path": "rotation"},
        })

    glb.json["animations"].append({
        "name": name, "samplers": samplers, "channels": channels,
    })


def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    default_reference = os.path.abspath(os.path.join(here, "..", "reference",
                                                     "Render96ex"))
    default_hd = os.path.abspath(os.path.join(here, "..", "reference",
                                              "RENDER96-HD-TEXTURE-PACK"))

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--actor", default="mario")
    parser.add_argument("--reference", default=default_reference)
    parser.add_argument("--hd-pack", default=default_hd)
    parser.add_argument("-o", "--out", default="assets/mario/mario.glb")
    parser.add_argument("--root-layout", default=None,
                        help="geo layout to export (default <actor>_geo_body)")
    parser.add_argument("--switch-case", type=int, default=0,
                        help="which case to keep at each GEO_SWITCH_CASE")
    parser.add_argument("--scale", type=float, default=None,
                        help="output scale (default matches game units); "
                             "use 0.0025 for a ~1.5m tall Mario in metres")
    parser.add_argument("--anims", default="none",
                        help="'none', 'all', or a comma-separated list")
    parser.add_argument("--exclude-dl", default=None,
                        help="regex of display lists to skip; overrides the "
                             "per-actor default. Pass '' to keep everything, "
                             "which for Mario means the wing-cap wings.")
    parser.add_argument("--no-textures", action="store_true")
    args = parser.parse_args(argv[1:])

    actor_dir = os.path.join(args.reference, "actors", args.actor)
    root_layout = args.root_layout or f"{args.actor}_geo_body"

    # Mario's animations live in a shared assets/anims directory; every other
    # actor keeps its own beside its model. Using the shared one for them does
    # not fail cleanly -- the tables are read positionally, so Mario's 20-joint
    # animations get applied to whatever hierarchy the actor has and either
    # warn about the joint count or run off the end of the index table.
    actor_anims = os.path.join(actor_dir, "anims")
    anim_dir = (actor_anims if os.path.isdir(actor_anims)
                else os.path.join(args.reference, "assets", "anims"))

    animations = {}
    if args.anims != "none" and os.path.isdir(anim_dir):
        animations = sm64_anim.load_animations(anim_dir)
        if args.anims != "all":
            wanted = {a.strip() for a in args.anims.split(",")}
            animations = {k: v for k, v in animations.items() if k in wanted}

    names = MARIO_JOINT_NAMES if args.actor == "mario" else None
    hd_pack = None if args.no_textures else (
        args.hd_pack if os.path.isdir(args.hd_pack) else None
    )
    scale = args.scale if args.scale is not None else ACTOR_SCALE.get(
        args.actor, DEFAULT_SCALE
    )
    pattern = (args.exclude_dl if args.exclude_dl is not None
               else DEFAULT_EXCLUDE.get(args.actor))
    exclude = re.compile(pattern) if pattern else None

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)

    # Playback metadata the glTF itself has nowhere to put. The start frame
    # matters: several clips are authored with lead-in frames the game never
    # shows, and playing from zero drops Mario through the floor mid-landing.
    #
    # The decomp header's frame count is not the count the game can play.
    # _write_animation lays the poses out at f / FRAME_RATE, putting the last
    # one at (frames - 1) / FRAME_RATE, and Panda3D reads that as the clip's
    # duration and builds a table one frame shorter (rig.panda_frame_count).
    # Everything the header says is clamped into that, or the game addresses
    # frames the AnimBundle does not have. The last time goes through float32
    # first, because that is the width it is stored at and the ceil turns on
    # its last bit.
    if animations:
        clips = {}
        for name, anim in animations.items():
            last = float(np.float32((anim.frame_count - 1) / FRAME_RATE))
            playable = rig.panda_frame_count(last)
            clips[name] = {
                "frames": playable,
                "start_frame": min(anim.start_frame, max(playable - 1, 0)),
                "loop_start": min(anim.loop_start, max(playable - 1, 0)),
                "loop_end": min(anim.loop_end or playable, playable),
            }
        sidecar = os.path.splitext(args.out)[0] + "_clips.json"
        with open(sidecar, "w", encoding="utf-8") as fh:
            json.dump(clips, fh, indent=2, sort_keys=True)

    stats = export(
        actor_dir=actor_dir, anim_dir=anim_dir, reference=args.reference,
        hd_pack=hd_pack, out_path=args.out, root_layout=root_layout,
        switch_case=args.switch_case, scale=scale,
        animations=animations, joint_names=names,
        embed_textures=not args.no_textures, exclude=exclude,
    )

    size = os.path.getsize(args.out)
    print(f"{stats['joints']} joints ({stats['animated_joints']} animated), "
          f"{stats['vertices']} vertices, {stats['triangles']} triangles, "
          f"{stats['groups']} material groups")
    print(f"{stats['animations']} animations -> {args.out} ({size / 1024:.0f} KB)")

    if animations:
        counts = {a.num_parts for a in animations.values()}
        if counts != {stats["animated_joints"]}:
            print(f"  WARNING: animation tables expect {sorted(counts)} joints "
                  f"but the hierarchy produced {stats['animated_joints']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
