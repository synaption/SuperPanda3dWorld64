"""Render turnaround views of Valkyrie_MV4 to PNG. Run inside Blender."""

import math
import os

import bpy
from mathutils import Euler, Vector

OUT = r"C:\Users\boogers\Downloads\vk_views"
VIEWS = {
    "front": (0.0, 0.0),
    "three_quarter": (35.0, 0.0),
    "side": (90.0, 0.0),
    "back": (180.0, 0.0),
    "top_three_quarter": (35.0, 40.0),
    "hero_low": (25.0, -22.0),
}


def ensure_rig(target):
    scene = bpy.context.scene
    cam = bpy.data.objects.get("VK_Cam")
    if cam is None:
        cam = bpy.data.objects.new("VK_Cam", bpy.data.cameras.new("VK_Cam"))
        scene.collection.objects.link(cam)
    cam.data.lens = 50.0
    scene.camera = cam

    pivot = bpy.data.objects.get("VK_Pivot")
    if pivot is None:
        pivot = bpy.data.objects.new("VK_Pivot", None)
        scene.collection.objects.link(pivot)

    corners = [target.matrix_world @ Vector(c) for c in target.bound_box]
    lo = Vector(min(c[i] for c in corners) for i in range(3))
    hi = Vector(max(c[i] for c in corners) for i in range(3))
    pivot.location = (lo + hi) * 0.5
    cam.parent = pivot
    cam.matrix_parent_inverse.identity()
    return cam, pivot, max(hi - lo)


def ensure_lights():
    for name, rot, energy, colour in (
        ("VK_Key", (math.radians(58), 0.0, math.radians(-42)), 4.0, (1.0, 0.97, 0.92)),
        ("VK_Fill", (math.radians(70), 0.0, math.radians(120)), 1.4, (0.72, 0.82, 1.0)),
        ("VK_Rim", (math.radians(105), 0.0, math.radians(190)), 2.6, (0.8, 0.9, 1.0)),
    ):
        obj = bpy.data.objects.get(name)
        if obj is None:
            light = bpy.data.lights.new(name, "SUN")
            obj = bpy.data.objects.new(name, light)
            bpy.context.scene.collection.objects.link(obj)
        obj.data.energy = energy
        obj.data.color = colour
        obj.data.angle = math.radians(12)
        obj.rotation_euler = Euler(rot, "XYZ")

    world = bpy.context.scene.world
    if world is None:
        world = bpy.data.worlds.new("VK_World")
        bpy.context.scene.world = world
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs[0].default_value = (0.045, 0.050, 0.075, 1.0)
        bg.inputs[1].default_value = 1.0


def main():
    target = bpy.data.objects["Valkyrie_MV4"]
    cam, pivot, extent = ensure_rig(target)
    ensure_lights()

    scene = bpy.context.scene
    engines = scene.render.bl_rna.properties["engine"].enum_items.keys()
    scene.render.engine = (
        "BLENDER_EEVEE_NEXT" if "BLENDER_EEVEE_NEXT" in engines else "BLENDER_EEVEE"
    )
    scene.render.resolution_x = 640
    scene.render.resolution_y = 800
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = "PNG"

    os.makedirs(OUT, exist_ok=True)
    dist = extent * 1.65

    for name, (yaw, pitch) in VIEWS.items():
        y, p = math.radians(yaw), math.radians(pitch)
        cam.location = (
            dist * math.sin(y) * math.cos(p),
            -dist * math.cos(y) * math.cos(p),
            dist * math.sin(p),
        )
        direction = Vector(cam.location) - Vector((0.0, 0.0, 0.0))
        cam.rotation_euler = direction.to_track_quat("Z", "Y").to_euler()
        scene.render.filepath = os.path.join(OUT, name + ".png")
        bpy.ops.render.render(write_still=True)
        print("wrote", scene.render.filepath)


if __name__ == "__main__":
    main()
