#!/usr/bin/env python3
"""Render one Blender Suzanne preview for every downloaded PBR material folder.

Run with Blender, for example:
  blender --background --python tools/render_material_monkeys.py -- \
    --input materials_download --output monkey_material_pngs
"""

import argparse
import re
import sys
from pathlib import Path

import bpy
from mathutils import Vector


IMAGE_EXTENSIONS = {".jpg", ".jpeg", ".png", ".tif", ".tiff", ".exr", ".webp"}
MAP_PATTERNS = {
    "color": ("basecolor", "base_color", "diffuse", "diff", "albedo", "_col", "color"),
    "roughness": ("roughness", "rough", "_rgh"),
    "normal": ("normalgl", "normal", "_nor", "_nrm"),
    "height": ("displacement", "_disp", "height", "bump"),
    "metallic": ("metallic", "metalness", "_metal"),
    "ao": ("ambientocclusion", "ambient_occlusion", "_ao"),
    "alpha": ("opacity", "alpha", "transparency"),
}


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--size", type=int, default=512)
    parser.add_argument("--samples", type=int, default=32)
    parser.add_argument("--overwrite", action="store_true")
    return parser.parse_args(argv)


def classify(path):
    stem = re.sub(r"[ .-]+", "_", path.stem.lower())
    for kind, patterns in MAP_PATTERNS.items():
        if any(pattern in stem for pattern in patterns):
            return kind
    return None


def material_folders(root):
    for folder in sorted({p.parent for p in root.rglob("*") if p.suffix.lower() in IMAGE_EXTENSIONS}):
        images = sorted(p for p in folder.iterdir() if p.is_file() and p.suffix.lower() in IMAGE_EXTENSIONS)
        maps = {}
        for image in images:
            kind = classify(image)
            if kind and kind not in maps:
                maps[kind] = image
        if "color" in maps:
            yield folder, maps


def image_node(nodes, path, non_color=False):
    node = nodes.new("ShaderNodeTexImage")
    node.image = bpy.data.images.load(str(path), check_existing=True)
    node.interpolation = "Linear"
    node.projection = "FLAT"
    node.extension = "REPEAT"
    if non_color:
        node.image.colorspace_settings.name = "Non-Color"
    return node


def make_material(folder, maps):
    material = bpy.data.materials.new(folder.name)
    material.use_nodes = True
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    nodes.clear()
    output = nodes.new("ShaderNodeOutputMaterial")
    shader = nodes.new("ShaderNodeBsdfPrincipled")
    shader.inputs["Roughness"].default_value = 0.48
    links.new(shader.outputs["BSDF"], output.inputs["Surface"])

    color = image_node(nodes, maps["color"])
    links.new(color.outputs["Color"], shader.inputs["Base Color"])
    if "roughness" in maps:
        tex = image_node(nodes, maps["roughness"], True)
        links.new(tex.outputs["Color"], shader.inputs["Roughness"])
    if "metallic" in maps:
        tex = image_node(nodes, maps["metallic"], True)
        links.new(tex.outputs["Color"], shader.inputs["Metallic"])
    if "normal" in maps:
        tex = image_node(nodes, maps["normal"], True)
        normal = nodes.new("ShaderNodeNormalMap")
        normal.inputs["Strength"].default_value = 0.7
        links.new(tex.outputs["Color"], normal.inputs["Color"])
        links.new(normal.outputs["Normal"], shader.inputs["Normal"])
    elif "height" in maps:
        tex = image_node(nodes, maps["height"], True)
        bump = nodes.new("ShaderNodeBump")
        bump.inputs["Strength"].default_value = 0.25
        bump.inputs["Distance"].default_value = 0.12
        links.new(tex.outputs["Color"], bump.inputs["Height"])
        links.new(bump.outputs["Normal"], shader.inputs["Normal"])
    if "alpha" in maps:
        tex = image_node(nodes, maps["alpha"], True)
        links.new(tex.outputs["Color"], shader.inputs["Alpha"])
        material.surface_render_method = "DITHERED"
    return material


def point_at(obj, target=(0, 0, 0.15)):
    obj.rotation_euler = (Vector(target) - obj.location).to_track_quat("-Z", "Y").to_euler()


def setup_scene(size, samples):
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.resolution_x = size
    scene.render.resolution_y = size
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGBA"
    scene.render.film_transparent = False
    scene.render.image_settings.color_depth = "8"
    scene.render.resolution_percentage = 100
    scene.world.color = (0.018, 0.022, 0.032)
    scene.view_settings.look = "AgX - Medium High Contrast"
    scene.render.engine = "BLENDER_EEVEE_NEXT"
    scene.render.image_settings.compression = 25

    bpy.ops.mesh.primitive_monkey_add(location=(0, 0, 0.25))
    monkey = bpy.context.object
    monkey.name = "Material Preview Monkey"
    monkey.rotation_euler[2] = -0.15
    bpy.ops.object.shade_smooth()
    bevel = monkey.modifiers.new("Soft bevel", "BEVEL")
    bevel.width = 0.025
    bevel.segments = 2
    subdivision = monkey.modifiers.new("Subdivision", "SUBSURF")
    subdivision.levels = 2
    subdivision.render_levels = 2

    bpy.ops.mesh.primitive_plane_add(size=200, location=(0, 0, -0.78))
    floor = bpy.context.object
    floor_mat = bpy.data.materials.new("Studio Floor")
    floor_mat.diffuse_color = (0.025, 0.03, 0.045, 1)
    floor.data.materials.append(floor_mat)

    bpy.ops.object.camera_add(location=(0, -6.4, 0.35))
    camera = bpy.context.object
    camera.data.lens = 58
    point_at(camera)
    scene.camera = camera

    for location, energy, size_lamp, color in [
        ((-3.8, -4.2, 5.5), 1050, 4.0, (1.0, 0.84, 0.72)),
        ((4.0, -2.0, 2.8), 850, 3.0, (0.62, 0.76, 1.0)),
        ((0.0, 3.0, 4.0), 1150, 2.5, (0.78, 0.86, 1.0)),
    ]:
        bpy.ops.object.light_add(type="AREA", location=location)
        light = bpy.context.object
        light.data.energy = energy
        light.data.shape = "DISK"
        light.data.size = size_lamp
        light.data.color = color
        point_at(light)
    return scene, monkey


def safe_relative_name(folder, root):
    relative = folder.relative_to(root).as_posix()
    return re.sub(r"[^A-Za-z0-9._-]+", "_", relative).strip("_") + ".png"


def main():
    args = parse_args()
    root = args.input.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    scene, monkey = setup_scene(args.size, args.samples)
    jobs = list(material_folders(root))
    print(f"Found {len(jobs)} renderable material folders")
    for index, (folder, maps) in enumerate(jobs, 1):
        destination = output / safe_relative_name(folder, root)
        if destination.exists() and not args.overwrite:
            continue
        monkey.data.materials.clear()
        material = make_material(folder, maps)
        monkey.data.materials.append(material)
        scene.render.filepath = str(destination)
        print(f"[{index}/{len(jobs)}] {folder.name} -> {destination.name}", flush=True)
        bpy.ops.render.render(write_still=True)
        bpy.data.materials.remove(material, do_unlink=True)
        for image in list(bpy.data.images):
            if image.users == 0:
                bpy.data.images.remove(image)


if __name__ == "__main__":
    main()
