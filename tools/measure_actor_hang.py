#!/usr/bin/env python3
"""How far an actor's posed geometry sits below its own transform origin.

    python3 tools/measure_actor_hang.py assets/actors/slime.blend
    python3 tools/measure_actor_hang.py assets/actors/slime.blend --clip Scoot_Move

This is the authority for ``enemy::Kind::lift``. That constant exists because
the placement code seats an enemy's translation on the ground, and a rig whose
root sits up inside the body puts a third of the model underground when it does
-- the "scuttlebugs clipping through the floor" report.

**Why not read it off the baked impostor sheets.** `impostor::tests` measures
the lowest opaque pixel of a cell, which is the right instrument for a tall,
narrow actor and the wrong one for a wide, flat one. The bake camera is tilted
15 degrees down, so the near rim of a wide body projects below where its origin
projects even though nothing is below it in world space. On the slime that
overstates the hang by about 15 cm -- enough to float it a quarter of its own
height if the number were believed.

**Why not read it off the bind pose either.** The bind pose is not what is
drawn. This evaluates the skinned mesh on every frame of every clip, which is
also what tells a permanent offset apart from a transient one: a rig-root
offset is on every frame, and a squash that dips through the floor plane for a
few frames of a walk cycle is the animation doing its job.

Objects in Blender's ``glTF_not_exported`` collection are skipped. Its glTF
importer parks a unit Icosphere there in every file it imports, so every
committed actor source has one; it spans z = -1 to 1, it is not in any export,
and counting it makes every actor look like it hangs a whole unit below its
own origin.

The script is run twice, like `tools/blend_to_glb.py`: once as a plain python3
process that builds a Blender command line, and once inside Blender where
`bpy` exists.
"""

import argparse
import os
import subprocess
import sys

try:
    import bpy
except ImportError:                      # The launcher half.
    bpy = None

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PROJECT_BLENDER = os.path.join(ROOT, "blender-5.2.0-linux-x64", "blender")

# The collection Blender's glTF importer parks its own placeholder geometry in.
NOT_EXPORTED = "glTF_not_exported"


def measure_inside_blender(argv):
    parser = argparse.ArgumentParser(prog="measure_actor_hang (in-blender)")
    parser.add_argument("--clip", action="append", default=None,
                        help="only these clips (default: every one)")
    parser.add_argument("--scale", type=float, default=1.0,
                        help="Kind::draw_scale, to report in world metres")
    args = parser.parse_args(argv)

    scene = bpy.context.scene
    exported = lambda obj: not any(
        collection.name == NOT_EXPORTED for collection in obj.users_collection
    )
    meshes = [obj for obj in scene.objects if obj.type == "MESH" and exported(obj)]
    rigs = [obj for obj in scene.objects if obj.type == "ARMATURE" and exported(obj)]
    if not meshes:
        raise SystemExit("no mesh in this file")

    actions = list(bpy.data.actions)
    if args.clip:
        actions = [action for action in actions if action.name in args.clip]
        if not actions:
            raise SystemExit(f"no clip named {args.clip}")

    resting = None
    for action in actions:
        for rig in rigs:
            if not rig.animation_data:
                rig.animation_data_create()
            rig.animation_data.action = action
            slots = getattr(action, "slots", None)
            if slots:
                rig.animation_data.action_slot = slots[0]

        start, end = (int(value) for value in action.frame_range)
        lowest = float("inf")
        # Per frame as well as over the clip, because the two answer different
        # questions: the deepest frame is what a sheet measures, and the
        # shallowest is how far the model is below its origin at *all* times.
        shallowest = float("-inf")
        for frame in range(start, end + 1):
            scene.frame_set(frame)
            depsgraph = bpy.context.evaluated_depsgraph_get()
            here = float("inf")
            for mesh in meshes:
                evaluated = mesh.evaluated_get(depsgraph)
                data = evaluated.to_mesh()
                matrix = evaluated.matrix_world
                for vertex in data.vertices:
                    here = min(here, (matrix @ vertex.co).z)
                evaluated.to_mesh_clear()
            lowest = min(lowest, here)
            shallowest = max(shallowest, here)

        deepest_m = max(0.0, -lowest) * args.scale
        resting_m = max(0.0, -shallowest) * args.scale
        resting = resting_m if resting is None else min(resting, resting_m)
        print(f"{action.name}: deepest {deepest_m:.4f} m below origin, "
              f"resting {resting_m:.4f} m")

    if resting is not None:
        print(f"\nKind::lift for this actor: {resting:.3f}")
        print("(the resting hang -- a transient dip belongs on the floor, a "
              "permanent one is a model that sinks into it)")


def launch(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("blend")
    parser.add_argument("--clip", action="append")
    parser.add_argument("--scale", type=float, default=1.0)
    parser.add_argument("--blender",
                        default=PROJECT_BLENDER if os.path.isfile(PROJECT_BLENDER)
                        else "blender")
    args = parser.parse_args(argv)

    inner = []
    for clip in args.clip or []:
        inner.extend(["--clip", clip])
    inner.extend(["--scale", str(args.scale)])
    command = [args.blender, "--background", "-noaudio", "--factory-startup",
               args.blend, "--python", os.path.abspath(__file__),
               "--python-exit-code", "1", "--"] + inner
    return subprocess.run(command, cwd=ROOT).returncode


if __name__ == "__main__":
    if bpy is None:
        sys.exit(launch(sys.argv[1:]))
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    measure_inside_blender(argv)
