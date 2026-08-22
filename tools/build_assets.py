#!/usr/bin/env python3
"""Build every asset the game loads, out of the Blender sources.

    python3 tools/build_assets.py
    python3 tools/build_assets.py --blender /path/to/blender
    python3 tools/build_assets.py --only actors --only impostors

One entry point, because the pipeline's failure mode is a step that gets
skipped rather than a step that goes wrong. The impostor sheets are the reason
this exists: they are baked by the *game* rather than by Blender, so no
Blender-facing tool ever touched them, and an actor whose model was re-exported
without them was drawn two different ways at once -- the new model up close and
the old picture of it past `enemy_draw`. Rotating an actor is the case that
shows it worst, since every sprite in the atlas then faces the wrong way.

The five stages, in the order they have to run:

    mario      assets/mario/mario.glb        + mario_clips.json
    hero       assets/hero/hero.glb          + hero_clips.json
    castle     assets/bevy/castle.glb, castle.bin, water.png
    actors     assets/actors/*.glb           + a clips sidecar each
    impostors  assets/impostors/*.png, *.json

`impostors` runs last and depends on `actors`: it renders the actor GLBs this
script just wrote. It also needs `cargo`, since the baker runs inside the game
so that its sprites are lit by the same material the skinned models are.

Weapons are excluded -- they are authoring resources the game does not load --
and so are the retired goomba and scuttlebug sources, which are kept as
sources for reference rather than exported.

Every runtime asset here also has a committed Blender source. Run
`python3 tools/build_blender_sources.py --check` to audit that invariant.
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
ACTOR_DIR = ROOT / "assets" / "actors"

# Actor -> its clip sidecar, or None where the actor does not animate.
ACTORS = {
    "ant": "ant_clips.json",
    "slime": "slime_clips.json",
    "tree": None,
    "warp_pipe": None,
}

# The node an actor's skinned mesh belongs under, where it is not the decomp
# exporter's ``armature``. Both Blender-authored actors name theirs themselves:
# the slime after itself, the ant after the object Blender made for it.
SKELETON_ROOTS = {
    "ant": "Armature",
    "slime": "Slime_Rig",
}

STAGES = ("mario", "hero", "castle", "actors", "impostors")


def run(command):
    print("+", " ".join(map(str, command)), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def blend_to_glb(blend, output, blender):
    command = [sys.executable, TOOLS / "blend_to_glb.py", blend, "--out", output]
    if blender:
        command.extend(["--blender", blender])
    run(command)


def adopt(raw, output, sidecar, skeleton_root=None):
    """Normalise a Blender export and resync its clip sidecar.

    Blender's exporter hangs the skinned mesh beside the skeleton rather than
    under it, and can drop a clip's last frame. See adopt_blender_export.py.
    """
    command = [sys.executable, TOOLS / "adopt_blender_export.py", raw,
               "--out", output, "--sidecar", sidecar]
    if skeleton_root:
        command.extend(["--skeleton-root", skeleton_root])
    run(command)


def staged(staging, source, name):
    """A copy of an existing sidecar to work on, so a failure leaves the real
    one alone.

    The sidecar is edited in place rather than regenerated: its `start_frame`
    values come from the decomp's animation headers and cannot be recovered
    from a .glb at all.
    """
    copy = staging / name
    shutil.copy2(source, copy)
    return copy


def build_mario(staging, blender):
    """Mario takes the same armature-wrapper normalisation as the actors."""
    raw = staging / "mario-raw.glb"
    out = staging / "mario.glb"
    sidecar = staged(staging, ROOT / "assets/mario/mario_clips.json",
                     "mario_clips.json")
    blend_to_glb(ROOT / "assets/mario/mario.blend", raw, blender)
    adopt(raw, out, sidecar)
    os.replace(out, ROOT / "assets/mario/mario.glb")
    os.replace(sidecar, ROOT / "assets/mario/mario_clips.json")
    return ["assets/mario/mario.glb"]


def build_hero(staging, blender):
    """The Hero has his own Rigify export, root-motion and aiming passes."""
    out = staging / "hero.glb"
    sidecar = staged(staging, ROOT / "assets/hero/hero_clips.json",
                     "hero_clips.json")
    command = [sys.executable, TOOLS / "build_hero.py",
               "--out", out, "--sidecar", sidecar]
    if blender:
        command.extend(["--blender", blender])
    run(command)
    os.replace(out, ROOT / "assets/hero/hero.glb")
    os.replace(sidecar, ROOT / "assets/hero/hero_clips.json")
    return ["assets/hero/hero.glb"]


def build_castle(_staging, _blender):
    """The one stage that does not come out of Blender, deliberately.

    `assets/bevy/castle_grounds.blend` exists and opens, and exporting it
    produces a castle that looks wrong in two ways at once. It loses
    `KHR_materials_unlit`, which every one of the level's 45 materials carries
    and `n64::translate` reads: that mesh's lighting was resolved offline and
    baked into its vertex colours, so a castle without the flag is lit a second
    time on top of the light already painted into it. And it gains
    `alphaMode: BLEND` on all 45, which turns the entire level into sorted
    draws. Neither shows up as an error anywhere.

    So the castle is built by the tool that actually produces what the game
    loads. It reads the committed NPZs under `assets/castle_grounds/` -- the
    decomp's geometry, parsed once -- and writes all three of the runtime
    files together, reproducibly. The .blend stays an authoring copy for
    looking at and editing by hand; it is not a source this can build from.
    """
    run([sys.executable, TOOLS / "convert_level.py"])
    return ["assets/bevy/castle.glb", "assets/bevy/castle.bin",
            "assets/bevy/water.png"]


def build_actors(staging, blender):
    built = []
    for actor, sidecar_name in ACTORS.items():
        blend = ACTOR_DIR / f"{actor}.blend"
        if not blend.is_file():
            raise SystemExit(f"missing Blender source: {blend}")

        raw = staging / f"{actor}-raw.glb"
        blend_to_glb(blend, raw, blender)

        runtime = ACTOR_DIR / f"{actor}.glb"
        if sidecar_name:
            sidecar = staged(staging, ACTOR_DIR / sidecar_name, sidecar_name)
            out = staging / f"{actor}.glb"
            adopt(raw, out, sidecar, SKELETON_ROOTS.get(actor))
            os.replace(out, runtime)
            os.replace(sidecar, ACTOR_DIR / sidecar_name)
        else:
            os.replace(raw, runtime)
        built.append(str(runtime.relative_to(ROOT)))
    return built


def bake_impostors(_staging, _blender):
    """Re-render the far crowd's sprite atlases from the actors just built.

    Named no kinds, the baker does every enemy that has a sheet, which keeps
    the list of them in `enemy::Kind` where it belongs rather than copied here
    where it would rot.

    Expect the PNGs to come back very slightly different every time even when
    nothing changed -- a few dozen pixels of a four-million-pixel sheet, none
    of them off by more than a step or two. It is a GPU render, not a
    calculation, so it is not bit-reproducible the way the exports above are.
    """
    run(["cargo", "run", "--release", "--", "bake-impostors"])
    return sorted(
        str(path.relative_to(ROOT))
        for path in (ROOT / "assets" / "impostors").glob("*.png")
    )


BUILDERS = {
    "mario": build_mario,
    "hero": build_hero,
    "castle": build_castle,
    "actors": build_actors,
    "impostors": bake_impostors,
}


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--blender", help="Blender executable")
    parser.add_argument("--only", action="append", choices=STAGES, metavar="STAGE",
                        help=f"run just this stage ({', '.join(STAGES)}); "
                             "repeatable, and order is fixed regardless")
    args = parser.parse_args(argv)

    # Fixed order rather than the order they were asked for: `impostors` bakes
    # the actor GLBs, so running it first would bake the previous ones.
    wanted = [stage for stage in STAGES if not args.only or stage in args.only]

    built = []
    with tempfile.TemporaryDirectory(prefix="mario-assets-") as temp:
        staging = Path(temp)
        for stage in wanted:
            print(f"\n=== {stage} ===", flush=True)
            built.extend(BUILDERS[stage](staging, args.blender))

    print("\nbuilt:")
    for path in built:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
