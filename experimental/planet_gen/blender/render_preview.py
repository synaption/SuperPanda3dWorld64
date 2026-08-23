"""Render preview images of the built planet.

    blender --background --factory-startup --python blender/render_preview.py
"""

import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from planetgen import manifest  # noqa: E402


def look_at(camera, target, up=None):
    """Aim the camera, with an explicit up vector.

    to_track_quat resolves roll against world +Y, which is meaningless on a
    sphere: at most points on the planet it lands the horizon running down the
    frame instead of across it. Anything standing on the surface has to be
    rolled against its own radial up.
    """
    forward = (Vector(target) - camera.location).normalized()
    if up is None:
        camera.rotation_euler = forward.to_track_quat("-Z", "Y").to_euler()
        return
    right = forward.cross(Vector(up)).normalized()
    true_up = right.cross(forward).normalized()
    camera.rotation_euler = Matrix((right, true_up, -forward)).transposed().to_euler()


def setup(scene):
    scene.render.engine = "BLENDER_EEVEE"
    scene.render.resolution_x, scene.render.resolution_y = 1280, 960
    scene.render.film_transparent = False
    scene.eevee.taa_render_samples = 32
    world = bpy.data.worlds.new("Space")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs[0].default_value = (0.03, 0.035, 0.05, 1)
    scene.world = world

    sun = bpy.data.objects.new("Sun", bpy.data.lights.new("Sun", "SUN"))
    sun.data.energy = 3.0
    sun.data.angle = 0.02
    scene.collection.objects.link(sun)
    bpy.context.scene["sun"] = sun.name

    cam = bpy.data.objects.new("Camera", bpy.data.cameras.new("Camera"))
    scene.collection.objects.link(cam)
    scene.camera = cam
    return cam


def show(lod):
    for name in ("LOD0", "LOD1"):
        col = bpy.data.collections.get(name)
        if col:
            col.hide_viewport = col.hide_render = (name != f"LOD{lod}")


def vista_tile():
    """Pick a tile worth photographing: the most material variety, mid-height.

    The global peak is a bad choice -- it is pure snow at max altitude, so the
    shot comes back as a white slope with nothing in it. A tile spanning coast,
    grass and rock shows what the generator actually produced.
    """
    best, best_score = None, -1.0
    for path in sorted((ROOT / "tiles" / "lod0").glob("*.npz")):
        with np.load(path) as z:
            material, alt, pos = z["material"], z["altitude"], z["positions"]
            variety = len(np.unique(material))
            land = alt > 2.0
            if land.mean() < 0.45:
                continue
            score = variety + land.mean()
            if score > best_score:
                best_score = score
                centre = pos[land].mean(axis=0).astype(float)
                best = (Vector(centre), float(alt[land].mean()), path.stem, variety)
    return best


def aim_sun(travel):
    """Point the sun along a travel direction, so each shot is actually lit."""
    sun = bpy.data.objects[bpy.context.scene["sun"]]
    sun.rotation_euler = Vector(travel).normalized().to_track_quat("-Z", "Y").to_euler()


def render(cam, path, location, target, lens=50.0, lod=0, sun=None, up=None):
    if sun is not None:
        aim_sun(sun)
    show(lod)
    cam.data.lens = lens
    cam.data.clip_start, cam.data.clip_end = 0.1, 20000.0
    cam.location = location
    look_at(cam, target, up)
    bpy.context.scene.render.filepath = str(path)
    bpy.ops.render.render(write_still=True)
    print(f"wrote {path}")


def main():
    m = manifest.load(ROOT)
    r = m["radius"]
    bpy.ops.wm.open_mainfile(filepath=str(ROOT / "out" / "planet.blend"))
    scene = bpy.context.scene
    cam = setup(scene)
    out = ROOT / "out"

    # From space. Pulled back far enough to see the whole disc, with the sun
    # off to one side so the relief reads against a terminator.
    eye = Vector((1.0, -1.9, 1.1)).normalized() * r * 4.2
    sun_space = (Vector((0, 0, 0)) - eye).normalized() + Vector((-0.55, 0.15, -0.35))
    render(cam, out / "planet_space.png", eye, (0, 0, 0), 50, 0, sun_space)

    # Same camera, same sun, on LOD1 -- the honest cost of the space mesh.
    render(cam, out / "planet_lod1.png", eye, (0, 0, 0), 50, 1, sun_space)

    # Down on the surface. Aimed along the ground rather than at it, so the
    # horizon curve is in frame -- on a 300 m planet that curve is the whole
    # point and a shot pointed at your feet hides it.
    centre, alt, name, variety = vista_tile()
    up = centre.normalized()
    east = Vector((0, 0, 1)).cross(up)
    east = east.normalized() if east.length > 0.1 else Vector((1, 0, 0)).cross(up).normalized()
    north = up.cross(east).normalized()
    eye = up * (r + alt + 30.0) - east * 150.0 + north * 20.0
    look = up * (r + alt + 6.0) + east * 130.0
    sun_ground = (-up * 0.55 + east * 0.75 - north * 0.35)
    render(cam, out / "planet_surface.png", eye, look, 30, 0, sun_ground,
           up=eye.normalized())
    print(f"surface shot: tile {name}, {variety} materials, mean land {alt:+.1f} m")


if __name__ == "__main__":
    main()
