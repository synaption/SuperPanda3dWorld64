"""Show every placement in a level .blend as the thing it actually is.

    python3 tools/dress_level_furniture.py castle
    python3 tools/dress_level_furniture.py castle --strip

A placement is an empty: `tools/export_level_furniture.py` reads its name, its
position and a custom property or two, and never looks at what it is drawn as.
That makes a level file that has never been near this tool export exactly the
same JSON as one that has -- but it also makes it a scene full of unlabelled
axes, and placing a warp pipe against a hillside by dragging a three-axis cross
is guesswork with a build in the middle of it.

So this hangs the real model off each empty, as a **linked** collection
instance. Linked, and that is the whole point: the model lives in its own
.blend, the level file holds a reference to it, and resizing the ant or
remodelling the warp pipe shows up in every level that places one the next time
that level is opened. Nothing is copied, so nothing can drift.

What gets shown is `SHOWS` in the exporter, and where each model is authored is
`MODEL_SOURCE` beside it -- one table, read by this and by
`tools/build_castle_furniture.py`, so a new kind of placement is added in one
place and both agree about what it looks like.

Run it whenever a level's empties have lost their models -- a placement created
by hand is a bare empty, and so is every one in a file that predates this -- or
after adding a model that a placement kind should show. It is idempotent: an
empty that is already instancing the right collection is left alone.

Three things it will not touch:

- **where anything is.** Position and rotation are the level's, and this tool
  has no opinion about them. It is a dresser, not a seeder;
  `tools/build_castle_furniture.py` is the one that ever placed anything, and it
  ran once.
- **a pipe's scale.** Nothing collides with a warp pipe, so how big one is is
  the level's business -- see `PipeSpec::scale` in `src/furniture.rs`. An
  actor's is not: its size is measured off its own model at load, so the scale
  on the empty is a viewport detail and this sets it to the factor the game
  draws that model at.
- **the JSON or the GLB.** Those come out of the exporter, which reads this
  file's output not at all.

`--strip` puts a level back to bare empties, which is worth having if a linked
model ever goes missing and Blender starts asking about it on load.

Like `tools/blend_to_glb.py`, the script is run twice: once as a plain python3
process that builds a Blender command line, and once inside Blender where `bpy`
exists.
"""

import argparse
import os
import subprocess
import sys

try:
    import bpy
except ImportError:  # Outside Blender -- we are the launcher half.
    bpy = None

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PROJECT_BLENDER = os.path.join(ROOT, "blender-5.2.0-linux-x64", "blender")

sys.path.insert(0, HERE)
from export_level_furniture import (  # noqa: E402
    DISPLAY_SCALE, MODEL_SOURCE, SHOWS, furniture_objects, stem,
)

#: The local collection a linked model is parked in, one per model however many
#: placements instance it. Named so that a level file's outliner says where the
#: extra collections came from, and so that `--strip` can find them again.
HOLDER = "Model %s"

#: Blender's own "do not export this" collection, which the actor sources keep a
#: reference icosphere in. Instancing it would draw every ant inside a ball.
NOT_EXPORTED = "glTF_not_exported"

#: Rigify's bone shapes. They are objects in the model file like any other, they
#: are not in `glTF_not_exported`, and Luna has two hundred and forty of
#: them -- wireframe circles and arrows sitting on his bones. The glTF export
#: never sees them because their collections are excluded from the view layer,
#: which is a thing a link cannot ask about, so they are dropped by name.
WIDGET_PREFIX = "WGT-"

#: How big a placement's own axes are drawn, in metres, once its model is
#: covering them. Small, because the model is the thing to look at now, but not
#: nothing: an empty that has ended up inside a hill still has to be findable.
AXES = 0.5


# ---------------------------------------------------------------------------
# Inside Blender
# ---------------------------------------------------------------------------

def linked_model(name, cache):
    """The collection holding one model, linked from the file it is authored in.

    Reused across placements and across runs: the second ant instances the same
    collection as the first, and a level dressed twice links nothing twice.
    """
    if name in cache:
        return cache[name]
    holder = bpy.data.collections.get(HOLDER % name)
    if holder is not None and holder.objects:
        cache[name] = holder
        return holder

    path = os.path.join(ROOT, MODEL_SOURCE[name])
    if not os.path.isfile(path):
        print(f"  no {MODEL_SOURCE[name]}: {name} placements stay bare empties")
        cache[name] = None
        return None
    with bpy.data.libraries.load(path, link=True, relative=True) as (src, dst):
        dst.objects = list(src.objects)
        dst.collections = [c for c in src.collections if c == NOT_EXPORTED]
    hidden = {obj.name for collection in dst.collections if collection
              for obj in collection.objects}
    holder = bpy.data.collections.new(HOLDER % name)
    for obj in dst.objects:
        if obj is None or obj.name in hidden:
            continue
        if obj.name.startswith(WIDGET_PREFIX):
            continue
        holder.objects.link(obj)
    print(f"  linked {name}: {len(holder.objects)} objects from "
          f"{MODEL_SOURCE[name]}")
    cache[name] = holder
    return holder


def dress(obj, holder, model):
    """Hang one model off one placement, and touch nothing else about it.

    A collection instance is still an empty, so everything the exporter knows
    about this object is still true: its name says what it is, its custom
    properties say the rest, and its position is where the thing goes.
    """
    changed = obj.instance_collection is not holder
    obj.instance_type = "COLLECTION"
    obj.instance_collection = holder
    # A pipe's size is the level's to decide and the exporter carries it
    # through, so it is not this tool's to overwrite. Everything else is drawn
    # at the factor the game draws that model at. Most models are authored at
    # their final size; Mario and the decomp tree card retain source units.
    scale = DISPLAY_SCALE.get(model, 1.0)
    if stem(obj.name) != "pipe" and tuple(obj.scale) != (scale, scale, scale):
        obj.scale = (scale, scale, scale)
        changed = True
    # The axes are drawn through the object's scale, so a Mario at 0.00667 needs
    # a display size of 75 to draw half a metre of them. Left alone where the
    # model draws at one, because then whatever size they are is a size somebody
    # chose while looking at the file.
    if abs(scale - 1.0) > 1e-6:
        obj.empty_display_size = round(AXES / max(scale, 1e-6), 4)
    return changed


def strip(obj):
    """Back to a bare empty."""
    if obj.instance_collection is None:
        return False
    obj.instance_collection = None
    obj.instance_type = "NONE"
    return True


def dress_inside_blender(argv):
    parser = argparse.ArgumentParser(prog="dress_level_furniture (in Blender)")
    parser.add_argument("--strip", action="store_true")
    args = parser.parse_args(argv)

    cache = {}
    dressed, bare, unknown = 0, 0, []
    for obj in furniture_objects():
        if obj.type != "EMPTY":
            continue
        kind = stem(obj.name)
        if kind == "gravity":
            # Which way down is. There is nothing to draw.
            continue
        model = SHOWS.get(kind)
        if model is None:
            unknown.append(obj.name)
            continue
        if args.strip:
            bare += strip(obj)
            continue
        holder = linked_model(model, cache)
        if holder is not None:
            dressed += dress(obj, holder, model)
    if args.strip:
        for collection in [c for c in bpy.data.collections
                           if c.name.startswith("Model ")]:
            bpy.data.collections.remove(collection)
        print(f"stripped {bare} placements back to bare empties")
    else:
        print(f"dressed {dressed} placements; "
              f"{len(cache)} models linked in")
    for name in unknown:
        print(f"  {name!r} shows nothing: no model for that name in SHOWS")
    bpy.ops.wm.save_mainfile(compress=True)
    print(f"saved {bpy.data.filepath}")


# ---------------------------------------------------------------------------
# Outside Blender -- the launcher half
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("level", help="the level to dress, e.g. castle")
    parser.add_argument("--blender", help="Blender executable")
    parser.add_argument("--strip", action="store_true",
                        help="remove the models and go back to bare empties")
    args = parser.parse_args()

    blend = os.path.join(ROOT, "assets", "levels", f"{args.level}.blend")
    if not os.path.isfile(blend):
        raise SystemExit(f"no furniture file at {blend}")
    blender = args.blender or os.environ.get("BLENDER") or PROJECT_BLENDER
    command = [blender, "--background", "--factory-startup", blend,
               "--python", os.path.abspath(__file__), "--"]
    if args.strip:
        command.append("--strip")
    print("+", " ".join(command), flush=True)
    result = subprocess.run(command, cwd=ROOT)
    if result.returncode == 0:
        print(f"\nassets/levels/{args.level}.blend was saved. Anything holding "
              "it open has the old one.")
    return result.returncode


if __name__ == "__main__":
    if bpy is None:
        sys.exit(main())
    else:
        dress_inside_blender(sys.argv[sys.argv.index("--") + 1:])
