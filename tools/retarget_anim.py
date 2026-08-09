"""Retarget an animation from another rig onto a decomp actor's skeleton.

The clips in reference/mesh2motion-app are authored on a humanoid rig with a
proper T-pose bind, and Mario's skeleton is nothing like it.  `rig.py` carries
the part that makes the transfer possible at all -- his bind pose is not a pose,
so his A-pose clip is what the joint conventions get measured from.  What is
left here is the source side.

The source rig is measured the same way against its own bind pose, giving a
constant J per bone.  A source joint's anatomical frame at time t is then
R_src(t) @ J^T, and the matching Mario joint's world rotation is that frame
carried back through its own constant.  Both rigs end up expressed in the same
body-relative terms, so this is an absolute transfer: Mario's bones point where
the source's bones point, and the two skeletons' rest postures never have to
agree.

Usage:
    python3 tools/retarget_anim.py --clip Zombie_Walk:zombie_walk \\
                                   --clip Zombie_Idle:zombie_idle

The retargeted clips are appended to assets/mario/mario.glb and recorded in
assets/mario/mario_clips.json, replacing any clip of the same name.  Re-running
tools/export_actor_gltf.py rewrites that .glb from the decomp and drops them,
so this is the step that follows it.
"""

import argparse
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, HERE)

import rig  # noqa: E402

DEFAULT_SOURCE = os.path.join(
    ROOT, "reference", "mesh2motion-app", "static", "animations",
    "human-base-animations.glb",
)
DEFAULT_TARGET = os.path.join(ROOT, "assets", "mario", "mario.glb")

# Which source joint drives which Mario joint, and the child that gives the
# source bone its direction.  Mario's own bone direction is always local +X,
# so only the source side needs a child named.
#
# Mario's torso is a single joint spanning the whole spine, so it follows
# spine_03: that is the segment his shoulders hang off, and taking it absolute
# rather than relative means a hunched chest still reads as hunched.
BONE_MAP = {
    "pelvis":     ("butt",        "spine_01"),
    "spine_03":   ("torso",       "neck_01"),
    "head":       ("head",        "head_leaf"),

    "upperarm_l": ("upper_arm.L", "lowerarm_l"),
    "lowerarm_l": ("forearm.L",   "hand_l"),
    "hand_l":     ("hand.L",      "middle_01_l"),
    "upperarm_r": ("upper_arm.R", "lowerarm_r"),
    "lowerarm_r": ("forearm.R",   "hand_r"),
    "hand_r":     ("hand.R",      "middle_01_r"),

    "thigh_l":    ("thigh.L",     "calf_l"),
    "calf_l":     ("shin.L",      "foot_l"),
    "foot_l":     ("foot.L",      "ball_l"),
    "thigh_r":    ("thigh.R",     "calf_r"),
    "calf_r":     ("shin.R",      "foot_r"),
    "foot_r":     ("foot.R",      "ball_r"),
}

# The joint whose translation carries the whole body, on the source rig.
SOURCE_ROOT = "pelvis"


def source_constants(source):
    """Per-bone J and the reference axis each bone was measured with.

    J relates a source joint's local axes to the anatomical frame of its bone
    in the bind pose, so R(t) @ J^T recovers that frame at any time.
    """
    rest = source.world()
    constants = {}
    for name, (_, child) in BONE_MAP.items():
        if name not in source.index or child not in source.index:
            raise KeyError(f"source rig has no joint {name!r} or {child!r}")
        head = rest[source.index[name]][:3, 3]
        tail = rest[source.index[child]][:3, 3]
        direction = tail - head
        if np.linalg.norm(direction) < 1e-9:
            raise ValueError(f"source bone {name!r} has no length")
        axis = rig.reference_axis(direction / np.linalg.norm(direction))
        frame = rig.anatomical_frame(direction, axis)
        constants[name] = (frame.T @ rest[source.index[name]][:3, :3], axis)
    return constants


def retarget(source, target, clip, frames=None):
    """Pose Mario from a source clip, one 30 Hz frame at a time.

    Returns (rotations, source root positions), rotations keyed by target node
    -- local quaternions, ready to write as glTF channels.
    """
    source_j = source_constants(source)
    # Each Mario joint is measured with the same reference axis its source
    # counterpart used, or the two rigs' rolls are not comparable.
    target_k = rig.joint_constants(target, {
        BONE_MAP[name][0]: axis for name, (_, axis) in source_j.items()
    })

    tracks = source.tracks(clip)
    if frames is None:
        # The final key of a cycle repeats the first, so the loop is the
        # duration minus one frame of it -- sampling through the end would
        # play that duplicate and stall the cycle for a frame.
        frames = max(int(round(rig.clip_length(tracks) * rig.FRAME_RATE)), 1)

    order = target.order()
    fallback = rig.defaults(target, order)
    animated = set(rig.animated_joints(target, order))

    rotations = {node: [] for node in animated}
    root_positions = []

    for frame in range(frames):
        world = source.world(rig.pose_at(source, tracks, frame / rig.FRAME_RATE))

        wanted = {}
        for source_name, (name, _) in BONE_MAP.items():
            j, _ = source_j[source_name]
            frame_world = world[source.index[source_name]][:3, :3] @ j.T
            wanted[target.index[name]] = frame_world @ target_k[name]

        local, _ = rig.locals_from_world(target, order, wanted, fallback)
        for node in animated:
            rotations[node].append(local[node])

        root_positions.append(world[source.index[SOURCE_ROOT]][:3, 3].copy())

    return ({node: np.array(values) for node, values in rotations.items()},
            np.array(root_positions), fallback, order)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--clip", action="append", required=True,
                        help="SOURCE_NAME:target_name, repeatable")
    parser.add_argument("--source", default=DEFAULT_SOURCE)
    parser.add_argument("--target", default=DEFAULT_TARGET)
    parser.add_argument("--frames", type=int, default=None,
                        help="override the resampled length")
    args = parser.parse_args(argv[1:])

    source = rig.Gltf(args.source)
    target = rig.Gltf(args.target)

    # Mario's hip height in the A-pose, and how much bigger he is than the
    # source rig standing beside him. Both come off the rigs rather than being
    # typed in, so a different source rig needs no new numbers.
    base_height = target.world(rig.a_pose(target))[target.index[rig.ROOT]][1, 3]
    scale = base_height / source.world()[source.index[SOURCE_ROOT]][1, 3]

    skin = rig.Skin(target)
    reference = rig.clearance(target, skin)
    clips_path = os.path.splitext(args.target)[0] + "_clips.json"

    for spec in args.clip:
        source_name, _, short = spec.partition(":")
        name = f"anim_{short or source_name.lower()}"

        rotations, source_root, fallback, order = retarget(
            source, target, source_name, args.frames)

        # Into Mario's units, measured from wherever the source rig's hips
        # started rather than from its origin, so his own hip height sets the
        # height of the clip and only the movement within it is borrowed.
        rest = source_root[0]
        root = np.column_stack([
            (source_root[:, 0] - rest[0]) * scale,
            base_height + (source_root[:, 1] - rest[1]) * scale,
            (source_root[:, 2] - rest[2]) * scale,
        ])
        root, shift = rig.ground(target, skin, rotations, root, fallback,
                                 order, reference)
        frames = rig.write_clip(target, name, rotations, root)
        steps = rig.footfalls(target, rotations, fallback, order, root)
        rig.record_clip(clips_path, name, frames)

        print(f"{source_name} -> {name}: {frames} frames, "
              f"dropped {shift:.1f} units onto the floor, "
              f"feet plant on {steps}")

    target.write(args.target)
    size = os.path.getsize(args.target)
    print(f"wrote {args.target} ({size / 1024:.0f} KB) and {clips_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
