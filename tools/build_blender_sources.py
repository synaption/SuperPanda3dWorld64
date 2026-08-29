#!/usr/bin/env python3
"""Create and audit self-contained Blender sources for every 3D asset.

Run the audit after adding a model:

    python3 tools/build_blender_sources.py --check

Create any missing sources (or rebuild selected ones) headlessly:

    python3 tools/build_blender_sources.py
    python3 tools/build_blender_sources.py assets/actors/goomba.glb --force

Runtime GLBs remain committed so the game does not need Blender.  The matching
``.blend`` files are the editable source of truth from this point onward; the
first generation necessarily imports the old GLB/FBX and packs its textures.
"""

import argparse
import math
from pathlib import Path
import subprocess
import sys

try:
    import bpy
    import mathutils
except ImportError:
    bpy = None
    mathutils = None


ROOT = Path(__file__).resolve().parents[1]

# Exceptions where an existing hand-authored source deliberately has a
# different name, or one source produces more than one processed runtime GLB.
SOURCE_OVERRIDES = {
    "assets/bevy/castle.glb": "assets/bevy/castle_grounds.blend",
    "assets/luna/luna.glb": "assets/luna/Luna.blend",
}

SKIP_PARTS = {"packs"}
MODEL_SUFFIXES = {".glb", ".gltf", ".fbx", ".obj"}


def models():
    return sorted(
        path for path in (ROOT / "assets").rglob("*")
        if path.is_file()
        and path.suffix.lower() in MODEL_SUFFIXES
        and not SKIP_PARTS.intersection(path.relative_to(ROOT / "assets").parts)
    )


def source_for(model):
    relative = model.relative_to(ROOT).as_posix()
    override = SOURCE_OVERRIDES.get(relative)
    return ROOT / override if override else model.with_suffix(".blend")


def reset_scene():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for collection in (bpy.data.meshes, bpy.data.armatures, bpy.data.materials,
                       bpy.data.images, bpy.data.actions, bpy.data.cameras,
                       bpy.data.lights):
        for block in list(collection):
            collection.remove(block)


def import_model(model):
    suffix = model.suffix.lower()
    if suffix in {".glb", ".gltf"}:
        bpy.ops.import_scene.gltf(filepath=str(model), import_pack_images=True)
    elif suffix == ".fbx":
        bpy.ops.import_scene.fbx(filepath=str(model))
    elif suffix == ".obj":
        bpy.ops.wm.obj_import(filepath=str(model))
    else:
        raise SystemExit(f"unsupported model format: {model}")


def prepare_authoring_view():
    """Save a useful pose and viewport instead of Blender's cube-scale view."""
    scene = bpy.context.scene
    if bpy.data.actions:
        starts = [action.frame_range[0] for action in bpy.data.actions]
        ends = [action.frame_range[1] for action in bpy.data.actions]
        scene.frame_start = math.floor(min(starts))
        scene.frame_end = math.ceil(max(ends))
        # SM64's bind pose is not visually meaningful. Force dependency-graph
        # evaluation on the first animated frame before the file is saved.
        scene.frame_set(scene.frame_start)
        bpy.context.view_layer.update()

    corners = []
    for obj in scene.objects:
        if obj.type == "MESH":
            corners.extend(obj.matrix_world @ mathutils.Vector(corner)
                           for corner in obj.bound_box)
    if not corners:
        return
    low = mathutils.Vector(tuple(min(point[i] for point in corners)
                                 for i in range(3)))
    high = mathutils.Vector(tuple(max(point[i] for point in corners)
                                  for i in range(3)))
    center = (low + high) * 0.5
    extent = max(high - low)

    bpy.ops.object.select_all(action="DESELECT")
    bpy.context.view_layer.objects.active = None
    eye = center + mathutils.Vector((extent * 1.3, -extent * 1.8,
                                     extent * 1.1))
    view_rotation = (center - eye).to_track_quat("-Z", "Y")

    for area in bpy.context.screen.areas:
        if area.type != "VIEW_3D":
            continue
        space = area.spaces.active
        space.clip_start = max(0.01, extent / 10000)
        space.clip_end = max(1000, extent * 100)
        space.shading.type = "MATERIAL"
        space.region_3d.view_location = center
        space.region_3d.view_distance = max(1, extent * 1.2)
        # A stable three-quarter view makes asymmetry and broken poses obvious.
        space.region_3d.view_rotation = view_rotation


def build_inside_blender(model, output):
    reset_scene()
    import_model(model)
    scene = bpy.context.scene
    scene.name = model.stem
    scene["source_asset"] = model.relative_to(ROOT).as_posix()
    scene["runtime_export"] = model.relative_to(ROOT).as_posix()
    scene["coordinate_system"] = "glTF Y-up; Blender Z-up"
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0

    for mesh in bpy.data.meshes:
        mesh.validate(clean_customdata=False)
        mesh.update()

    prepare_authoring_view()

    # Some third-party FBXs retain absolute paths to textures that were never
    # distributed with the model. Remove those empty placeholders instead of
    # preserving a broken C:\\ path in the source. Available pixels stay put.
    for image in list(bpy.data.images):
        if image.source == "FILE" and not image.packed_file:
            resolved = Path(bpy.path.abspath(image.filepath))
            if not resolved.is_file():
                print(f"discarding unavailable image reference: {image.filepath}")
                bpy.data.images.remove(image)

    # Imported GLBs embed their images. Packing makes the new authoring file
    # independent of the old exporter and portable between WSL and Windows.
    bpy.ops.file.pack_all()
    output.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=str(output), compress=True)
    print(f"wrote {output.relative_to(ROOT)}")


def audit():
    missing = [(model, source_for(model)) for model in models()
               if not source_for(model).is_file()]
    if missing:
        for model, source in missing:
            print(f"MISSING {source.relative_to(ROOT)} for {model.relative_to(ROOT)}")
        return 1
    print(f"all {len(models())} primary 3D assets have Blender sources")
    return 0


def launch(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("models", nargs="*", help="specific model files to import")
    parser.add_argument("--check", action="store_true", help="only audit source coverage")
    parser.add_argument("--force", action="store_true", help="replace existing sources")
    parser.add_argument("--blender", default="blender", help="Blender executable")
    args = parser.parse_args(argv)
    if args.check:
        return audit()

    selected = [Path(item).resolve() for item in args.models] if args.models else models()
    script = Path(__file__).resolve()
    for model in selected:
        if model not in models():
            raise SystemExit(f"not a primary 3D asset: {model}")
        output = source_for(model)
        if output.exists() and not args.force:
            continue
        command = [args.blender, "--background", "--factory-startup",
                   "--python", str(script), "--python-exit-code", "1", "--",
                   str(model), str(output)]
        subprocess.run(command, check=True)
    return audit()


if __name__ == "__main__":
    if bpy is None:
        raise SystemExit(launch(sys.argv[1:]))
    inner = sys.argv[sys.argv.index("--") + 1:]
    build_inside_blender(Path(inner[0]), Path(inner[1]))
