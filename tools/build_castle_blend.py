#!/usr/bin/env python3
"""Build a clean, Bevy-oriented castle_grounds.blend from castle.glb.

Run with Blender, not the system Python:

    blender --background --factory-startup --python tools/build_castle_blend.py

The source GLB remains the generated runtime asset.  This file creates an
editable Blender source without the default camera/light/cube which can leak
into exports.  The resulting blend contains an ``Export_Bevy_GLTF`` text block
that can be run from Blender's Text Editor to export a fresh GLB.
"""

from pathlib import Path
import bpy


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets" / "bevy" / "castle.glb"
OUTPUT = ROOT / "assets" / "bevy" / "castle_grounds.blend"


EXPORT_SCRIPT = r'''# Run this text block from Blender's Text Editor.
from pathlib import Path
import bpy

blend = Path(bpy.data.filepath)
out = blend.with_name("castle_from_blender.glb")
bpy.ops.object.select_all(action="SELECT")
bpy.ops.export_scene.gltf(
    filepath=str(out),
    export_format="GLB",
    use_selection=True,
    export_yup=True,
    export_apply=False,
    export_texcoords=True,
    export_normals=True,
    export_colors=True,
    export_materials="EXPORT",
    export_cameras=False,
    export_lights=False,
    export_extras=True,
    export_animations=False,
)
print(f"Exported {out}")
'''


def main():
    if not SOURCE.is_file():
        raise SystemExit(f"missing source asset: {SOURCE}")

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for datablocks in (bpy.data.meshes, bpy.data.materials, bpy.data.images,
                       bpy.data.cameras, bpy.data.lights):
        for block in list(datablocks):
            datablocks.remove(block)

    bpy.ops.import_scene.gltf(filepath=str(SOURCE), import_pack_images=True)

    # These are Blender's factory scene, accidentally present in an earlier
    # export.  Match both name and type so a legitimate castle mesh cannot be
    # removed merely for having a generic name.
    unwanted = {"Cube": "MESH", "Camera": "CAMERA", "Light": "LIGHT"}
    for obj in list(bpy.context.scene.objects):
        if unwanted.get(obj.name) == obj.type:
            bpy.data.objects.remove(obj, do_unlink=True)

    scene = bpy.context.scene
    scene.name = "CastleGrounds"
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    # EEVEE was renamed in Blender 4.2; choose the identifier available in
    # the running Blender so the builder works on both Linux and Windows.
    engines = {item.identifier for item in
               scene.render.bl_rna.properties["engine"].enum_items}
    scene.render.engine = ("BLENDER_EEVEE_NEXT" if "BLENDER_EEVEE_NEXT" in engines
                           else "BLENDER_EEVEE")
    scene["bevy_asset"] = True
    scene["coordinate_system"] = "glTF Y-up; Blender Z-up"
    scene["units"] = "meters"

    # A single root gives Bevy one stable node to spawn while leaving every
    # material group independently cullable.  Parenting with keep-transform
    # preserves the imported glTF coordinates exactly.
    root = bpy.data.objects.new("CastleGrounds", None)
    root["bevy_role"] = "level_visual"
    root["source"] = "assets/bevy/castle.glb"
    scene.collection.objects.link(root)
    for obj in list(scene.objects):
        if obj.type == "MESH" and obj.parent is None:
            world = obj.matrix_world.copy()
            obj.parent = root
            obj.matrix_world = world

    for mesh in bpy.data.meshes:
        mesh.validate(clean_customdata=False)
        mesh.update()

    text = bpy.data.texts.get("Export_Bevy_GLTF") or bpy.data.texts.new(
        "Export_Bevy_GLTF")
    text.clear()
    text.write(EXPORT_SCRIPT)

    # Keep embedded source textures self-contained on Windows and WSL.
    bpy.ops.file.pack_all()
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=str(OUTPUT), compress=True)

    meshes = [o for o in scene.objects if o.type == "MESH"]
    triangles = sum(len(o.data.loop_triangles) for o in meshes)
    print(f"wrote {OUTPUT}: {len(meshes)} meshes, "
          f"{len(bpy.data.materials)} materials, {triangles} triangles")


if __name__ == "__main__":
    main()
