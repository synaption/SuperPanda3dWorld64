#!/usr/bin/env python3
"""Set how big an actor is, in its .blend, so the whole pipeline follows.

    python3 tools/resize_actor.py assets/actors/ant.blend
    python3 tools/resize_actor.py assets/actors/ant.blend --reach 0.60
    python3 tools/resize_actor.py assets/actors/slime.blend --factor 0.5

With no target it measures and prints, and changes nothing.

**Size is authored, not tuned.** The game reads an actor's collision radius, its
height and how far it hangs below its own origin straight out of the model it
loads -- see `enemy::Kind::body` -- so the size in the .blend is the size in the
game, and there is no scale factor anywhere in the code to keep in step with it.
This is how that size gets changed.

Which is not the same as typing a number into the armature's scale field. A
rigged actor's size lives in its **mesh data and bone rest positions**, and an
object-level scale on the armature is cancelled twice over: Blender's own
parent-inverse cancels it for the mesh in the viewport, and glTF's inverse bind
matrices cancel it again on export. `assets/actors/ant.blend` carried a 4.0 that
did precisely nothing -- the model in the file and the model in the game were
both the same 4.13 units long. So this scales and then *applies*, which is the
only form of the operation that reaches the exported file.

Applying scale to a rigged armature has a reputation for eating animation, so
the tool checks rather than trusts: it evaluates the posed mesh over every frame
of every clip before and after, and refuses to save unless every extent came out
scaled by exactly the factor asked for.

After this, re-export and re-bake:

    python3 tools/build_assets.py --only actors --only impostors

The sheets are not optional. They are pictures of the model, so an actor
resized without them is drawn at the new size up close and the old size beyond
`enemy_draw`.

The script is run twice, like `tools/blend_to_glb.py`: once as a plain python3
process that builds a Blender command line, and once inside Blender where `bpy`
exists.
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
# Every imported actor has a unit Icosphere in there, in no export, and counting
# it would measure that instead of the actor.
NOT_EXPORTED = "glTF_not_exported"

# How far a scaled extent may sit from where the factor says it should, as a
# fraction of the actor's own size. Loose enough for what an apply does to a
# rest skeleton -- the ant's widest leg spread came back 0.1% narrower, a
# millimetre on a 60 cm animal -- and tight enough that a clip whose keyframes
# did not come along is a refusal rather than a shrug, since that lands at 12%.
TOLERANCE = 5e-3


def exported(obj):
    return not any(c.name == NOT_EXPORTED for c in obj.users_collection)


def rig_of(mesh):
    """The armature deforming this mesh, if any."""
    if mesh.parent and mesh.parent.type == "ARMATURE":
        return mesh.parent
    for modifier in mesh.modifiers:
        if modifier.type == "ARMATURE" and modifier.object:
            return modifier.object
    return None


def scale_keyframes(factor):
    """Multiply every keyframed *length* in the file by `factor`.

    This is the half of the operation Blender's apply-scale does not do, and
    the reason applying scale to a rigged armature has its reputation. A pose
    bone's `location` is a length in bone space; applying scale rewrites the
    rest skeleton those lengths are measured against and leaves the keyframes
    alone, so every animated translation is suddenly that much larger relative
    to the actor. Rotations are angles and scales are ratios -- neither has
    units to fix.

    It is easy to think an actor is fine without this. The ant's two clips are
    pure rotation, so it came through the apply looking perfect; the slime's ten
    all key location, and its walk cycle came out 12% too big.

    Blender 5 has no `Action.fcurves`: an action is layers of strips of
    channelbags, one bag per slot.
    """
    touched = 0
    for action in bpy.data.actions:
        for layer in action.layers:
            for strip in layer.strips:
                for bag in strip.channelbags:
                    for curve in bag.fcurves:
                        if not curve.data_path.rsplit(".", 1)[-1] == "location":
                            continue
                        for key in curve.keyframe_points:
                            key.co.y *= factor
                            key.handle_left.y *= factor
                            key.handle_right.y *= factor
                        curve.update()
                        touched += 1
    return touched


def posed_bounds(meshes, rigs):
    """World-space bounds over every frame of every clip.

    Every clip, because this is the check that the animation survived, and a
    resize that only kept the walk cycle is not a resize that worked.
    """
    scene = bpy.context.scene
    low = [float("inf")] * 3
    high = [float("-inf")] * 3

    def sweep():
        depsgraph = bpy.context.evaluated_depsgraph_get()
        for mesh in meshes:
            evaluated = mesh.evaluated_get(depsgraph)
            data = evaluated.to_mesh()
            matrix = evaluated.matrix_world
            for vertex in data.vertices:
                point = matrix @ vertex.co
                for axis in range(3):
                    low[axis] = min(low[axis], point[axis])
                    high[axis] = max(high[axis], point[axis])
            evaluated.to_mesh_clear()

    actions = list(bpy.data.actions) if rigs else []
    if not actions:
        sweep()
    for action in actions:
        for rig in rigs:
            if not rig.animation_data:
                rig.animation_data_create()
            rig.animation_data.action = action
            slots = getattr(action, "slots", None)
            if slots:
                rig.animation_data.action_slot = slots[0]
        start, end = (int(value) for value in action.frame_range)
        for frame in range(start, end + 1):
            scene.frame_set(frame)
            sweep()
    return low, high


def rest_bounds(meshes, rigs):
    """World-space bounds of the bind pose.

    This is the measurement the *game* makes: a glTF's POSITION accessors hold
    the mesh as it was bound, so `enemy::Kind::body` sees the rest pose and
    nothing else. Reporting anything else here would be a tool that disagrees
    with the thing it is meant to be the authority for.
    """
    was = [(rig, rig.data.pose_position) for rig in rigs]
    for rig in rigs:
        rig.data.pose_position = "REST"
    bpy.context.view_layer.update()
    low, high = posed_bounds(meshes, [])
    for rig, before in was:
        rig.data.pose_position = before
    bpy.context.view_layer.update()
    return low, high


def describe(low, high):
    # Blender is Z-up and glTF is Y-up, so the game's "height" is z here and
    # its horizontal reach is x and y.
    reach = max(abs(low[0]), abs(high[0]), abs(low[1]), abs(high[1]))
    return reach, high[2] - low[2], max(0.0, -low[2])


def resize_inside_blender(argv):
    parser = argparse.ArgumentParser(prog="resize_actor (in-blender)")
    parser.add_argument("--reach", type=float)
    parser.add_argument("--height", type=float)
    parser.add_argument("--factor", type=float)
    parser.add_argument("--save-to")
    args = parser.parse_args(argv)

    scene = bpy.context.scene
    meshes = [o for o in scene.objects if o.type == "MESH" and exported(o)]
    rigs = [o for o in scene.objects if o.type == "ARMATURE" and exported(o)]
    if not meshes:
        raise SystemExit("no exported mesh in this file")

    low, high = rest_bounds(meshes, rigs)
    reach, height, hang = describe(low, high)
    print(f"as authored: reach {reach:.4f}, height {height:.4f}, "
          f"hangs {hang:.4f} below its origin")

    asked = [a for a in (args.reach, args.height, args.factor) if a is not None]
    if not asked:
        return
    if len(asked) > 1:
        raise SystemExit("pick one of --reach, --height, --factor")
    if args.factor is not None:
        factor = args.factor
    elif args.reach is not None:
        factor = args.reach / reach
    else:
        factor = args.height / height
    if not factor > 0.0:
        raise SystemExit("a size has to be positive")

    before_low, before_high = posed_bounds(meshes, rigs)

    for obj in scene.objects:
        obj.select_set(False)
    # The rigs carry the scale and the meshes come along in the same call, so
    # that bone rest positions and the vertices weighted to them are scaled by
    # one operation rather than two that could disagree.
    movers = rigs or meshes
    for mover in movers:
        mover.scale = tuple(value * factor for value in mover.scale)
        # The location as well, or an actor authored away from the world origin
        # keeps its old offset and ends up hovering by the difference.
        mover.location = tuple(value * factor for value in mover.location)
    bpy.context.view_layer.update()
    for obj in movers + meshes:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = movers[0]
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True,
                                   isolate_users=True)
    curves = scale_keyframes(factor)
    print(f"scaled {curves} keyframed location channels")

    after_low, after_high = posed_bounds(meshes, rigs)
    for axis, name in enumerate("xyz"):
        span = max(after_high[axis] - after_low[axis], 1e-6)
        for edge, (was, now) in enumerate(zip((before_low, before_high),
                                              (after_low, after_high))):
            wanted = was[axis] * factor
            got = now[axis]
            # Against the actor's own size rather than the coordinate, so the
            # tolerance means the same thing at the far end of a long model as
            # it does next to the origin. Skinning is evaluated in floats and a
            # thousandth of an actor is noise; a broken clip is percents.
            slack = span * TOLERANCE
            if abs(got - wanted) > slack:
                raise SystemExit(
                    f"the posed mesh did not scale: {name} "
                    f"{'low' if edge == 0 else 'high'} wanted {wanted:.4f}, "
                    f"got {got:.4f}. Nothing was saved."
                )

    low, high = rest_bounds(meshes, rigs)
    reach, height, hang = describe(low, high)
    print(f"scaled by {factor:.5f}")
    print(f"now:         reach {reach:.4f}, height {height:.4f}, "
          f"hangs {hang:.4f} below its origin")
    for mover in movers:
        print(f"  {mover.name} object scale is now "
              f"{tuple(round(v, 4) for v in mover.scale)}")

    if args.save_to:
        bpy.ops.wm.save_as_mainfile(filepath=args.save_to)
        print(f"WROTE {args.save_to}")


def launch(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("blend")
    parser.add_argument("--reach", type=float,
                        help="target horizontal distance from the origin, "
                             "which is what becomes the collision radius")
    parser.add_argument("--height", type=float, help="target height")
    parser.add_argument("--factor", type=float, help="multiply the size by this")
    parser.add_argument("--out", help="write here instead of over the .blend")
    parser.add_argument("--blender",
                        default=PROJECT_BLENDER if os.path.isfile(PROJECT_BLENDER)
                        else "blender")
    args = parser.parse_args(argv)

    inner = []
    for flag in ("reach", "height", "factor"):
        value = getattr(args, flag)
        if value is not None:
            inner.extend([f"--{flag}", str(value)])
    if inner:
        inner.extend(["--save-to", os.path.abspath(args.out or args.blend)])
    command = [args.blender, "--background", "-noaudio", "--factory-startup",
               args.blend, "--python", os.path.abspath(__file__),
               "--python-exit-code", "1", "--"] + inner
    return subprocess.run(command, cwd=ROOT).returncode


if __name__ == "__main__":
    if bpy is None:
        sys.exit(launch(sys.argv[1:]))
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    resize_inside_blender(argv)
