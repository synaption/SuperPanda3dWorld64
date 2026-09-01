"""A level's furniture .blend -> the two files the game reads.

    python3 tools/export_level_furniture.py castle
    python3 tools/export_level_furniture.py castle --blender /path/to/blender

Furniture is everything in a level that is placed rather than modelled: where
the player starts, which way gravity points, where the water is, where the warp
pipes are and what comes out of each, where the trees grow, and what is standing
about when the level comes up. All of it used to be literals in `src/world.rs`,
`src/water.rs` and the decomp's collision data, which meant moving a tree or a
warp pipe was a code change. Now it is an empty in
`assets/levels/<level>.blend`, and this is the step that carries it across.

Two outputs, because the game needs the two halves at different times:

    assets/bevy/<level>_furniture.json   placements, volumes and parameters
    assets/bevy/<level>_furniture.glb    the meshes those parameters describe

The JSON is small and `src/furniture.rs` embeds it with `include_str!`, the
same way `src/level.rs` embeds `castle.bin`, so gravity and the spawn point and
the water are known in the frame the level comes up rather than a few frames
later. The GLB is loaded like any other asset and its surfaces are adopted as
they arrive, which is fine: they are scenery.

What the exporter recognises, by the object's name up to the first dot -- so a
duplicate Blender called `pipe.001` is still a pipe:

    empty   spawn        where the player is put down
    empty   gravity      ["mode"] "down" or "radial"; radial uses its location
    empty   pipe         ["spawns"] mario|slime|ant, ["interval"] seconds
    empty   slime|ant    one of those, standing there when the level comes up
    empty   tree         a billboard tree rooted at this point
    empty   stellarator  a machine, at the size and the turn it is drawn with
    mesh    water        a water box: its footprint, at the height it sits
    mesh    <anything>   a drawn surface, exported to the GLB. ["drift_u"],
                         ["drift_v"] scroll it, ["alpha"] sets it transparent

Objects in the `Reference` collection are the level's own geometry, linked in
to place against, and are never exported. Anything unrecognised is reported and
skipped, so a typo shows up as a warning here rather than as furniture that
quietly is not in the game.

Like `tools/blend_to_glb.py`, this file is run twice: once as a normal python3
process that builds a Blender command line, and once inside Blender where bpy
exists. Keeping both halves together is what stops the naming rules above and
the command that applies them from drifting apart.
"""

import argparse
import json
import os
import subprocess
import sys

try:
    import bpy
    from mathutils import Vector
except ImportError:  # Outside Blender -- we are the launcher half.
    bpy = None

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PROJECT_BLENDER = os.path.join(ROOT, "blender-5.2.0-linux-x64", "blender")

#: The collection that is the level. Everything else in the file -- the linked
#: castle, a stray cube -- is scaffolding for the person placing things.
FURNITURE = "Furniture"

#: Empties whose name alone says what they are. Actors that a pipe can also
#: produce share these names on purpose: a slime is a slime whether it walked
#: out of a pipe or was standing there when the level came up.
ACTOR_KINDS = ("slime", "ant", "mario")

#: Placements that are structures rather than creatures. Like a pipe and unlike
#: an actor, the whole transform of one is the level's to decide: a stellarator
#: is built at a size the player chooses when the build button puts one down, so
#: a level saying what size it wants is saying something the game can already
#: hear. `src/stellarator.rs` measures the model and follows it, so the scale
#: here is a multiple of the machine's real size the way a pipe's is.
PROP_KINDS = ("stellarator",)

#: What a pipe produces if its `spawns` property says nothing, and how long it
#: waits between two of them.
DEFAULT_SPAWNS = "mario"
DEFAULT_INTERVAL = 12.0

#: The factor the game draws each model at, which is what a placement showing
#: that model is scaled by so the viewport tells the truth. It is a property of
#: the model and not of the level: `mario.glb` is 160 units tall because it came
#: out of the decomp that way.
#:
#: A one here means the model is authored at its final size, which is what most
#: models in this game are. The tree card and Mario retain their source units;
#: their factors make the linked models truthful in the level viewport. The
#: warp pipe was resized in its own source, so its placement stays at one.
#:
#: Only the pipe's is read back out (see `read_furniture` below). An actor's
#: size is measured off its own glTF by `enemy::Kind::body` and is what its
#: collision radius is built from, so a level that scaled one would be drawing a
#: creature at a size it does not collide at.
DISPLAY_SCALE = {
    "warp_pipe": 1.0,
    "tree": 0.01,
    "luna": 1.0,
    "stellarator": 1.0,
    "mario": 0.00667,
    "slime": 1.0,
    "ant": 1.0,
}

#: What each placement is a placement *of*, keyed by the object's name up to
#: the first dot -- the same stem `read_furniture` sorts on. A pipe shows a warp
#: pipe and the spawn shows Luna, who is what is standing there when the
#: level comes up; the rest are named after their own model.
#:
#: This is what `tools/dress_level_furniture.py` links into a level file so the
#: thing being dragged about is the thing itself. Nothing in the export reads
#: it: a placement is what its name says whether or not it has a model hanging
#: off it, and a level file that has never been dressed exports identically.
SHOWS = {
    "pipe": "warp_pipe",
    "tree": "tree",
    "spawn": "luna",
    "stellarator": "stellarator",
    "mario": "mario",
    "slime": "slime",
    "ant": "ant",
}

#: Where each model is authored. Every one of these is a .blend rather than the
#: .glb the game loads, because a level links to it: edit the ant and every ant
#: in every level is the new ant, next time the file is opened.
MODEL_SOURCE = {
    "warp_pipe": "assets/actors/warp_pipe.blend",
    "tree": "assets/actors/tree.blend",
    "stellarator": "assets/actors/stellarator.blend",
    "luna": "assets/luna/Luna.blend",
    "mario": "assets/mario/mario.blend",
    "slime": "assets/actors/slime.blend",
    "ant": "assets/actors/ant.blend",
}


def stem(name):
    """`pipe.001` -> `pipe`. Blender numbers duplicates and a duplicated warp
    pipe is still a warp pipe."""
    return name.split(".")[0]


def yaw_of(obj):
    """The placement's turn about the vertical, in radians.

    The only rotation worth carrying. Blender's vertical is Z and the game's is
    Y, and the axis conversion takes a turn of theta about Blender's +Z to a
    turn of theta about the game's +Y -- the same angle, which is the one piece
    of this conversion that needs no sign flipped. Tip a placement off the
    vertical and that part is silently dropped, which the exporter says so.
    """
    # Adding zero folds -0.0 onto 0.0, which is otherwise a diff on a file
    # nobody changed.
    return round(float(obj.matrix_world.to_euler("ZYX").z), 6) + 0.0


def uniform_scale(obj):
    """One number, or a stop. A placement is a thing standing somewhere, and a
    thing squashed along one axis is a modelling change wearing a placement's
    clothes."""
    x, y, z = (abs(v) for v in obj.matrix_world.to_scale())
    if max(x, y, z) - min(x, y, z) > 1e-4:
        raise SystemExit(
            f"{obj.name!r} is scaled {x:.4f}, {y:.4f}, {z:.4f}: a placement "
            "can only be scaled evenly")
    return round(x, 6)


def to_game(vector):
    """Blender's Z-up to the game's Y-up, the same turn `export_yup` makes.

    The GLB half of this export is handed the conversion by the exporter and
    the JSON half is not, so it happens here, once, against a test in
    `src/furniture.rs` that the two halves agree about where the waterfall is.
    """
    return [round(float(vector.x), 4), round(float(vector.z), 4),
            round(-float(vector.y), 4)]


# ---------------------------------------------------------------------------
# Inside Blender
# ---------------------------------------------------------------------------

def furniture_objects():
    collection = bpy.data.collections.get(FURNITURE)
    if collection is None:
        raise SystemExit(
            f"no {FURNITURE!r} collection: this .blend is not a level "
            "furniture file, or the collection was renamed")
    return sorted(collection.objects, key=lambda obj: obj.name)


def footprint(obj):
    """A mesh's world-space extent, as a water box.

    Read off the bounding box rather than off the vertices, so a plane that has
    been rotated flat still measures what it covers, and so the shape of the
    mesh does not matter: what the game asks a water box is whether an (x, z)
    is inside it and how far below the top a point is, and a bounding box
    answers both. The height is the top, so a plane dragged to a slope puts its
    water at the high corner rather than half way up.
    """
    corners = [obj.matrix_world @ Vector(corner) for corner in obj.bound_box]
    points = [to_game(corner) for corner in corners]
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    zs = [p[2] for p in points]
    return {
        "min_x": min(xs), "max_x": max(xs),
        "min_z": min(zs), "max_z": max(zs),
        "surface_y": max(ys),
    }


def read_furniture():
    """Every recognised object, sorted into the placements the game reads."""
    level = {"spawn": None, "gravity": None, "water": [], "pipes": [],
             "actors": [], "trees": [], "props": [], "surfaces": []}
    meshes = []
    unknown = []
    for obj in furniture_objects():
        kind = stem(obj.name)
        at = to_game(obj.matrix_world.translation)
        if obj.type == "MESH":
            if kind == "water":
                level["water"].append(footprint(obj))
            else:
                meshes.append(obj)
                level["surfaces"].append(surface(obj))
            continue
        if obj.type != "EMPTY":
            unknown.append((obj.name, obj.type))
            continue
        if kind == "spawn":
            level["spawn"] = at
        elif kind == "gravity":
            level["gravity"] = gravity(obj, at)
        elif kind == "pipe":
            # The only placement whose whole transform is the level's to
            # decide. A pipe is drawn and not collided with -- `world.rs` adds
            # nothing to the collision for one -- so nothing anywhere depends
            # on how big it is, and a pipe scaled or turned in Blender can be
            # scaled and turned in the game.
            level["pipes"].append({
                "spawns": actor_kind(obj, obj.get("spawns", DEFAULT_SPAWNS)),
                "interval": float(obj.get("interval", DEFAULT_INTERVAL)),
                "at": at,
                "yaw": yaw_of(obj),
                "scale": uniform_scale(obj),
            })
        elif kind in ACTOR_KINDS:
            check_display_scale(obj, kind)
            level["actors"].append({"kind": kind, "at": at})
        elif kind == "tree":
            check_display_scale(obj, kind)
            level["trees"].append(at)
        elif kind in PROP_KINDS:
            # A structure, and so the same three numbers a pipe gets. Nothing
            # walks, so nothing overwrites the turn a second later, and the
            # scale is read for the same reason a pipe's is: the game builds
            # these at a size already.
            level["props"].append({
                "kind": kind,
                "at": at,
                "yaw": yaw_of(obj),
                "scale": uniform_scale(obj),
            })
        else:
            unknown.append((obj.name, obj.type))
    for name, type_ in unknown:
        print(f"  ignored {name!r} ({type_}): no rule for that name")
    return level, meshes


def actor_kind(obj, value):
    """One of the things this game can put in a level, or a stop.

    Checked here rather than left to the game, because the person who can fix
    it is looking at Blender right now. `src/furniture.rs` refuses the same
    word a second time -- its parse fails and the game will not start -- so a
    hand-edited JSON cannot get past it either; this is the half that names the
    object you have to go and rename.
    """
    name = str(value).strip().lower()
    if name not in ACTOR_KINDS:
        raise SystemExit(
            f"{obj.name!r} spawns {value!r}, which is not one of "
            f"{', '.join(ACTOR_KINDS)}")
    return name


def check_display_scale(obj, kind):
    """A fixed-size placement's size is its model's, so report any edit.

    The instance is scaled to `DISPLAY_SCALE` purely so the viewport draws it
    at the size the game does. Actors derive collision from their model and
    trees have one model-wide runtime scale, so neither can be resized per
    placement here.
    """
    want = DISPLAY_SCALE.get(kind, 1.0)
    have = uniform_scale(obj)
    if abs(have - want) > 1e-4:
        print(f"  {obj.name!r} is scaled {have:g}, not the {want:g} the game "
              f"draws a {kind} at. Per-placement scale is ignored")
    if abs(yaw_of(obj)) > 1e-4:
        print(f"  {obj.name!r} is turned {yaw_of(obj):.3f} rad, which is "
              "ignored: this placement's facing is decided at runtime")


def gravity(obj, at):
    """Which way down is, out of one empty.

    `radial` carries its location because that is the whole of it -- down is
    towards the empty. `down` carries nothing: a flat level's gravity is `-Y`
    wherever you are standing, so an empty that has been dragged somewhere is
    saying nothing, and writing its position out would invite somebody to
    believe otherwise.
    """
    mode = str(obj.get("mode", "down")).lower()
    if mode not in ("down", "radial"):
        raise SystemExit(f"{obj.name!r}: gravity mode {mode!r} is neither "
                         "'down' nor 'radial'")
    out = {"mode": mode}
    if mode == "radial":
        out["centre"] = at
    if "accel" in obj:
        out["accel"] = round(float(obj["accel"]), 4)
    return out


def surface(obj):
    """A drawn surface's parameters. Its geometry goes in the GLB."""
    out = {"node": obj.name}
    drift = (float(obj.get("drift_u", 0.0)), float(obj.get("drift_v", 0.0)))
    if drift != (0.0, 0.0):
        out["drift"] = [round(drift[0], 6), round(drift[1], 6)]
    if "alpha" in obj:
        out["alpha"] = round(float(obj["alpha"]), 6)
    return out


def write_glb(path, meshes):
    """The surface meshes, and nothing else, in one small GLB.

    Selection-scoped like `blender/export_tiles.py`: the linked castle is in
    the file to place against and exporting it would ship a second copy of the
    level. Materials are left behind entirely -- `src/water.rs` owns what water
    looks like, so that the castle's sheets and the planet's sea stay the same
    substance, and exporting them packed `water.png` into this file a second
    time and took it from 6 KB to 365. What comes across is position, normals
    and UVs, which is all the game reads.
    """
    import addon_utils
    addon_utils.enable("io_scene_gltf2", default_set=False, persistent=False)
    bpy.ops.object.select_all(action="DESELECT")
    for obj in meshes:
        obj.select_set(True)
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        use_selection=True,
        export_yup=True,
        export_normals=True,
        export_texcoords=True,
        export_materials="NONE",
        export_cameras=False,
        export_lights=False,
        export_animations=False,
        export_apply=False,
        # Custom properties ride along as glTF extras. Nothing reads them
        # today -- the JSON carries the parameters, because the game needs
        # some of them before this file has finished loading -- but a surface
        # that has lost its scroll speed somewhere is much easier to find when
        # the .glb still says what it was.
        export_extras=True,
    )
    bpy.ops.object.select_all(action="DESELECT")
    if not os.path.isfile(path):
        raise SystemExit(f"exporter reported success but wrote no {path}")
    return os.path.getsize(path)


def export_inside_blender(argv):
    parser = argparse.ArgumentParser(prog="export_level_furniture (in Blender)")
    parser.add_argument("--level", required=True)
    parser.add_argument("--json", required=True)
    parser.add_argument("--glb", required=True)
    args = parser.parse_args(argv)

    level, meshes = read_furniture()
    if level["spawn"] is None:
        raise SystemExit("no 'spawn' empty: the level has nowhere to start")
    if level["gravity"] is None:
        raise SystemExit("no 'gravity' empty: the level has no down")
    level["level"] = args.level
    level["source"] = f"assets/levels/{args.level}.blend"

    size = write_glb(args.glb, meshes)
    with open(args.json, "w") as out:
        json.dump(level, out, indent=2, sort_keys=True)
        out.write("\n")
    print(f"wrote {args.json}: {len(level['water'])} water boxes, "
          f"{len(level['pipes'])} pipes, {len(level['actors'])} actors, "
          f"{len(level['trees'])} trees, "
          f"{len(level['props'])} props, {len(level['surfaces'])} surfaces, "
          f"gravity {level['gravity']['mode']}")
    print(f"wrote {args.glb}: {len(meshes)} surface meshes, {size:,} bytes")
    # A saved BlenderMCP handler can leave a server or audio thread alive even
    # in background/factory mode. Both outputs are closed above, so leave
    # directly instead of hanging a build after reporting that it succeeded.
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(0)


# ---------------------------------------------------------------------------
# Outside Blender -- the launcher half
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("level", help="the level to export, e.g. castle")
    parser.add_argument("--blender", help="Blender executable")
    args = parser.parse_args()

    blend = os.path.join(ROOT, "assets", "levels", f"{args.level}.blend")
    if not os.path.isfile(blend):
        raise SystemExit(f"no furniture file at {blend}")
    out = os.path.join(ROOT, "assets", "bevy")
    blender = args.blender or os.environ.get("BLENDER") or PROJECT_BLENDER
    command = [
        blender, "--background", "--factory-startup", blend,
        "--python", os.path.abspath(__file__),
        "--",
        "--level", args.level,
        "--json", os.path.join(out, f"{args.level}_furniture.json"),
        "--glb", os.path.join(out, f"{args.level}_furniture.glb"),
    ]
    print("+", " ".join(command), flush=True)
    result = subprocess.run(command, cwd=ROOT)
    return result.returncode


if __name__ == "__main__":
    if bpy is None:
        sys.exit(main())
    else:
        export_inside_blender(sys.argv[sys.argv.index("--") + 1:])
