"""Lock a clip's horizontal travel so the controller owns the character's position.

The Hero's clips were authored on a stage rather than on the spot, and it shows
in the exported data: measured at the spine, `Idle ` sits at the origin, the run
cycles start 0.68 units forward of it, and `Attack 2` starts 0.83 forward and
lunges another 0.85 during the swing. At the game's scale that is a couple of
hundred units, so simply playing one clip after another slides the character
across the ground and snaps him back on every transition -- while the physics
position, which is what walls and floors are tested against, never moves.

The fix is the usual division of labour: animation supplies the pose, the
controller supplies the position. Every frame of every clip is shifted so the
reference joint sits at the horizontal origin, which removes both the authored
offset and the in-clip travel. Height is left exactly as authored -- the feet
already sit on the origin plane, and the vertical differences between clips are
real crouches and leaps, not offsets.

An action that should cover ground during its clip -- the attack lunge -- gets
that back as forward velocity in `src/player.rs`, where it can be stopped by a
wall like any other movement.

Only the skeleton's *root* joints are shifted, and all of them by the same
amount, so the body keeps its shape; everything below a root moves with its
parent already.

Usage:
    python3 tools/lock_root_motion.py assets/hero/hero.glb
    python3 tools/lock_root_motion.py assets/hero/hero.glb --reference DEF-spine
"""

import argparse
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import rig  # noqa: E402

DEFAULT_REFERENCE = "DEF-spine"


def skin_roots(gltf):
    """Joints with no parent among the joints -- what a clip translates."""
    joints = gltf.json["skins"][0]["joints"]
    inside = set(joints)
    return [j for j in joints if gltf.parent.get(j) not in inside]


def lock(gltf, reference=DEFAULT_REFERENCE, clips=None):
    """Shift every frame so `reference` sits at x=0, z=0. Returns per-clip shift."""
    if reference not in gltf.index:
        raise KeyError(f"no joint named {reference!r} in this file")
    ref_node = gltf.index[reference]
    roots = skin_roots(gltf)
    if ref_node not in roots:
        raise ValueError(
            f"{reference!r} is not a root joint; shifting it would move it "
            "relative to its parent rather than moving the body")

    report = {}
    for anim in gltf.json.get("animations", []):
        name = anim.get("name")
        if clips and name not in clips:
            continue

        channels = {}
        for channel in anim["channels"]:
            if channel["target"]["path"] != "translation":
                continue
            node = channel["target"]["node"]
            if node in roots:
                channels[node] = anim["samplers"][channel["sampler"]]

        sampler = channels.get(ref_node)
        if sampler is None:
            report[name] = None          # nothing translates; nothing to lock
            continue

        ref_times = gltf.read(sampler["input"])[:, 0]
        ref_values = gltf.read(sampler["output"])

        moved = 0.0
        for node, sampler in channels.items():
            times = gltf.read(sampler["input"])[:, 0]
            values = gltf.read(sampler["output"]).copy()

            # Sampled per channel rather than assumed shared: the exporter is
            # asked not to reduce keyframes, but a clip that was authored with
            # keys on different frames still arrives with different times.
            shift = np.array([
                rig.sample_track(ref_times, ref_values, t, rotation=False)
                for t in times
            ])
            moved = max(moved, float(np.abs(shift[:, [0, 2]]).max()))

            values[:, 0] -= shift[:, 0]
            values[:, 2] -= shift[:, 2]
            sampler["output"] = gltf.add_array(values, "VEC3", with_bounds=True)

        report[name] = moved

    gltf.compact()
    return report


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("model", help="the .glb to rewrite in place")
    parser.add_argument("--reference", default=DEFAULT_REFERENCE,
                        help=f"joint held at the origin (default {DEFAULT_REFERENCE})")
    parser.add_argument("--out", default=None, help="defaults to overwriting the model")
    args = parser.parse_args(argv[1:])

    gltf = rig.Gltf(args.model)
    report = lock(gltf, args.reference)
    gltf.write(args.out or args.model)

    for name, moved in sorted(report.items(), key=lambda kv: -(kv[1] or 0)):
        if moved is None:
            print(f"  {name:22} no root translation")
        else:
            print(f"  {name:22} removed up to {moved:.3f} units of travel")
    print(f"locked {len(report)} clips against {args.reference!r} in "
          f"{args.out or args.model}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
