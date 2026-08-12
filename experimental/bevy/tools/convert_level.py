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

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parents[1] / "assets" / "castle.bin"
GLB_OUT = ROOT / "assets" / "bevy" / "castle.glb"
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
            colors = np.ones((len(used), 4), dtype=np.float32)
            colors[:, 3] = raw_colors[:, 3] / 255.0
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
        if not group.get("lighting"):
            material["extensions"] = {"KHR_materials_unlit": {}}
        if group.get("layer") in ("ALPHA", "TRANSPARENT_DECAL"):
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
