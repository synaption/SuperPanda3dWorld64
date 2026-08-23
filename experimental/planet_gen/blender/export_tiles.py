"""Tile .npz files -> a Blender scene and .glb. The only module that imports bpy.

    blender --background --factory-startup --python blender/export_tiles.py

Writes out/planet.blend, plus out/planet.glb (LOD0, what the player walks on)
and out/planet_lod1.glb (LOD1, the space sphere). glTF conventions follow
tools/blend_to_glb.py, which is the project's generic "just give me a glb" path.

One collection per tile, named by tile id, so a re-export replaces a tile
rather than accumulating copies. Tiles stay separate objects: working on them
one at a time is the entire point.

Each LOD also carries the sea: one transparent sphere at sea level, in a node
named `ocean`. It rides in the same file as the terrain because it is the same
planet, and the game tells the two apart by that name -- see `src/world.rs`.
"""

import json
import sys
from pathlib import Path

import bpy
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
TEXTURES = ROOT.parent.parent / "assets" / "planet_gen" / "textures"

# The sea uses the castle's own water sheet rather than a sixth Render96
# terrain image, so the two bodies of water in this game look like the same
# substance. It is a committed asset like the rest; nothing here reaches into
# the untracked HD pack.
WATER_TEXTURE = ROOT.parent.parent / "assets" / "bevy" / "water.png"

sys.path.insert(0, str(ROOT))

from planetgen import manifest, ocean, surface  # noqa: E402

#: How much of the seabed shows through. The castle's sheet is 0x96 of 0xFF,
#: and this is the same water.
WATER_ALPHA = 0x96 / 0xFF


def clear():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.collections):
        for item in list(block):
            block.remove(item)


def ground_material(spec):
    """A Render96 terrain material using the mesh's dominant-axis UVs."""
    path = TEXTURES / spec["texture"]
    if not path.is_file():
        raise SystemExit(f"missing PlanetGen terrain texture: {path}")

    mat = bpy.data.materials.new(f"PlanetGround_{spec['name']}")
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = tree.nodes["Principled BSDF"]
    bsdf.inputs["Roughness"].default_value = 0.92
    bsdf.inputs["Metallic"].default_value = 0.0

    image = bpy.data.images.load(str(path), check_existing=True)
    image.pack()
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = image
    tex.interpolation = "Linear"
    tex.extension = "REPEAT"
    tex.location = (-320, 0)
    tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])

    # The exporter follows only an output targeted ALL or CYCLES; an EEVEE-only
    # one yields glTF's default white metallic material with no warning.
    for node in tree.nodes:
        if node.type == "OUTPUT_MATERIAL":
            node.target = "ALL"
    return mat


def water_material():
    """The sea surface: transparent, glossy, and lit like everything else.

    Two flags here are the difference between an ocean and a bug. Without
    `BLEND` the sphere is an opaque shell and the planet loses its terrain
    entirely; without backface culling turned off it vanishes the moment the
    camera goes under, which is exactly when the player most needs to see it.
    """
    if not WATER_TEXTURE.is_file():
        raise SystemExit(f"missing water texture: {WATER_TEXTURE}")

    mat = bpy.data.materials.new("PlanetOcean")
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = tree.nodes["Principled BSDF"]
    # Glossier than any terrain: the sun glint off the sea is most of what
    # reads as water from orbit, where the surface texture is far too small to
    # see.
    bsdf.inputs["Roughness"].default_value = 0.12
    bsdf.inputs["Metallic"].default_value = 0.0
    bsdf.inputs["Alpha"].default_value = WATER_ALPHA

    image = bpy.data.images.load(str(WATER_TEXTURE), check_existing=True)
    image.pack()
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = image
    tex.interpolation = "Linear"
    tex.extension = "REPEAT"
    tex.location = (-320, 0)
    tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])

    # Blender renamed this between EEVEE and EEVEE Next and the glTF exporter
    # reads whichever exists, so both are set when both are there. Getting it
    # wrong writes alphaMode OPAQUE and the sea comes out solid.
    for attribute, value in (("blend_method", "BLEND"),
                             ("surface_render_method", "BLENDED")):
        if hasattr(mat, attribute):
            setattr(mat, attribute, value)
    mat.use_backface_culling = False
    mat.show_transparent_back = True

    for node in tree.nodes:
        if node.type == "OUTPUT_MATERIAL":
            node.target = "ALL"
    return mat


def build_ocean(m, material, parent, name):
    """One sphere at sea level, in its own object so nothing stands on it.

    The name matters beyond the outliner: the game reads a planet's collision
    straight out of this glTF, and it tells the sea from the ground by the node
    name. A tile called `ocean` would be water you could walk on, and an ocean
    called anything else would be a glass floor over the whole planet.
    """
    data = ocean.build(m)
    positions = data["positions"].astype(np.float64)
    triangles = data["triangles"].astype(np.int64)

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(positions.tolist(), [], triangles.tolist())
    mesh.update()
    mesh.materials.append(material)

    uv = mesh.uv_layers.new(name="water_uv")
    flat = data["uvs"].reshape(-1, 2)
    uv.data.foreach_set("uv", flat.ravel())

    # A sphere's normal is known exactly, so there is no reason to let Blender
    # infer it from the triangles and put a crease on the cube edges.
    try:
        mesh.normals_split_custom_set_from_vertices(
            data["normals"].astype(np.float64).tolist())
    except Exception as exc:                       # older/newer API drift
        print(f"  ocean custom normals unavailable ({exc}); using smooth shading")
    for poly in mesh.polygons:
        poly.use_smooth = True

    obj = bpy.data.objects.new(name, mesh)
    obj["sea_radius"] = float(data["radius"])
    parent.objects.link(obj)
    return obj


def assign_triplanar_uvs(mesh, positions, triangles, metres_per_repeat):
    """Project each triangle along its dominant normal axis.

    glTF has no procedural triplanar shader. Per-loop UV islands retain the
    useful part of triplanar mapping (world-space scale and no sphere poles)
    while remaining portable to the game export.
    """
    uv = mesh.uv_layers.new(name="terrain_uv")
    for poly, tri in zip(mesh.polygons, triangles):
        p = positions[tri]
        normal = np.cross(p[1] - p[0], p[2] - p[0])
        axis = int(np.argmax(np.abs(normal)))
        axes = ((1, 2), (0, 2), (0, 1))[axis]
        for loop_index, vertex_index in zip(poly.loop_indices, tri):
            co = positions[vertex_index]
            uv.data[loop_index].uv = (
                float(co[axes[0]] / metres_per_repeat),
                float(co[axes[1]] / metres_per_repeat),
            )


def build_tile(data, materials, parent):
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

    for mat in materials:
        mesh.materials.append(mat)

    # A triangle cannot interpolate a categorical material index. Choose its
    # majority vertex class, then use that class's world-scale planar mapping.
    tri_materials = data["material"][triangles]
    polygon_materials = np.array([
        np.bincount(values, minlength=len(materials)).argmax()
        for values in tri_materials
    ], dtype=np.int64)
    for poly, material_index in zip(mesh.polygons, polygon_materials):
        poly.material_index = int(material_index)

    # UV coordinates are per loop, so neighbouring triangles may select
    # different projection planes without splitting the welded geometry.
    assign_triplanar_uvs(mesh, positions, triangles, 1.0)
    uv = mesh.uv_layers.active
    for poly, tri, material_index in zip(mesh.polygons, triangles, polygon_materials):
        scale = float(surface.MATERIALS[int(material_index)]["texture_scale"])
        normal = np.cross(positions[tri[1]] - positions[tri[0]],
                          positions[tri[2]] - positions[tri[0]])
        axis = int(np.argmax(np.abs(normal)))
        axes = ((1, 2), (0, 2), (0, 1))[axis]
        for loop_index, vertex_index in zip(poly.loop_indices, tri):
            co = positions[vertex_index]
            uv.data[loop_index].uv = (float(co[axes[0]] / scale),
                                      float(co[axes[1]] / scale))
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


def export_lod(m, lod, materials, water):
    tiles = sorted((ROOT / "tiles" / f"lod{lod}").glob("*.npz"))
    collection = bpy.data.collections.new(f"LOD{lod}")
    bpy.context.scene.collection.children.link(collection)
    for path in tiles:
        with np.load(path) as z:
            build_tile({k: z[k] for k in z.files}, materials, collection)
    # Both LODs get their own copy of the sea. The two glTFs are written by
    # selecting a collection, so a shared object would land in whichever file
    # happened to be exported and be missing from the other.
    build_ocean(m, water, collection, "ocean" if lod == 0 else f"ocean_lod{lod}")
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
    materials = [ground_material(spec) for spec in surface.MATERIALS]
    water = water_material()
    # LOD1 is the one you open the file to; LOD0 is 96 objects and is there to
    # be worked on a tile at a time, not looked at all at once.
    lod0, n0 = export_lod(m, 0, materials, water)
    lod1, n1 = export_lod(m, 1, materials, water)

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
    print(f"sea level at r={ocean.sea_radius(m):.1f} m, "
          f"{len(bpy.data.objects['ocean'].data.polygons):,} triangles")


if __name__ == "__main__":
    main()
