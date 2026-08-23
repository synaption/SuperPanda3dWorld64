"""Tile .npz files -> a Blender scene and .glb. The only module that imports bpy.

    blender --background --factory-startup --python blender/export_tiles.py

Writes out/planet.blend, plus out/planet.glb (LOD0, what the player walks on)
and out/planet_lod1.glb (LOD1, the space sphere). glTF conventions follow
tools/blend_to_glb.py, which is the project's generic "just give me a glb" path.

One collection per tile, named by tile id, so a re-export replaces a tile
rather than accumulating copies. Tiles stay separate objects: working on them
one at a time is the entire point.
"""

import json
import sys
from pathlib import Path

import bpy
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from planetgen import manifest, surface  # noqa: E402


def clear():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.collections):
        for item in list(block):
            block.remove(item)


def ground_material():
    """One material for the whole planet, coloured by the tiles' own attribute.

    Material identity travels in a per-vertex colour rather than in slots, so a
    tile is one mesh with one material no matter how many biomes cross it, and
    repainting faces/material_N.png changes the look without touching topology.
    """
    mat = bpy.data.materials.new("PlanetGround")
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = tree.nodes["Principled BSDF"]
    bsdf.inputs["Roughness"].default_value = 0.92
    # ShaderNodeVertexColor ("Color Attribute"), not the generic Attribute
    # node: the glTF exporter decides whether a mesh's colours are used by
    # walking the shader graph, and it only recognizes this node. With the
    # generic one the .blend renders correctly and the .glb comes out untinted,
    # which is the same silent-white failure mode blend_to_glb.py documents.
    attr = tree.nodes.new("ShaderNodeVertexColor")
    attr.layer_name = "material_color"
    attr.location = (-320, 0)
    tree.links.new(attr.outputs["Color"], bsdf.inputs["Base Color"])

    # The exporter follows only an output targeted ALL or CYCLES; an EEVEE-only
    # one yields glTF's default white metallic material with no warning.
    for node in tree.nodes:
        if node.type == "OUTPUT_MATERIAL":
            node.target = "ALL"
    return mat


def build_tile(data, mat, parent):
    face, depth, tu, tv = (int(x) for x in data["tile"])
    name = f"tile_{face}_{depth}_{tu}_{tv}"
    positions = data["positions"].astype(np.float64)
    triangles = data["triangles"].astype(np.int64)

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(positions.tolist(), [], triangles.tolist())
    mesh.update()

    palette = np.array([m["color"] for m in surface.MATERIALS], dtype=np.float64)
    rgb = palette[data["material"]]
    rgba = np.concatenate([rgb, np.ones((len(rgb), 1))], axis=1)
    layer = mesh.color_attributes.new("material_color", "FLOAT_COLOR", "POINT")
    layer.data.foreach_set("color", rgba.ravel())

    mesh.materials.append(mat)
    obj = bpy.data.objects.new(name, mesh)

    # The generator's normals are accumulated planet-wide, so a boundary vertex
    # already sees the triangles on the far side of the seam. Handing them to
    # Blender keeps that; letting Blender recompute per object would put a
    # lighting seam back on geometry that is exact.
    try:
        mesh.normals_split_custom_set_from_vertices(
            data["normals"].astype(np.float64).tolist())
    except Exception as exc:                       # older/newer API drift
        print(f"  custom normals unavailable ({exc}); using smooth shading")
    for poly in mesh.polygons:
        poly.use_smooth = True

    obj["tile"] = [face, depth, tu, tv]
    obj["walkable_fraction"] = float(np.mean(data["walkable"]))
    parent.objects.link(obj)
    return obj


def export_lod(lod, mat):
    tiles = sorted((ROOT / "tiles" / f"lod{lod}").glob("*.npz"))
    collection = bpy.data.collections.new(f"LOD{lod}")
    bpy.context.scene.collection.children.link(collection)
    for path in tiles:
        with np.load(path) as z:
            build_tile({k: z[k] for k in z.files}, mat, collection)
    return collection, len(tiles)


def write_glb(path, collection):
    """Export one LOD collection to .glb, selection-scoped.

    Hiding is applied after this runs: a hidden collection's objects cannot be
    selected, and selection is how the two LODs are kept in separate files.
    """
    import addon_utils
    addon_utils.enable("io_scene_gltf2", default_set=False, persistent=False)

    bpy.ops.object.select_all(action="DESELECT")
    for obj in collection.objects:
        obj.select_set(True)

    options = dict(
        filepath=str(path),
        export_format="GLB",
        use_selection=True,
        # glTF is Y-up, Blender is Z-up. Every consumer expects the conversion
        # to have happened -- same reasoning as tools/blend_to_glb.py.
        export_yup=True,
        export_normals=True,
        export_animations=False,
        export_apply=False,
    )
    properties = bpy.ops.export_scene.gltf.get_rna_type().properties
    if "export_vertex_color" in properties:
        options["export_vertex_color"] = "MATERIAL"
    bpy.ops.export_scene.gltf(**options)
    bpy.ops.object.select_all(action="DESELECT")

    if not path.is_file():
        raise SystemExit(f"exporter reported success but wrote no {path}")
    return path.stat().st_size


def main():
    m = manifest.load(ROOT)
    clear()
    mat = ground_material()
    # LOD1 is the one you open the file to; LOD0 is 96 objects and is there to
    # be worked on a tile at a time, not looked at all at once.
    lod0, n0 = export_lod(0, mat)
    lod1, n1 = export_lod(1, mat)

    scene = bpy.context.scene
    scene.name = "Planet"
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    scene["generator"] = "experimental/planet_gen"
    scene["planet"] = json.dumps({k: v for k, v in m.items() if k != "materials"})

    out_dir = ROOT / "out"
    out_dir.mkdir(parents=True, exist_ok=True)

    for name, collection in (("planet.glb", lod0), ("planet_lod1.glb", lod1)):
        size = write_glb(out_dir / name, collection)
        print(f"wrote {out_dir / name}: {size / 1e6:.1f} MB")

    for collection, hide in ((lod0, True), (lod1, False)):
        collection.hide_viewport = hide
        collection.hide_render = hide

    out = out_dir / "planet.blend"
    bpy.ops.wm.save_as_mainfile(filepath=str(out), compress=True)
    tris = sum(len(o.data.polygons) for o in lod0.objects)
    print(f"wrote {out}: LOD0 {n0} tiles / {tris:,} triangles, LOD1 {n1} tiles")


if __name__ == "__main__":
    main()
