#!/usr/bin/env python3
"""Convert the committed SM64 NPZ level into Bevy's tiny native blob.

Run from anywhere. The generated file is committed so playing the Bevy port
does not require Python or NumPy.

**Collision comes out of Blender.** It is the castle mesh in
``assets/bevy/castle_grounds.blend``, in its own world space -- one Blender
metre is one game metre, so nothing scales it on the way through. It used to be
``assets/castle_grounds/collision.npz``, the decomp's separate 879-triangle
collision hull, and the level therefore shipped two meshes that nobody could
compare: the one you saw and the one you walked on. They disagreed, and every
"I fell through the floor" and "it is standing inside the wall" had that as a
candidate cause with no way to rule it out. Now there is one mesh, so the
question cannot be asked.

What is lost with the hull is what a hull is for: the decomp's invisible walls,
its death planes and its floor types (``tri_type``/``tri_force`` in the NPZ,
which nothing here has ever read). A castle collided with as it is drawn has
none of those, and things you could once walk through -- railings, the castle
facade -- now stop you. That is the intended trade.

The render mesh still comes from the NPZ, because exporting the blend loses
``KHR_materials_unlit`` and gains ``alphaMode: BLEND`` on all 45 materials --
see ``build_assets.build_castle``. The two are the same geometry today, and
`report_cover` below says so out loud on every run, because the day they stop
being the same is the day the disagreement comes back.
"""
import argparse
from pathlib import Path
import json
import math
import os
import subprocess
import struct
import sys
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
# All three outputs live together under assets/bevy/. castle.bin is the only
# one the crate reads at build time -- level.rs embeds it with include_bytes!
# -- while the other two are loaded from disk at runtime.
OUT = ROOT / "assets" / "bevy" / "castle.bin"
GLB_OUT = ROOT / "assets" / "bevy" / "castle.glb"
# The water sheet's texture. Water is not part of the level mesh, so it is not
# in the GLB above and is copied out of the reference pack on its own.
WATER_TEXTURE = ROOT / (
    "reference/RENDER96-HD-TEXTURE-PACK/gfx/textures/segment2/"
    "segment2.11C58.rgba16.png")
WATER_OUT = ROOT / "assets" / "bevy" / "water.png"
CASTLE_BLEND = ROOT / "assets" / "bevy" / "castle_grounds.blend"
CASTLE_ROOT = "CastleGrounds"
BOUNDS_MARKER = "CASTLE_GROUNDS_EXTENTS="
GEOMETRY_MARKER = "CASTLE_GROUNDS_GEOMETRY="
sys.path.insert(0, str(ROOT))
from tools.glb import GLB, FLOAT, UNSIGNED_INT, ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER
from tools.blend_to_glb import resolve_blender


def uniform_scale(values):
    """Validate equivalent axis ratios and return their single value."""
    try:
        values = tuple(float(value) for value in values)
    except (TypeError, ValueError) as error:
        raise ValueError(f"invalid castle axis ratios: {values!r}") from error
    if len(values) != 3 or not all(math.isfinite(value) for value in values):
        raise ValueError(f"invalid castle axis ratios: {values!r}")
    if any(value <= 0.0 for value in values):
        raise ValueError(f"castle dimensions must be positive, got {values!r}")
    if not all(math.isclose(value, values[0], rel_tol=1e-6, abs_tol=1e-6)
               for value in values[1:]):
        raise ValueError(
            "the Blender castle must retain the source proportions so render "
            f"geometry and collision stay aligned, got axis ratios {values!r}")
    return values[0]


def world_scale(source_extents, blender_extents):
    """Map raw source units to the castle's authored Blender-metre bounds."""
    try:
        source = sorted(float(value) for value in source_extents)
        authored = sorted(float(value) for value in blender_extents)
    except (TypeError, ValueError) as error:
        raise ValueError("castle extents must be three numeric values") from error
    if len(source) != 3 or len(authored) != 3:
        raise ValueError("castle extents must have three axes")
    if any(not math.isfinite(value) or value <= 0.0
           for value in source + authored):
        raise ValueError(
            f"castle extents must be finite and positive: "
            f"source={source!r}, Blender={authored!r}")
    return uniform_scale([want / raw for raw, want in zip(source, authored)])


def blender_castle(explicit_blender=None, dump=None):
    """Read the authored castle out of Blender: its size and its triangles.

    One launch for both, because they are one question -- what is in the blend
    -- and because Blender takes seconds to start. `extents` is still measured
    off the object bounding boxes, exactly as when this only measured, so the
    render mesh's scale is unchanged to the last bit by collision arriving
    beside it.

    The vertices come back in *game* axes and in metres. Blender is Z-up and
    the game is Y-up, so the swap happens here rather than being left for a
    caller to remember: `(x, y, z)` in Blender is `(x, z, -y)` in the game,
    which is the same convention glTF's `+Y up` export uses.

    Evaluated meshes rather than the authored ones, so a wall built with a
    mirror or an array modifier is collided with as it is drawn rather than as
    it is stored.
    """
    if not CASTLE_BLEND.is_file():
        raise SystemExit(f"missing Blender level source: {CASTLE_BLEND}")
    blender, version = resolve_blender(explicit_blender)
    # The geometry goes to a file rather than through the marker line: it is a
    # megabyte of JSON, and Blender's stdout is shared with whatever an add-on
    # decides to print.
    dump = Path(dump) if dump else CASTLE_BLEND.with_suffix(".geometry.json")
    expression = "\n".join((
        "import bpy, json, os, sys",
        "from mathutils import Vector",
        f"root = bpy.data.objects.get({CASTLE_ROOT!r})",
        "if root is None:",
        f"    print({('missing object ' + CASTLE_ROOT)!r}, "
        "file=sys.stderr, flush=True)",
        "    os._exit(1)",
        ("meshes = [obj for obj in root.children_recursive "
         "if obj.type == 'MESH']"),
        "if not meshes:",
        f"    print({'no castle meshes beneath ' + CASTLE_ROOT!r}, "
        "file=sys.stderr, flush=True)",
        "    os._exit(1)",
        ("corners = [obj.matrix_world @ Vector(corner) "
         "for obj in meshes for corner in obj.bound_box]"),
        "low = [min(point[axis] for point in corners) for axis in range(3)]",
        "high = [max(point[axis] for point in corners) for axis in range(3)]",
        "extents = [high[axis] - low[axis] for axis in range(3)]",
        "depsgraph = bpy.context.evaluated_depsgraph_get()",
        "vertices, triangles = [], []",
        "for obj in meshes:",
        "    evaluated = obj.evaluated_get(depsgraph)",
        "    mesh = evaluated.to_mesh()",
        "    mesh.calc_loop_triangles()",
        "    base = len(vertices)",
        "    matrix = obj.matrix_world",
        "    for vertex in mesh.vertices:",
        "        point = matrix @ vertex.co",
        "        vertices.append([point.x, point.z, -point.y])",
        "    for tri in mesh.loop_triangles:",
        "        triangles.append([base + i for i in tri.vertices])",
        "    evaluated.to_mesh_clear()",
        (f"open({str(dump)!r}, 'w').write(json.dumps("
         "{'vertices': vertices, 'triangles': triangles}))"),
        f"print({BOUNDS_MARKER!r} + json.dumps(extents), flush=True)",
        f"print({GEOMETRY_MARKER!r} + json.dumps("
        "[len(vertices), len(triangles)]), flush=True)",
        # A saved BlenderMCP handler can leave a server thread alive even in
        # background/factory mode. This process has only read the file, so a
        # direct exit avoids waiting forever for unrelated add-on threads.
        "os._exit(0)",
    ))
    command = [
        blender,
        "--background",
        "-noaudio",
        "--factory-startup",
        str(CASTLE_BLEND),
        "--python-exit-code", "1",
        "--python-expr", expression,
    ]
    try:
        result = subprocess.run(command, capture_output=True, text=True,
                                timeout=60)
    except subprocess.TimeoutExpired as error:
        raise SystemExit(
            f"timed out measuring {CASTLE_ROOT} in {CASTLE_BLEND}") \
            from error
    chatter = (result.stdout or "") + (result.stderr or "")
    if result.returncode != 0:
        sys.stderr.write(chatter)
        raise SystemExit(
            f"could not read the castle with Blender "
            f"{'.'.join(map(str, version or ['unknown']))}")
    marked = [line[len(BOUNDS_MARKER):] for line in chatter.splitlines()
              if line.startswith(BOUNDS_MARKER)]
    if len(marked) != 1:
        raise SystemExit(
            f"Blender did not report exactly one {CASTLE_ROOT} extent")
    try:
        extents = tuple(float(value) for value in json.loads(marked[0]))
    except (TypeError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid Blender castle extents: {marked[0]!r}") from error
    if len(extents) != 3:
        raise SystemExit(f"invalid Blender castle extents: {extents!r}")
    if not dump.is_file():
        raise SystemExit(f"Blender wrote no castle geometry to {dump}")
    try:
        geometry = json.loads(dump.read_text())
        vertices = np.asarray(geometry["vertices"], dtype=np.float32)
        triangles = np.asarray(geometry["triangles"], dtype=np.uint32)
    except (KeyError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid castle geometry in {dump}") from error
    finally:
        dump.unlink(missing_ok=True)
    if not len(triangles):
        raise SystemExit(f"no triangles beneath {CASTLE_ROOT} in {CASTLE_BLEND}")
    return extents, vertices, triangles


def drop_degenerate(vertices, triangles):
    """Throw away the triangles with no area to speak of.

    A hull authored by hand had none; a mesh authored to be *looked at* has
    them by the dozen, and one of them is an invisible floor in the middle of
    the world -- `level::degenerate` on the Rust side has the whole story for
    the planet, and this is the same hazard reaching the castle for the first
    time. Cheaper to drop them here, once, than to have every query in the game
    step over them for ever.
    """
    corners = vertices[triangles.astype(np.int64)]
    normals = np.cross(corners[:, 1] - corners[:, 0], corners[:, 2] - corners[:, 0])
    keep = np.linalg.norm(normals, axis=1) > 1e-6
    if not keep.all():
        print(f"dropped {int((~keep).sum())} degenerate collision triangles")
    return triangles[keep]


def report_cover(collision_vertices, collision_triangles, positions, triangles):
    """Say how far the collision and the render mesh have drifted apart.

    They are the same mesh today: the blend was built from `castle.glb`, which
    is built from the same NPZ these positions come from. Nothing enforces
    that, though -- the blend is the one a person edits, and the render mesh is
    regenerated from the decomp -- so the moment somebody moves a wall in
    Blender the two part company, and the game goes back to drawing one castle
    and colliding with another.

    A warning rather than a failure, because parting company is sometimes the
    point: the collision is the authored one and a blend edit is meant to take
    effect. But it is said out loud, with numbers, so it is never a surprise.
    Turn on `collide_debug 2` in the game to see which faces.
    """
    def faces(vertices, indices):
        return {tuple(sorted(tuple(point) for point in np.round(vertices[tri], 3)))
                for tri in indices.astype(np.int64)}
    collided, drawn = faces(collision_vertices, collision_triangles), \
        faces(positions, triangles)
    only_drawn, only_collided = len(drawn - collided), len(collided - drawn)
    if not (only_drawn or only_collided):
        print(f"collision matches the render mesh exactly ({len(drawn)} triangles)")
        return
    print(f"WARNING: the blend's collision and the render mesh disagree -- "
          f"{only_drawn} face(s) are drawn with nothing to stand on, "
          f"{only_collided} are collided with and never drawn")


def write_vec(out, values, code):
    values = np.asarray(values)
    out.extend(struct.pack("<I", len(values)))
    out.extend(values.astype(code).tobytes(order="C"))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--blender", default=os.environ.get("BLENDER"),
                        help="Blender executable used to read castle scale")
    args = parser.parse_args(argv)

    mesh = np.load(ROOT / "assets/castle_grounds/mesh.npz")
    source_extents = np.ptp(mesh["positions"].astype(np.float64), axis=0)
    authored_extents, coll_vertices, coll_triangles = blender_castle(args.blender)
    try:
        scale = world_scale(source_extents, authored_extents)
    except ValueError as error:
        raise SystemExit(str(error)) from error
    print("castle dimensions in Blender: "
          + " x ".join(f"{value:g} m" for value in authored_extents))
    print(f"derived source-unit conversion: {scale:g} m")
    coll_triangles = drop_degenerate(coll_vertices, coll_triangles)
    print(f"collision from {CASTLE_BLEND.name}: "
          f"{len(coll_vertices):,} vertices, {len(coll_triangles):,} triangles")
    positions = mesh["positions"].astype(np.float32) * scale
    triangles = mesh["triangles"].astype(np.uint32)

    normals = np.zeros_like(positions)
    for tri in triangles:
        a, b, c = positions[tri]
        n = np.cross(b - a, c - a)
        length = np.linalg.norm(n)
        if length:
            n /= length
        normals[tri] += n
    lengths = np.linalg.norm(normals, axis=1)
    normals /= np.where(lengths == 0, 1, lengths)[:, None]

    colors = mesh["colors"].astype(np.float32) / 255.0
    # The source UV convention is vertically opposite glTF/Bevy.
    uvs = mesh["uvs"].astype(np.float32)
    uvs[:, 1] = 1.0 - uvs[:, 1]
    # The water is not in here either. It is furniture -- two planes in
    # `assets/levels/castle.blend`, exported by
    # `tools/export_level_furniture.py` -- so that moving the moat is
    # something you do by dragging it, which is now what moving anything is.
    report_cover(coll_vertices, coll_triangles, positions, triangles)

    out = bytearray(b"SBW1")
    for values, code in (
        (positions, "<f4"), (normals, "<f4"), (uvs, "<f4"),
        (colors, "<f4"), (triangles, "<u4"),
        (coll_vertices, "<f4"), (coll_triangles, "<u4"),
    ):
        write_vec(out, values, code)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(out)
    print(f"wrote {OUT} ({len(out):,} bytes)")
    write_render_glb(mesh, scale)
    write_water_texture()


def write_water_texture():
    """Copy the water texture beside the converted level.

    Skipped rather than fatal when the reference pack is absent: the committed
    copy is what the game loads, and a checkout without the large reference
    sources can still regenerate everything else.
    """
    if not WATER_TEXTURE.exists():
        print(f"skipped {WATER_OUT} (no {WATER_TEXTURE.name} in reference/)")
        return
    WATER_OUT.parent.mkdir(parents=True, exist_ok=True)
    WATER_OUT.write_bytes(WATER_TEXTURE.read_bytes())
    print(f"wrote {WATER_OUT} ({WATER_OUT.stat().st_size:,} bytes)")


def n64_shade_colors(raw_colors, group):
    """Evaluate the original RSP's one-light vertex-lighting equation.

    With G_LIGHTING enabled, the vertex colour bytes are signed normals. SM64
    adds the material's ambient colour to the positive normal/light dot product
    times its diffuse colour, clamps to a byte, then interpolates that result
    across the triangle. Baking those bytes into COLOR_0 and drawing unlit is
    materially closer than asking Bevy's fragment PBR pipeline to reinterpret
    the same data.
    """
    ambient = np.asarray(group.get("light_ambient") or (255, 255, 255),
                         dtype=np.float32)
    diffuse = np.asarray(group.get("light_diffuse") or (0, 0, 0),
                         dtype=np.float32)
    direction = np.asarray(group.get("light_direction") or (40, 40, 40),
                           dtype=np.float32)
    direction_length = np.linalg.norm(direction)
    if direction_length:
        direction /= direction_length

    normals = raw_colors[:, :3].astype(np.int16)
    normals[normals > 127] -= 256
    intensity = np.maximum((normals.astype(np.float32) @ direction) / 127.0, 0.0)
    rgb = np.clip(ambient + intensity[:, None] * diffuse, 0.0, 255.0)
    colors = np.empty((len(raw_colors), 4), dtype=np.float32)
    colors[:, :3] = rgb / 255.0
    colors[:, 3] = raw_colors[:, 3] / 255.0
    return colors


def write_render_glb(mesh, scale):
    GLB_OUT.parent.mkdir(parents=True, exist_ok=True)
    groups = json.loads((ROOT / "assets/castle_grounds/mesh_materials.json").read_text())
    glb = GLB()
    glb.json["extensionsUsed"] = ["KHR_materials_unlit"]
    image_cache = {}

    for number, group in enumerate(groups):
        first, count = group["first"], group["count"]
        if not count:
            continue
        source_tris = mesh["triangles"][first:first + count]
        used = np.unique(source_tris)
        remap = np.zeros(len(mesh["positions"]), dtype=np.uint32)
        remap[used] = np.arange(len(used), dtype=np.uint32)
        positions = mesh["positions"][used].astype(np.float32) * scale
        uvs = mesh["uvs"][used].astype(np.float32)
        uvs[:, 1] = 1.0 - uvs[:, 1]

        normals = np.zeros_like(positions)
        for tri in remap[source_tris]:
            a, b, c = positions[tri]
            n = np.cross(b - a, c - a)
            length = np.linalg.norm(n)
            if length:
                n /= length
            normals[tri] += n
        lengths = np.linalg.norm(normals, axis=1)
        normals /= np.where(lengths == 0, 1, lengths)[:, None]

        raw_colors = mesh["colors"][used]
        if group.get("lighting"):
            colors = n64_shade_colors(raw_colors, group)
        else:
            colors = raw_colors.astype(np.float32) / 255.0

        attrs = {
            "POSITION": glb.add_array(positions.tolist(), FLOAT, "VEC3", ARRAY_BUFFER, True),
            "NORMAL": glb.add_array(normals.tolist(), FLOAT, "VEC3", ARRAY_BUFFER),
            "TEXCOORD_0": glb.add_array(uvs.tolist(), FLOAT, "VEC2", ARRAY_BUFFER),
            "COLOR_0": glb.add_array(colors.tolist(), FLOAT, "VEC4", ARRAY_BUFFER),
        }
        indices = glb.add_array(remap[source_tris].reshape(-1).tolist(), UNSIGNED_INT,
                                "SCALAR", ELEMENT_ARRAY_BUFFER)

        material = {
            "name": group.get("texture") or f"shade_{number}",
            "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 1.0},
            "doubleSided": not group.get("cull", True),
        }
        image_path = group.get("image")
        if image_path:
            image_path = ROOT / image_path
            key = str(image_path)
            if key not in image_cache:
                image = glb.add_image(image_path.read_bytes(), image_path.stem)
                sampler = len(glb.json["samplers"])
                glb.json["samplers"].append({
                    "magFilter": 9729, "minFilter": 9987,
                    "wrapS": 10497 if group.get("wrap_s") == "wrap" else 33071,
                    "wrapT": 10497 if group.get("wrap_t") == "wrap" else 33071,
                })
                texture = len(glb.json["textures"])
                glb.json["textures"].append({"source": image, "sampler": sampler})
                image_cache[key] = texture
            material["pbrMetallicRoughness"]["baseColorTexture"] = {
                "index": image_cache[key]
            }
        # Both paths already contain their final N64 shade colour: either the
        # source vertex colour, or the RSP lighting result baked above.
        material["extensions"] = {"KHR_materials_unlit": {}}
        # SM64's ALPHA layer is alpha *tested*, not blended: the fence and the
        # castle doorway are cutouts with binary alpha. Blending them puts
        # them in the renderer's transparent queue, where they are sorted
        # per object against the water sheet behind them and flicker as the
        # camera moves. Masked geometry draws in the opaque pass and writes
        # depth, so the water sorts behind it correctly and stays put.
        if group.get("layer") == "ALPHA":
            material["alphaMode"] = "MASK"
            material["alphaCutoff"] = 0.5
        elif group.get("layer") == "TRANSPARENT_DECAL":
            material["alphaMode"] = "BLEND"

        material_index = len(glb.json["materials"])
        glb.json["materials"].append(material)
        mesh_index = len(glb.json["meshes"])
        glb.json["meshes"].append({
            "name": material["name"],
            "primitives": [{"attributes": attrs, "indices": indices,
                            "material": material_index}],
        })
        node = len(glb.json["nodes"])
        glb.json["nodes"].append({"name": material["name"], "mesh": mesh_index})
        glb.json["scenes"][0]["nodes"].append(node)

    glb.write(GLB_OUT)
    print(f"wrote {GLB_OUT} ({GLB_OUT.stat().st_size:,} bytes, {len(groups)} materials)")


if __name__ == "__main__":
    main()
