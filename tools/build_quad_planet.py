#!/usr/bin/env python3
"""Build a quad-sphere planet and place independent Blender assets on it.

The source asset files are read-only inputs; this script writes a separate
planet blend. Run it with Blender:

    blender --background --factory-startup --python tools/build_quad_planet.py

Add more handcrafted models to ASSETS below. Each asset reserves a circular
patch spanning one or more planet quads. The surface is smoothly flattened
inside that patch and the model is placed tangent to the planet.
"""

from dataclasses import dataclass
from math import cos, radians, sin
from pathlib import Path
import os
import bpy
from mathutils import Matrix, Vector


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = Path(os.environ.get(
    "QUAD_PLANET_OUTPUT", ROOT / "assets" / "bevy" / "quad_planet.blend"))

# Planet controls. With 32 subdivisions each cube face contains 1,024 quads.
RADIUS = 300.0
SUBDIVISIONS = 32


@dataclass(frozen=True)
class Asset:
    name: str
    blend: Path
    latitude: float
    longitude: float
    # Handcrafted ground footprint in local meters (X width, Y depth). Terrain
    # stays tangent beneath it and begins curving immediately outside it.
    footprint: tuple
    # Width of the smooth terrain handoff in approximate surface quads.
    blend_quads: float = 4.0
    # Local asset coordinates: X/Y ground plane and +Z up.
    heading: float = 0.0
    height_offset: float = 0.0
    fallback_glb: Path = None


ASSETS = (
    Asset("CastleGrounds", ROOT / "assets/bevy/castle_grounds.blend",
          latitude=90.0, longitude=0.0, footprint=(164.0, 150.0),
          fallback_glb=ROOT / "assets/bevy/castle.glb"),
)


def direction(latitude, longitude):
    lat, lon = radians(latitude), radians(longitude)
    return Vector((cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat)))


def cube_sphere():
    """Return shared-per-face vertices and quad polygons for a cube sphere."""
    vertices, faces = [], []
    # (normal, horizontal, vertical), consistently viewed from outside.
    sides = (
        (Vector((1, 0, 0)), Vector((0, 1, 0)), Vector((0, 0, 1))),
        (Vector((-1, 0, 0)), Vector((0, -1, 0)), Vector((0, 0, 1))),
        (Vector((0, 1, 0)), Vector((-1, 0, 0)), Vector((0, 0, 1))),
        (Vector((0, -1, 0)), Vector((1, 0, 0)), Vector((0, 0, 1))),
        (Vector((0, 0, 1)), Vector((1, 0, 0)), Vector((0, 1, 0))),
        (Vector((0, 0, -1)), Vector((1, 0, 0)), Vector((0, -1, 0))),
    )
    patches = []
    for asset in ASSETS:
        center = direction(asset.latitude, asset.longitude)
        lon = radians(asset.longitude)
        east = Vector((-sin(lon), cos(lon), 0.0)).normalized()
        north = center.cross(east).normalized()
        h = radians(asset.heading)
        axis_x = east * cos(h) + north * sin(h)
        axis_y = -east * sin(h) + north * cos(h)
        patches.append((center, axis_x, axis_y, asset))
    quad_size = (RADIUS * 2.0) / SUBDIVISIONS

    for normal, axis_u, axis_v in sides:
        start = len(vertices)
        for y in range(SUBDIVISIONS + 1):
            v = y * 2.0 / SUBDIVISIONS - 1.0
            for x in range(SUBDIVISIONS + 1):
                u = x * 2.0 / SUBDIVISIONS - 1.0
                radial = (normal + axis_u * u + axis_v * v).normalized()
                radius = RADIUS
                # Reserve a smooth tangent platform. Angular distance is
                # measured as a chord, adequate for patches much smaller than
                # the planet. At the center, tangent-plane projection gives a
                # level surface at exactly RADIUS.
                point = radial * radius
                for center, axis_x, axis_y, asset in patches:
                    tangent = point - center * point.dot(center)
                    dx = abs(tangent.dot(axis_x)) - asset.footprint[0] * 0.5
                    dy = abs(tangent.dot(axis_y)) - asset.footprint[1] * 0.5
                    # Signed distance to a rectangle: <= 0 inside the actual
                    # handcrafted footprint, positive immediately outside it.
                    outside = Vector((max(dx, 0.0), max(dy, 0.0))).length
                    inside = min(max(dx, dy), 0.0)
                    edge_distance = outside + inside
                    blend = asset.blend_quads * quad_size
                    if edge_distance < blend:
                        flat = RADIUS / max(radial.dot(center), 0.001)
                        t = max(0.0, min(1.0,
                            edge_distance / max(blend, 0.001)))
                        t = t * t * (3.0 - 2.0 * t)
                        radius = radius * t + flat * (1.0 - t)
                vertices.append(tuple(radial * radius))
        row = SUBDIVISIONS + 1
        for y in range(SUBDIVISIONS):
            for x in range(SUBDIVISIONS):
                a = start + y * row + x
                faces.append((a, a + 1, a + row + 1, a + row))
    return vertices, faces


def make_planet():
    verts, faces = cube_sphere()
    mesh = bpy.data.meshes.new("QuadPlanetMesh")
    mesh.from_pydata(verts, [], faces)
    mesh.update()
    obj = bpy.data.objects.new("QuadPlanet", mesh)
    bpy.context.scene.collection.objects.link(obj)
    obj["topology"] = "cube sphere; quad faces"
    obj["radius_m"] = RADIUS
    obj["face_subdivisions"] = SUBDIVISIONS
    material = bpy.data.materials.new("PlanetGround")
    material.diffuse_color = (0.12, 0.32, 0.08, 1.0)
    material.use_nodes = True
    bsdf = material.node_tree.nodes.get("Principled BSDF")
    bsdf.inputs["Base Color"].default_value = material.diffuse_color
    bsdf.inputs["Roughness"].default_value = 0.92
    mesh.materials.append(material)
    return obj


def tangent_matrix(asset):
    up = direction(asset.latitude, asset.longitude)
    # East is stable except exactly at a pole, where longitude still selects a
    # useful heading direction.
    lon = radians(asset.longitude)
    east = Vector((-sin(lon), cos(lon), 0.0)).normalized()
    north = up.cross(east).normalized()
    h = radians(asset.heading)
    x_axis = east * cos(h) + north * sin(h)
    y_axis = -east * sin(h) + north * cos(h)
    origin = up * (RADIUS + asset.height_offset)
    return Matrix((x_axis.to_4d(), y_axis.to_4d(), up.to_4d(),
                   Vector((origin.x, origin.y, origin.z, 1.0)))).transposed()


def append_asset(asset, parent):
    if not asset.blend.is_file() and not (asset.fallback_glb and asset.fallback_glb.is_file()):
        print(f"skipped {asset.name}: missing {asset.blend}")
        return
    before = set(bpy.data.objects)
    source = asset.blend
    try:
        with bpy.data.libraries.load(str(asset.blend), link=False) as (src, dst):
            # Loading all objects supports source files which do not organize
            # their asset into a specially named collection.
            dst.objects = list(src.objects)
    except Exception as exc:
        if not asset.fallback_glb or not asset.fallback_glb.is_file():
            raise
        print(f"could not read {asset.blend.name} ({exc}); using {asset.fallback_glb.name}")
        source = asset.fallback_glb
        bpy.ops.import_scene.gltf(filepath=str(source), import_pack_images=True)
    loaded = [o for o in bpy.data.objects if o not in before]
    loaded_set = set(loaded)
    roots = [o for o in loaded if o.parent not in loaded_set]
    collection = bpy.data.collections.new(f"Asset_{asset.name}")
    bpy.context.scene.collection.children.link(collection)
    for obj in loaded:
        if not obj.users_collection:
            collection.objects.link(obj)
    anchor = bpy.data.objects.new(f"Placement_{asset.name}", None)
    collection.objects.link(anchor)
    anchor.parent = parent
    anchor.matrix_world = tangent_matrix(asset)
    anchor["source_asset"] = str(source.relative_to(ROOT))
    anchor["footprint_m"] = list(asset.footprint)
    anchor["blend_quads"] = asset.blend_quads
    for obj in roots:
        world = obj.matrix_world.copy()
        obj.parent = anchor
        obj.matrix_world = anchor.matrix_world @ world


def main():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    planet = make_planet()
    assets_root = bpy.data.objects.new("HandcraftedAssets", None)
    bpy.context.scene.collection.objects.link(assets_root)
    for asset in ASSETS:
        append_asset(asset, assets_root)
    scene = bpy.context.scene
    scene.name = "QuadPlanet"
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    scene["bevy_asset"] = True
    scene["generator"] = "tools/build_quad_planet.py"
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=str(OUTPUT), compress=True)
    print(f"wrote {OUTPUT}: {len(planet.data.polygons)} planet quads, "
          f"{len(ASSETS)} asset placements")


if __name__ == "__main__":
    main()
