"""Check that each action's animation puts Mario's feet on the ground.

SM64 animations are authored relative to Mario's logical position, and that
position is at his feet. So for any action where he is standing on a floor,
the lowest vertex of the posed model should sit close to zero.

A clip that sits far below zero is nearly always a mis-mapped action -- a
grounded action pointing at an airborne clip -- which shows up in game as
Mario sunk into the floor. Airborne actions are exempt, and so are the few
actions that hang below their position on purpose, like a ledge grab.

Usage:
    python3 tools/check_anim_grounding.py [--tolerance 12]
"""

import argparse
import os
import re
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, ".."))

import sm64_anim  # noqa: E402
from export_actor_gltf import (  # noqa: E402
    ACTOR_SCALE,
    DEFAULT_EXCLUDE,
    MARIO_JOINT_NAMES,
    collect_joints,
    compose,
    euler_to_matrix,
    extract_meshes,
)
from geo_layout import build_tree, parse_geo_layouts  # noqa: E402
from parse_f3d import Level  # noqa: E402

from sm64py.mario import animations as A  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402

ACTION_NAMES = {v: k for k, v in vars(C).items() if k.startswith("ACT_")}


def build_poser(reference, actor="mario"):
    actor_dir = os.path.join(reference, "actors", actor)
    files = [os.path.join(actor_dir, f) for f in os.listdir(actor_dir)
             if f.endswith(".inc.c")]

    joints = collect_joints(
        build_tree(parse_geo_layouts(files), f"{actor}_geo_body"),
        MARIO_JOINT_NAMES if actor == "mario" else None,
    )
    animated = [i for i, j in enumerate(joints) if j.animated]

    level = Level()
    for path in files:
        level.add_source(path)

    pattern = DEFAULT_EXCLUDE.get(actor)
    mesh, vertex_bones = extract_meshes(
        level, joints, re.compile(pattern) if pattern else None
    )
    positions = mesh["positions"].astype(float)
    vertex_bones = np.array(vertex_bones)
    scale = ACTOR_SCALE.get(actor, 1.0)

    def lowest_point(anim, frame):
        translation, rotations = anim.sample(frame, len(animated))
        slot = {node: k for k, node in enumerate(animated)}
        globals_ = [None] * len(joints)

        for i, joint in enumerate(joints):
            if joint.animated:
                k = slot[i]
                offset = joint.translation
                if k == 0:
                    offset = tuple(offset[a] + translation[a] for a in range(3))
                matrix = np.eye(4)
                matrix[:3, :3] = euler_to_matrix(*rotations[k])
                matrix[:3, 3] = offset
            else:
                matrix = compose(joint.translation, joint.rotation, joint.scale)
            globals_[i] = (matrix if joint.parent < 0
                           else globals_[joint.parent] @ matrix)

        lowest = float("inf")
        for bone in np.unique(vertex_bones):
            mask = vertex_bones == bone
            points = np.c_[positions[mask], np.ones(mask.sum())]
            lowest = min(lowest, (globals_[bone] @ points.T).T[:, 1].min())
        return lowest * scale

    return lowest_point


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reference",
                        default=os.path.abspath(os.path.join(
                            HERE, "..", "reference", "Render96ex")))
    parser.add_argument("--tolerance", type=float, default=12.0,
                        help="how far below the floor is tolerable, in game units")
    args = parser.parse_args(argv[1:])

    lowest_point = build_poser(args.reference)
    animations = sm64_anim.load_animations(
        os.path.join(args.reference, "assets", "anims"))

    problems = []
    for action, entry in sorted(A.ACTION_ANIMATIONS.items()):
        if action & C.ACT_FLAG_AIR or action in A.EXPECTED_BELOW_GROUND:
            continue
        if callable(entry):
            continue

        clip = A.anim_name(entry)
        anim = animations.get(clip)
        if anim is None:
            continue

        # Only frames from the header's start frame onward are ever shown;
        # the lead-in frames before it never reach the screen.
        begin = min(anim.start_frame, max(anim.frame_count - 1, 0))
        step = max(1, (anim.frame_count - begin) // 8)
        lowest = min(lowest_point(anim, f)
                     for f in range(begin, anim.frame_count, step))
        if lowest < -args.tolerance:
            problems.append((lowest, ACTION_NAMES.get(action, hex(action)), clip))

    if not problems:
        print(f"All grounded actions keep their feet within "
              f"{args.tolerance:.0f} units of the floor.")
        return 0

    print("Grounded actions whose animation sinks below the floor:")
    for lowest, action, clip in sorted(problems):
        print(f"  {action:<28} {clip}  {lowest:7.1f}")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
