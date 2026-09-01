"""Seed assets/levels/castle.blend from the placements that were hard-coded.

    blender --background --factory-startup \
        --python tools/build_castle_furniture.py

A one-shot migration, and it must stay one: after this has run, the .blend is
the source and this script would overwrite hand edits with the values that
were in the Rust and the NPZ on the day the level stopped being written there.
`tools/export_level_furniture.py` is the one that runs every build, and it only
reads. This refuses to overwrite an existing file unless asked twice.

What it seeds is exactly what the game did before:

- the two water boxes from `assets/castle_grounds/collision.npz`
- the trees from `assets/castle_grounds/collision_objects.json`
- the 15-vertex movtex waterfall strip from `src/water.rs`
- the three warp pipes and what each produces, and the five hand-placed
  enemies, from `world::spawn_castle_inhabitants`
- `CASTLE_SPAWN` and the level's flat gravity, from `src/world.rs`

Two things are *linked* in rather than copied, so that placing is done against
what the game actually draws. The castle's own geometry, as a backdrop: it is
not authored here and rebuilding `castle_grounds.blend` should move it. And
each actor's model, as a collection every placement of that kind instances --
so a warp pipe empty draws a warp pipe, an ant empty draws an ant, and the file
holds one copy of each however many are placed. Neither is exported: the model
collections are not in the scene at all, and the exporter walks `Furniture`.
"""

import json
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

from dress_level_furniture import linked_model  # noqa: E402
from export_level_furniture import DISPLAY_SCALE, SHOWS, stem  # noqa: E402

REFERENCE = ROOT / "assets" / "bevy" / "castle_grounds.blend"
COLLISION = ROOT / "assets" / "castle_grounds" / "collision.npz"
COLLISION_OBJECTS = ROOT / "assets" / "castle_grounds" / "collision_objects.json"
WATER_TEXTURE = ROOT / "assets" / "bevy" / "water.png"
OUTPUT = ROOT / "assets" / "levels" / "castle.blend"

#: Which model each placement shows and where it is authored are `SHOWS` and
#: `MODEL_SOURCE` in the exporter, and the scale it is drawn at is
#: `DISPLAY_SCALE` beside them. All three are read from there rather than
#: repeated here, so that this and `tools/dress_level_furniture.py` -- which
#: puts the models back onto a level whose empties have lost them -- cannot come
#: to different conclusions about what an ant looks like.

#: SM64 units to metres, the port's world scale. Everything read out of the
#: decomp-derived data is in the former and everything authored here is in the
#: latter.
SCALE = 0.01

#: `WATERFALL_VERTICES` from src/water.rs: x, y, z in SM64 units, then s and t
#: in whole texture repeats. SM64's movtex coordinates are 6.10 fixed point, so
#: the integers in the level data name repeats rather than texels.
WATERFALL = [
    (-4469, -800, -6413, 0, 0), (-5525, 1171, -7026, 2, 0),
    (-6292, 2028, -7463, 4, 0), (-7302, 2955, -7461, 6, 0),
    (-4883, -800, -5690, 0, 3), (-5547, 1110, -6097, 2, 3),
    (-6732, 2587, -6770, 4, 3), (-7603, 3004, -7160, 6, 3),
    (-5580, -800, -4740, 0, 6), (-6151, 1110, -5155, 2, 6),
    (-7115, 2587, -5865, 4, 6), (-6151, -800, -4143, 0, 9),
    (-6687, 1110, -4573, 2, 9), (-7603, 2587, -5253, 4, 9),
    (-7603, 2955, -6210, 6, 9),
]
WATERFALL_FACES = [
    (0, 1, 5), (0, 5, 4), (1, 2, 6), (1, 6, 5), (2, 3, 6), (3, 7, 6),
    (4, 5, 9), (4, 9, 8), (5, 6, 9), (6, 10, 9), (6, 7, 10), (8, 9, 12),
    (8, 12, 11), (9, 10, 13), (9, 13, 12), (10, 7, 14), (10, 14, 13),
]

#: The three pipes and what each produces, and the five enemies standing on the
#: grounds when it comes up -- `world::spawn_castle_inhabitants`, in game
#: coordinates.
PIPES = [
    ("mario", 12.0, (-9.15, 2.6, 46.3)),
    ("slime", 12.0, (-55.1, 5.4, -39.2)),
    ("ant", 12.0, (46.8, 5.4, -68.1)),
]
ACTORS = [
    ("slime", (-3.0, 3.0, 26.0)),
    ("slime", (-24.0, 3.0, 29.0)),
    ("slime", (9.0, 3.0, 34.0)),
    ("ant", (-29.0, 3.0, 21.0)),
    ("ant", (4.0, 3.0, 19.0)),
]


def trees():
    """The decomp placements this one-shot migration moves into Blender."""
    objects = json.loads(COLLISION_OBJECTS.read_text())
    return [tuple(float(axis) * SCALE for axis in obj["pos"])
            for obj in objects if obj["preset"] == "special_bubble_tree"]

#: `world::CASTLE_SPAWN`.
SPAWN = (-13.28, 3.0, 46.64)

#: The waterfall's scroll, in texture repeats a second, and how much of the
#: cliff shows through it. SM64 advances S by 70/1024 of a repeat every 30 Hz
#: tick and leaves T alone; the alpha is 0xb4 of 0xFF.
WATERFALL_DRIFT = (70.0 * 30.0 / 1024.0, 0.0)
WATERFALL_ALPHA = 0xB4 / 0xFF


def to_blender(point):
    """Game coordinates (glTF, Y-up) to Blender's Z-up.

    The inverse of what the glTF exporter's `export_yup` does on the way back
    out, and the one piece of arithmetic in this pipeline that is silently
    wrong rather than loudly wrong when it is wrong: a level whose furniture is
    rotated a quarter turn about X still loads, still runs, and has its water
    standing on end.
    """
    x, y, z = point
    return Vector((x, -z, y))


def clear():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for block in (bpy.data.meshes, bpy.data.materials, bpy.data.images,
                  bpy.data.cameras, bpy.data.lights, bpy.data.collections):
        for item in list(block):
            block.remove(item)


def link_reference(into):
    """Link the castle's geometry in as a backdrop nobody can edit.

    A library link rather than an append, so rebuilding `castle_grounds.blend`
    moves what is being placed against instead of leaving this file showing
    last month's castle. Linked data is read-only in the UI, which is the
    property that matters: the level mesh is not authored here and an
    accidental nudge to it would export nothing and change nothing, so it would
    be found much later, by the collision no longer matching the picture.
    """
    if not REFERENCE.is_file():
        print(f"no {REFERENCE}; the furniture will have no backdrop")
        return
    with bpy.data.libraries.load(str(REFERENCE), link=True) as (source, target):
        target.objects = list(source.objects)
    linked = 0
    for obj in target.objects:
        if obj is None:
            continue
        into.objects.link(obj)
        linked += 1
    print(f"linked {linked} objects from {REFERENCE.name} as reference")


def water_material():
    """The sheet as Blender draws it, so the viewport shows water.

    Never exported and never read: the game builds this material itself in
    `src/water.rs`, because the two bodies of water it draws have to match each
    other rather than match whatever this file happens to say. It is here so
    that a plane being dragged over the moat looks like the moat.
    """
    material = bpy.data.materials.new("Water")
    material.blend_method = "BLEND"
    tree = material.node_tree
    shader = tree.nodes["Principled BSDF"]
    shader.inputs["Alpha"].default_value = 0x96 / 0xFF
    if WATER_TEXTURE.is_file():
        texture = tree.nodes.new("ShaderNodeTexImage")
        texture.image = bpy.data.images.load(str(WATER_TEXTURE))
        texture.location = (-320, 260)
        tree.links.new(shader.inputs["Base Color"], texture.outputs["Color"])
    return material


def empty(name, at, into, display="PLAIN_AXES", size=1.0):
    obj = bpy.data.objects.new(name, None)
    obj.empty_display_type = display
    obj.empty_display_size = size
    obj.location = to_blender(at)
    into.objects.link(obj)
    return obj


def link_models(kinds):
    """Every model these placements show, linked in and ready to be instanced.

    The holder collections are deliberately *not* put in the scene. A
    collection instance draws its contents wherever the instancing empty
    stands, so one link is enough for however many warp pipes the level ends up
    with, and the models themselves are nowhere -- which is what stops them
    being exported, selected, or nudged.

    Linked rather than appended for the same reason the castle is: re-exporting
    an actor should move what is being placed against, and an actor's model is
    not authored here.

    The linking itself is `dress_level_furniture.linked_model`, which is the
    tool that puts a model back onto a placement that has lost one. Seeding a
    level and dressing one are the same operation, so they are the same code:
    a file this wrote and a file that tool repaired are indistinguishable.
    """
    models = {}
    for kind in sorted(kinds):
        linked_model(kind, models)
    return models


def placement(name, at, models, into):
    """One thing standing somewhere, drawn as the thing it is.

    A collection instance is an empty with a collection hanging off it, so
    everything the exporter knows about empties still applies: the name says
    what this is and the custom properties say the rest. What the model adds is
    that you can see where the warp pipe's mouth actually comes to.

    Its scale is [`DISPLAY_SCALE`], which is the factor the game draws that
    model at -- one for everything authored at its final size, and 0.00667 for
    Mario, who is still the decomp's 160 units tall. The exporter reads it back
    for the placements whose size is the level's business and refuses it for the
    ones whose size is the model's.
    """
    obj = empty(name, at, into, display="PLAIN_AXES", size=0.5)
    model = SHOWS[stem(name)]
    scale = DISPLAY_SCALE.get(model, 1.0)
    obj.scale = (scale, scale, scale)
    holder = models.get(model)
    if holder is None:
        return obj
    obj.instance_type = "COLLECTION"
    obj.instance_collection = holder
    # The axes stay drawn under the model, small, so an empty that has ended up
    # inside a hill is still findable.
    obj.empty_display_size = 0.5 / max(scale, 1e-6)
    return obj


def water_planes(into, material):
    """One plane per water box, laid at the surface the box's top names.

    A plane and not a cube, because a plane is exactly what a water box is:
    `LevelData::water_depth` asks whether a point is inside the footprint and
    how far under the surface it is, and never asks where the bottom is. So
    the thing to author is the footprint and the height, and dragging a corner
    of a plane is how you say both.
    """
    boxes = np.load(COLLISION)["water_boxes"][:, 1:].astype(np.float64) * SCALE
    for index, (min_x, min_z, max_x, max_z, surface_y) in enumerate(boxes):
        mesh = bpy.data.meshes.new(f"Water{index}")
        corners = [(min_x, surface_y, min_z), (max_x, surface_y, min_z),
                   (max_x, surface_y, max_z), (min_x, surface_y, max_z)]
        mesh.from_pydata([to_blender(c) for c in corners], [], [(0, 1, 2, 3)])
        mesh.update()
        mesh.materials.append(material)
        # A single repeat over the plane. The game tiles the sheet on its own
        # fixed world scale, so this is for looking at, not for exporting.
        uv = mesh.uv_layers.new(name="UVMap")
        uv.data.foreach_set("uv", [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0])
        obj = bpy.data.objects.new("water", mesh)
        into.objects.link(obj)
    return len(boxes)


def waterfall(into, material):
    """SM64's `MOVTEX_CASTLE_WATERFALL`, as a mesh you can reshape.

    Its origin is put on the strip's centroid rather than on the world origin,
    for the reason the Rust did the same: a transparent surface is sorted by
    where its origin is, and an origin a hundred metres away from the thing it
    belongs to sorts against the wrong neighbours.
    """
    points = [Vector((v[0], v[1], v[2])) * SCALE for v in WATERFALL]
    origin = sum(points, Vector((0.0, 0.0, 0.0))) / len(points)
    mesh = bpy.data.meshes.new("Waterfall")
    mesh.from_pydata([to_blender(p - origin) for p in points], [],
                     WATERFALL_FACES)
    mesh.update()
    mesh.materials.append(material)
    uv = mesh.uv_layers.new(name="UVMap")
    flat = []
    for face in WATERFALL_FACES:
        for corner in face:
            flat.extend((float(WATERFALL[corner][3]), float(WATERFALL[corner][4])))
    uv.data.foreach_set("uv", flat)
    obj = bpy.data.objects.new("waterfall", mesh)
    obj.location = to_blender(origin)
    obj["drift_u"], obj["drift_v"] = WATERFALL_DRIFT
    obj["alpha"] = WATERFALL_ALPHA
    into.objects.link(obj)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if OUTPUT.is_file() and "--overwrite" not in argv:
        raise SystemExit(
            f"{OUTPUT.relative_to(ROOT)} already exists and is now the source "
            "of truth for these placements. Re-running this would replace "
            "them with the hard-coded values they were migrated out of. Pass "
            "-- --overwrite only if that is genuinely what you want.")

    clear()
    scene = bpy.context.scene
    scene.name = "CastleFurniture"
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0

    reference = bpy.data.collections.new("Reference")
    furniture = bpy.data.collections.new("Furniture")
    for collection in (reference, furniture):
        scene.collection.children.link(collection)
    link_reference(reference)

    material = water_material()
    boxes = water_planes(furniture, material)
    waterfall(furniture, material)

    empty("spawn", SPAWN, furniture, display="ARROWS", size=2.0)
    # A plain empty at the origin. Flat gravity does not need a position at
    # all -- down is `-Y` wherever you stand -- but it needs to be *sayable*,
    # and the alternative to an object is a level whose gravity is decided by
    # the absence of one.
    gravity = empty("gravity", (0.0, 0.0, 0.0), furniture,
                    display="SINGLE_ARROW", size=6.0)
    gravity["mode"] = "down"

    tree_positions = trees()
    models = link_models({SHOWS["pipe"], SHOWS["tree"]}
                         | {SHOWS[kind] for kind, _ in ACTORS})
    for spawns, interval, at in PIPES:
        # Every pipe shows a warp pipe. What comes out of it is a property, not
        # a different model, which is the distinction the level editor has to
        # make visible: three identical pipes producing three different things.
        pipe = placement("pipe", at, models, furniture)
        pipe["spawns"] = spawns
        pipe["interval"] = interval
    for kind, at in ACTORS:
        placement(kind, at, models, furniture)
    for at in tree_positions:
        placement("tree", at, models, furniture)

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=str(OUTPUT), compress=True,
                                relative_remap=True)
    print(f"wrote {OUTPUT}: {boxes} water boxes, 1 waterfall, "
          f"{len(PIPES)} pipes, {len(ACTORS)} actors, "
          f"{len(tree_positions)} trees")


if __name__ == "__main__":
    main()
