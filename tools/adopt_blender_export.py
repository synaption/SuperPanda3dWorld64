"""Make a Blender-exported mario.glb loadable by Panda3D, and resync the sidecar.

Blender's glTF exporter is not a faithful round trip of what
tools/export_actor_gltf.py writes. Two differences break the game, and both
are silent -- the file loads, and Mario is simply wrong on screen:

**The mesh comes back as a sibling of the skeleton.** The exporter wraps
everything in a new node and hangs the skinned mesh and the joint root off it
side by side. Panda3D's glTF loader builds its Character from the joint
hierarchy and only adopts geometry sitting *underneath* it, so a sibling mesh
renders but never binds and the Actor animates nothing. This reparents the
mesh back under the skeleton root, which is where the decomp exporter puts it.

**Some clips come back a frame shorter.** Sampling and re-emitting a clip can
drop its final frame, and the game reads frame counts from the `_clips.json`
sidecar rather than the .glb. A stale count runs an action past the end of its
clip. The sidecar's `start_frame` values cannot be recovered from a .glb at all
-- they come from the decomp's animation headers, and eighteen clips have a
non-zero one -- so they are carried across from the previous sidecar rather
than regenerated.

Usage:
    python3 tools/adopt_blender_export.py /mnt/c/Users/.../mario_export.glb \\
        --out assets/mario/mario.glb --sidecar assets/mario/mario_clips.json
"""

import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import rig  # noqa: E402

# The node the decomp exporter parents both the joints and the mesh to.
SKELETON_ROOT = "armature"


def mesh_nodes(gltf):
    return [i for i, n in enumerate(gltf.json["nodes"]) if "mesh" in n]


def reparent_mesh(gltf):
    """Put the skinned mesh back under the skeleton root."""
    if SKELETON_ROOT not in gltf.index:
        raise KeyError(f"no {SKELETON_ROOT!r} node -- is this a mario export?")
    skeleton = gltf.index[SKELETON_ROOT]

    moved = []
    for mesh in mesh_nodes(gltf):
        parent = gltf.parent.get(mesh)
        if parent == skeleton:
            continue                      # already where it belongs
        if parent is not None:
            children = gltf.json["nodes"][parent].get("children", [])
            if mesh in children:
                children.remove(mesh)
        gltf.json["nodes"][skeleton].setdefault("children", []).append(mesh)
        moved.append((gltf.json["nodes"][mesh].get("name"),
                      gltf.json["nodes"][parent].get("name") if parent is not None
                      else "(scene root)"))
    return moved


def clip_lengths(gltf):
    """Frame count per clip, read back off the animation samplers."""
    out = {}
    for anim in gltf.json.get("animations", []):
        longest = 0.0
        for sampler in anim["samplers"]:
            longest = max(longest, float(gltf.read(sampler["input"])[-1, 0]))
        out[anim["name"]] = round(longest * rig.FRAME_RATE) + 1
    return out


def resync_sidecar(path, lengths):
    """Update frame counts, keeping everything a .glb cannot tell us."""
    with open(path, "r", encoding="utf-8") as fh:
        clips = json.load(fh)

    changed = []
    for name, frames in sorted(lengths.items()):
        entry = clips.setdefault(name, {"frames": frames, "start_frame": 0,
                                        "loop_start": 0, "loop_end": frames})
        if entry["frames"] == frames:
            continue
        changed.append((name, entry["frames"], frames))
        entry["frames"] = frames
        entry["loop_end"] = min(entry.get("loop_end", frames) or frames, frames)
        entry["start_frame"] = min(entry.get("start_frame", 0), max(frames - 1, 0))

    with open(path, "w", encoding="utf-8") as fh:
        json.dump(clips, fh, indent=2, sort_keys=True)
    return changed, clips


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("export", help="the .glb Blender wrote")
    parser.add_argument("--out", default=os.path.join(
        os.path.dirname(HERE), "assets", "mario", "mario.glb"))
    parser.add_argument("--sidecar", default=None,
                        help="defaults to <out>_clips.json")
    args = parser.parse_args(argv[1:])

    sidecar = args.sidecar or os.path.splitext(args.out)[0] + "_clips.json"

    gltf = rig.Gltf(args.export)
    moved = reparent_mesh(gltf)
    lengths = clip_lengths(gltf)
    gltf.write(args.out)

    changed, clips = resync_sidecar(sidecar, lengths)

    for name, was in moved:
        print(f"reparented {name!r} from {was} under {SKELETON_ROOT!r}")
    if not moved:
        print("mesh was already under the skeleton root")
    print(f"{len(lengths)} clips, {len(clips)} sidecar entries")
    for name, old, new in changed:
        print(f"  {name}: {old} -> {new} frames")
    size = os.path.getsize(args.out)
    print(f"wrote {args.out} ({size / 1024:.0f} KB) and {sidecar}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
