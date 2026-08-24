#!/usr/bin/env python3
"""Convert the committed SM64 NPZ level into Bevy's tiny native blob.

Run from anywhere. The generated file is committed so playing the Bevy port
does not require Python or NumPy.
"""
from pathlib import Path
import json
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
SCALE = 0.01
sys.path.insert(0, str(ROOT))
from tools.glb import GLB, FLOAT, UNSIGNED_INT, ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER


def write_vec(out, values, code):
    values = np.asarray(values)
    out.extend(struct.pack("<I", len(values)))
    out.extend(values.astype(code).tobytes(order="C"))


def main():
    mesh = np.load(ROOT / "assets/castle_grounds/mesh.npz")
    collision = np.load(ROOT / "assets/castle_grounds/collision.npz")
    positions = mesh["positions"].astype(np.float32) * SCALE
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
    coll_vertices = collision["vertices"].astype(np.float32) * SCALE
    coll_triangles = collision["tri_verts"].astype(np.uint32)
    # The decomp's water boxes are deliberately not read. They are furniture
    # now -- two planes in `assets/levels/castle.blend`, exported by
    # `tools/export_level_furniture.py` -- so that moving the moat is
    # something you do by dragging it. This file is the level's geometry, and
    # geometry is the part nobody authors.

    objects = json.loads((ROOT / "assets/castle_grounds/collision_objects.json").read_text())
    trees = [o["pos"] for o in objects if o["preset"] == "special_bubble_tree"]
    trees = np.asarray(trees, dtype=np.float32) * SCALE

    out = bytearray(b"SBW1")
    for values, code in (
        (positions, "<f4"), (normals, "<f4"), (uvs, "<f4"),
        (colors, "<f4"), (triangles, "<u4"),
        (coll_vertices, "<f4"), (coll_triangles, "<u4"), (trees, "<f4"),
    ):
        write_vec(out, values, code)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(out)
    print(f"wrote {OUT} ({len(out):,} bytes, {len(trees)} trees)")
    write_render_glb(mesh)
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


def write_render_glb(mesh):
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
        positions = mesh["positions"][used].astype(np.float32) * SCALE
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
