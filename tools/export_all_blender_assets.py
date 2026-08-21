#!/usr/bin/env python3
"""Export every Blender-authored 3D asset currently loaded by the game.

    python3 tools/export_all_blender_assets.py
    python3 tools/export_all_blender_assets.py --blender /path/to/blender

This covers Mario, Hero, castle grounds, Goomba, Scuttlebug, tree, and warp
pipe. Weapon models are authoring resources but are not exported here because
the game does not currently load them.
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


def run(command):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def blender_export(blend, output, blender):
    command = [sys.executable, TOOLS / "blend_to_glb.py", blend,
               "--out", output]
    if blender:
        command.extend(["--blender", blender])
    run(command)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--blender", help="Blender executable")
    args = parser.parse_args(argv)

    with tempfile.TemporaryDirectory(prefix="mario-all-assets-") as temp:
        staging = Path(temp)

        # Mario uses the same armature-wrapper normalization as other actors.
        mario_raw = staging / "mario-raw.glb"
        mario_out = staging / "mario.glb"
        mario_sidecar = staging / "mario_clips.json"
        shutil.copy2(ROOT / "assets/mario/mario_clips.json", mario_sidecar)
        blender_export(ROOT / "assets/mario/mario.blend", mario_raw,
                       args.blender)
        run([sys.executable, TOOLS / "adopt_blender_export.py", mario_raw,
             "--out", mario_out, "--sidecar", mario_sidecar])
        os.replace(mario_out, ROOT / "assets/mario/mario.glb")
        os.replace(mario_sidecar, ROOT / "assets/mario/mario_clips.json")

        # Hero has its own Rigify export, root-motion, and aiming passes.
        hero_out = staging / "hero.glb"
        hero_sidecar = staging / "hero_clips.json"
        shutil.copy2(ROOT / "assets/hero/hero_clips.json", hero_sidecar)
        hero_command = [sys.executable, TOOLS / "build_hero.py",
                        "--out", hero_out, "--sidecar", hero_sidecar]
        if args.blender:
            hero_command.extend(["--blender", args.blender])
        run(hero_command)
        os.replace(hero_out, ROOT / "assets/hero/hero.glb")
        os.replace(hero_sidecar, ROOT / "assets/hero/hero_clips.json")

        # The Blender scene is the visual source. Collision remains in the
        # committed castle.bin produced by convert_level.py.
        castle_out = staging / "castle.glb"
        blender_export(ROOT / "assets/bevy/castle_grounds.blend", castle_out,
                       args.blender)
        os.replace(castle_out, ROOT / "assets/bevy/castle.glb")

    actor_command = [sys.executable, TOOLS / "export_all_actors.py"]
    if args.blender:
        actor_command.extend(["--blender", args.blender])
    run(actor_command)
    print("exported all 7 Blender-authored runtime assets")


if __name__ == "__main__":
    main()
