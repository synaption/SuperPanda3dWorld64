#!/usr/bin/env python3
"""Export every Blender-authored runtime actor to the game's GLBs.

    python3 tools/export_all_actors.py
    python3 tools/export_all_actors.py --blender /path/to/blender

Animated actors pass through ``adopt_blender_export.py`` so Blender's extra
armature wrapper is removed and their clip metadata is refreshed. Outputs are
staged in a temporary directory and replace runtime assets only after the
corresponding export succeeds.
"""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools"
# Actor -> its clip sidecar, or None where the actor does not animate.
ACTORS = {
    "goomba": "goomba_clips.json",
    "scuttlebug": "scuttlebug_clips.json",
    "slime": "slime_clips.json",
    "tree": None,
    "warp_pipe": None,
}

# The node an actor's skinned mesh belongs under, where it is not the decomp
# exporter's ``armature``. The slime was authored in Blender and names its
# armature after itself.
SKELETON_ROOTS = {
    "slime": "Slime_Rig",
}


def run(command):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--blender", help="Blender executable")
    args = parser.parse_args(argv)
    actor_dir = ROOT / "assets" / "actors"

    with tempfile.TemporaryDirectory(prefix="mario-actor-export-") as temp:
        staging = Path(temp)
        for actor, sidecar_name in ACTORS.items():
            blend = actor_dir / f"{actor}.blend"
            if not blend.is_file():
                raise SystemExit(f"missing Blender source: {blend}")

            raw = staging / f"{actor}-raw.glb"
            export = [sys.executable, TOOLS / "blend_to_glb.py", blend,
                      "--out", raw]
            if args.blender:
                export.extend(["--blender", args.blender])
            run(export)

            runtime = actor_dir / f"{actor}.glb"
            if sidecar_name:
                source_sidecar = actor_dir / sidecar_name
                staged_sidecar = staging / sidecar_name
                staged_runtime = staging / f"{actor}.glb"
                shutil.copy2(source_sidecar, staged_sidecar)
                adopt = [sys.executable, TOOLS / "adopt_blender_export.py", raw,
                         "--out", staged_runtime, "--sidecar", staged_sidecar]
                if actor in SKELETON_ROOTS:
                    adopt.extend(["--skeleton-root", SKELETON_ROOTS[actor]])
                run(adopt)
                os.replace(staged_runtime, runtime)
                os.replace(staged_sidecar, source_sidecar)
            else:
                os.replace(raw, runtime)
            print(f"updated {runtime.relative_to(ROOT)}")

    print(f"exported {len(ACTORS)} actors")


if __name__ == "__main__":
    main()
