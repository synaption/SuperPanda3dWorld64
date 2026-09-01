#!/usr/bin/env python3
"""Build every asset the game loads, out of the Blender sources.

    python3 tools/build_assets.py
    python3 tools/build_assets.py --blender /path/to/blender
    python3 tools/build_assets.py --only actors --only impostors
    python3 tools/build_assets.py --force

One entry point, because the pipeline's failure mode is a step that gets
skipped rather than a step that goes wrong. The impostor sheets are the reason
this exists: they are baked by the *game* rather than by Blender, so no
Blender-facing tool ever touched them, and an actor whose model was re-exported
without them was drawn two different ways at once -- the new model up close and
the old picture of it past `enemy_draw`. Rotating an actor is the case that
shows it worst, since every sprite in the atlas then faces the wrong way.

The eight stages, in the order they have to run:

    mario      assets/mario/mario.glb        + mario_clips.json
    luna       assets/luna/luna.glb          + luna_clips.json
    weapons    assets/weapons/*.glb          what Luna carries
    castle     assets/bevy/castle.glb, castle.bin, water.png
    levels     assets/bevy/*_furniture.json, *.glb   what is placed in each
    planet     assets/bevy/planet.glb        the generated planet, LOD0
    actors     assets/actors/*.glb           + a clips sidecar each
    impostors  assets/impostors/*.png, *.json

`impostors` runs last and depends on `actors`: it renders the actor GLBs this
script just wrote. It also needs `cargo`, since the baker runs inside the game
so that its sprites are lit by the same material the skinned models are.

The retired goomba and scuttlebug sources are excluded: they are kept as
sources for reference rather than exported.

Every runtime asset here also has a committed Blender source. Run
`python3 tools/build_blender_sources.py --check` to audit that invariant.

# What actually runs

A stage is a list of *units* -- one actor, one weapon, one impostor sheet --
and a unit is skipped when nothing it is built from has changed since the last
time it was built. Each one records the SHA-256 of every file it read and every
file it wrote into `.build_assets.json` at the repository root, and runs again
when any of those differs, is missing, or was never recorded at all.

Content and not timestamps, which is what makes the chain work: the Blender
exports are reproducible, so re-exporting an unedited actor writes the same
bytes back and the impostor sheet baked from it correctly stays skipped.
Touching a .blend is therefore free, and editing one costs only what actually
depends on it.

`--force` runs everything anyway. Reach for it if you suspect the stamps are
lying -- and if they were, that is a bug in the input list here rather than
something to work around twice.

The full pipeline is dominated by the impostor bake: on this machine it is
about 2m45s of a 3m40s run, and everything else together is under forty
seconds. So the thing worth getting right is not rebaking sheets whose
actor did not move, which is why an impostor unit lists both the actor's .glb
and the sources that decide how it is drawn.
"""

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Callable


ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools"
SRC = ROOT / "src"
ACTOR_DIR = ROOT / "assets" / "actors"
WEAPON_DIR = ROOT / "assets" / "weapons"
IMPOSTOR_DIR = ROOT / "assets" / "impostors"

# Where the stamps live. At the root rather than under `assets/`, because it is
# not an asset: it says nothing about the game and everything about this
# machine's last run, and a clone that has never built anything is supposed to
# arrive without one.
STAMPS = ROOT / ".build_assets.json"

# Levels with a furniture file. The planet has none yet: its ground is
# generated rather than authored, and where its gravity points is still
# `world::PLANET_CENTRE`.
LEVELS = ("castle",)

# What Luna can hold. Static props rather than actors: no rig, no clips,
# and so no adoption pass -- the export is the runtime file. Each one is
# authored with its grip on the origin and its bore down +Z, which is what
# lets `weapon::follow_socket` place it from the hand joint alone. See
# docs/pipeline.md, "Weapons".
WEAPONS = ("target_pistol",)

# Actor -> its clip sidecar, or None where the actor does not animate.
ACTORS = {
    "ant": "ant_clips.json",
    "pylon": None,
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

# What a sheet is a picture of, beyond the actor itself: the chain that draws
# it. `main::drawing` is run by the baker verbatim -- that is the whole point of
# it being a named chain -- so a change to any of these is a change to what
# comes out of the camera, and the sheet has to be baked again even though no
# .blend was touched.
#
# A judgment call, and deliberately a short list rather than "the binary":
# stamping the executable would rebake every sheet after an edit to the player's
# jump. If a rendering change ever does slip past this list, `--force` is the
# answer and the list is what to fix.
RENDER_SOURCES = (
    SRC / "impostor" / "bake.rs",   # the baker itself
    SRC / "impostor.rs",            # the sheet's material and its sidecar
    SRC / "billboard.rs",           # the first third of `main::drawing`
    SRC / "n64.rs",                 # the last third, and the lighting
    SRC / "n64.wgsl",               # what that material compiles to
    SRC / "enemy.rs",               # which clip is baked, and from which model
    SRC / "main.rs",                # `drawing` itself
    SRC / "console.rs",             # `GameTuning`, which the draw chain reads
)

STAGES = ("mario", "luna", "weapons", "castle", "levels", "planet",
          "actors", "impostors")


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


# ---------------------------------------------------------------------------
# The units
# ---------------------------------------------------------------------------

@dataclass
class Unit:
    """One thing that gets built, and everything that decides whether it has to.

    `inputs` and `outputs` are both stamped, and for different reasons. Inputs
    catch the ordinary case -- a .blend was edited, so rebuild. Outputs catch
    the rest: an asset deleted, reverted by git, or written over by hand is one
    this script would otherwise cheerfully report as up to date.
    """

    name: str
    build: Callable
    inputs: tuple = ()
    outputs: tuple = ()
    # Anything that changes the output without being a file: the list a loop
    # runs over, the flags a tool is given.
    config: str = ""
    # Whether Blender's version is part of what this unit is built from. Two
    # Blenders do not write the same .glb, and the stamp has to know it.
    blender: bool = False


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


def plan_mario():
    return [Unit(
        name="mario",
        build=build_mario,
        # The sidecar is an input as well as a thing this rewrites: `adopt`
        # resyncs the frame counts in the copy it is handed and keeps
        # everything else, so a hand-edited `start_frame` is an edit that has
        # to be picked up.
        inputs=(ROOT / "assets/mario/mario.blend",
                ROOT / "assets/mario/mario_clips.json",
                TOOLS / "blend_to_glb.py",
                TOOLS / "adopt_blender_export.py"),
        outputs=(ROOT / "assets/mario/mario.glb",),
        blender=True,
    )]


def build_luna(staging, blender):
    """Luna has his own Rigify export, root-motion and aiming passes."""
    out = staging / "luna.glb"
    sidecar = staged(staging, ROOT / "assets/luna/luna_clips.json",
                     "luna_clips.json")
    command = [sys.executable, TOOLS / "build_luna.py",
               "--out", out, "--sidecar", sidecar]
    if blender:
        command.extend(["--blender", blender])
    run(command)
    os.replace(out, ROOT / "assets/luna/luna.glb")
    os.replace(sidecar, ROOT / "assets/luna/luna_clips.json")


def plan_luna():
    return [Unit(
        name="luna",
        build=build_luna,
        # Four tools rather than one: build_luna.py is a driver, and the export,
        # the adoption, the aim rig and the root-motion lock each live in their
        # own file. See that script's docstring for what each does.
        inputs=(ROOT / "assets/luna/Luna.blend",
                ROOT / "assets/luna/luna_clips.json",
                TOOLS / "build_luna.py",
                TOOLS / "export_luna_gltf.py",
                TOOLS / "adopt_blender_export.py",
                TOOLS / "aim_rig.py",
                TOOLS / "lock_root_motion.py",
                TOOLS / "blend_to_glb.py"),
        outputs=(ROOT / "assets/luna/luna.glb",),
        blender=True,
    )]


def plan_weapons():
    """Luna's weapons: a plain mesh export each, straight to the runtime file.

    No adoption pass and no sidecar. A weapon has no skeleton to normalise and
    no clips to name, so `blend_to_glb` already writes exactly what the game
    loads. The empties the .blend carries -- `MUZZLE` where the shot leaves the
    barrel, `GRIP` where the hand takes hold -- survive the export as ordinary
    childless nodes, which is how the runtime finds them.
    """
    units = []
    for weapon in WEAPONS:
        blend = WEAPON_DIR / f"{weapon}.blend"
        runtime = WEAPON_DIR / f"{weapon}.glb"

        def build(staging, blender, blend=blend, runtime=runtime, weapon=weapon):
            if not blend.is_file():
                raise SystemExit(f"missing Blender source: {blend}")
            raw = staging / f"{weapon}.glb"
            blend_to_glb(blend, raw, blender)
            os.replace(raw, runtime)

        units.append(Unit(name=f"weapons:{weapon}", build=build,
                          inputs=(blend, TOOLS / "blend_to_glb.py"),
                          outputs=(runtime,), blender=True))
    return units


def build_castle(_staging, blender):
    """Build NPZ castle geometry at its Blender-authored dimensions.

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
    decomp's geometry, parsed once -- and writes all three runtime files
    together. The mesh still does not come out of Blender, but its world-space
    bounds do: one metre in the authoring copy is one metre in the game, and
    applying object scale back to one does not change the generated size.
    """
    command = [sys.executable, TOOLS / "convert_level.py"]
    if blender:
        command.extend(["--blender", blender])
    run(command)


def plan_castle():
    grounds = ROOT / "assets" / "castle_grounds"
    return [Unit(
        name="castle",
        build=build_castle,
        # The water texture is the one input that lives in `reference/`, which
        # is not in the repository. Stamped all the same: absent is a state
        # like any other here, and a run made with the pack present is not the
        # same run as one made without it.
        inputs=(grounds / "mesh.npz",
                grounds / "collision.npz",
                grounds / "collision_objects.json",
                grounds / "mesh_materials.json",
                ROOT / "assets/bevy/castle_grounds.blend",
                ROOT / "reference/RENDER96-HD-TEXTURE-PACK/gfx/textures/"
                       "segment2/segment2.11C58.rgba16.png",
                TOOLS / "convert_level.py",
                TOOLS / "blend_to_glb.py",
                TOOLS / "glb.py"),
        outputs=(ROOT / "assets/bevy/castle.glb",
                 ROOT / "assets/bevy/castle.bin",
                 ROOT / "assets/bevy/water.png"),
        blender=True,
    )]


def plan_levels():
    """A level's furniture: where everything in it is, as placed in Blender.

    The stage the `castle` one above could not be. That stage's problem is the
    45 materials on the level mesh, and furniture has no materials -- it is
    empties and a waterfall -- so it comes out of a .blend like everything else
    in this script.

    The source is `assets/levels/<level>.blend`, which links the level's own
    geometry in as a backdrop to place against and exports none of it. Water,
    warp pipes and what each produces, the enemies standing about, the spawn
    point and which way gravity points were all literals in the Rust until
    this existed.
    """
    units = []
    for level in LEVELS:
        def build(_staging, blender, level=level):
            command = [sys.executable, TOOLS / "export_level_furniture.py", level]
            if blender:
                command.extend(["--blender", blender])
            run(command)

        # The .blend links the level mesh and the actors in as a backdrop, and
        # exports none of them: what comes out is the empties and the
        # waterfall, so the libraries are not inputs to stamp.
        units.append(Unit(
            name=f"levels:{level}", build=build,
            inputs=(ROOT / f"assets/levels/{level}.blend",
                    TOOLS / "export_level_furniture.py"),
            outputs=(ROOT / f"assets/bevy/{level}_furniture.json",
                     ROOT / f"assets/bevy/{level}_furniture.glb"),
            blender=True))
    return units


def build_planet(_staging, _blender):
    """Adopt the generated planet from `experimental/planet_gen`.

    A copy and not a build, and that is the honest description of it. The
    generator is its own program with its own Blender pass -- see that
    directory's readme -- and re-running it here would mean this script owning
    a planet's worth of parameters it has no opinion about. What it does own is
    the invariant that everything under `assets/` is derived from something
    committed and can be produced again, so the copy is written down here
    rather than being a thing somebody once did by hand.

    Regenerate the source with, from `experimental/planet_gen`:

        python3 -m planetgen.cli build
        blender --background --factory-startup --python blender/export_tiles.py

    LOD0 is what the game takes. The collision it stands on is read out of this
    same mesh at load time, so a LOD1 planet would be a planet whose ground is
    a few metres from where it is drawn.
    """
    source = ROOT / "experimental/planet_gen/out/planet.glb"
    if not source.is_file():
        raise SystemExit(
            f"missing generated planet: {source}\n"
            "run planetgen's build and export first; see its readme")
    shutil.copy2(source, ROOT / "assets/bevy/planet.glb")


def plan_planet():
    return [Unit(
        name="planet",
        build=build_planet,
        inputs=(ROOT / "experimental/planet_gen/out/planet.glb",),
        outputs=(ROOT / "assets/bevy/planet.glb",),
    )]


def plan_actors():
    units = []
    for actor, sidecar_name in ACTORS.items():
        blend = ACTOR_DIR / f"{actor}.blend"
        runtime = ACTOR_DIR / f"{actor}.glb"
        sidecar = ACTOR_DIR / sidecar_name if sidecar_name else None

        def build(staging, blender, actor=actor, blend=blend, runtime=runtime,
                  sidecar_name=sidecar_name):
            if not blend.is_file():
                raise SystemExit(f"missing Blender source: {blend}")
            raw = staging / f"{actor}-raw.glb"
            blend_to_glb(blend, raw, blender)
            if sidecar_name:
                copy = staged(staging, ACTOR_DIR / sidecar_name, sidecar_name)
                out = staging / f"{actor}.glb"
                adopt(raw, out, copy, SKELETON_ROOTS.get(actor))
                os.replace(out, runtime)
                os.replace(copy, ACTOR_DIR / sidecar_name)
            else:
                os.replace(raw, runtime)

        inputs = [blend, TOOLS / "blend_to_glb.py"]
        if sidecar:
            inputs.extend([sidecar, TOOLS / "adopt_blender_export.py"])
        units.append(Unit(
            name=f"actors:{actor}", build=build,
            inputs=tuple(inputs), outputs=(runtime,),
            # The skeleton root is a flag handed to the adoption pass, so it
            # belongs to what this was built from as much as the .blend does.
            config=repr(SKELETON_ROOTS.get(actor)),
            blender=True))
    return units


def sheet_kinds():
    """The actors with an impostor sheet, read out of `enemy::KINDS`.

    Read rather than listed, and the difference matters. `bake-impostors` with
    no arguments does every kind that has a sheet, which is what kept that list
    in `enemy::Kind` where it belongs; baking them one at a time so that an
    unchanged one can be skipped means knowing the list here, and a copy of it
    in this file is a copy that rots -- a new enemy would silently never get a
    sheet, which is the exact failure this whole script exists to prevent.

    So it is parsed from the Rust. If the parse ever comes back empty -- the
    declaration reformatted, moved, renamed -- the caller bakes the whole lot in
    one go, which is slower and always right.
    """
    try:
        source = (SRC / "enemy.rs").read_text()
    except OSError:
        return []
    match = re.search(r"pub const KINDS:\s*\[Kind;\s*\d+\]\s*=\s*\[([^\]]*)\]",
                      source)
    if not match:
        return []
    return [name.lower() for name in re.findall(r"Kind::(\w+)", match.group(1))]


def bake(kinds):
    """Re-render the far crowd's sprite atlases from the actors just built.

    Expect the PNGs to come back very slightly different every time even when
    nothing changed -- a few dozen pixels of a four-million-pixel sheet, none
    of them off by more than a step or two. It is a GPU render, not a
    calculation, so it is not bit-reproducible the way the exports above are.
    That is exactly why the stamp is taken over what a sheet was baked *from*
    and not over the sheet: comparing the sheets themselves would rebake all of
    them, for ever, on the strength of that handful of pixels.
    """
    run(["cargo", "run", "--release", "--", "bake-impostors", *kinds])


def plan_impostors():
    kinds = sheet_kinds()
    if not kinds:
        # The list could not be read, so fall back to what this always did:
        # every sheet, every time, named by nobody.
        return [Unit(name="impostors",
                     build=lambda _staging, _blender: bake([]),
                     inputs=tuple(ACTOR_DIR.glob("*.glb")) + RENDER_SOURCES,
                     outputs=tuple(sorted(IMPOSTOR_DIR.glob("*.png"))))]
    units = []
    for kind in kinds:
        # `Kind::model` and `impostor::stem` agree that both are the kind's own
        # name in lower case, which is what makes one string enough here.
        units.append(Unit(
            name=f"impostors:{kind}",
            build=lambda _staging, _blender, kind=kind: bake([kind]),
            inputs=(ACTOR_DIR / f"{kind}.glb",) + RENDER_SOURCES,
            outputs=(IMPOSTOR_DIR / f"{kind}.png",
                     IMPOSTOR_DIR / f"{kind}.json")))
    return units


PLANS = {
    "mario": plan_mario,
    "luna": plan_luna,
    "weapons": plan_weapons,
    "castle": plan_castle,
    "levels": plan_levels,
    "planet": plan_planet,
    "actors": plan_actors,
    "impostors": plan_impostors,
}


# ---------------------------------------------------------------------------
# Stamps
# ---------------------------------------------------------------------------

def digest(path):
    """A file's SHA-256, or None where there is no such file.

    Missing is a value rather than an error: `reference/` is not in the
    repository, so the castle's water texture is legitimately absent on most
    machines -- and a build made without it is still not the same build as one
    made with it.
    """
    sha = hashlib.sha256()
    try:
        with open(path, "rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                sha.update(block)
    except OSError:
        return None
    return sha.hexdigest()


def fingerprint(unit, blender_version):
    def digests(paths):
        return {str(Path(path).relative_to(ROOT)): digest(path) for path in paths}

    return {
        "config": unit.config,
        "blender": blender_version if unit.blender else "",
        "inputs": digests(unit.inputs),
        "outputs": digests(unit.outputs),
    }


def why(unit, recorded, current):
    """Why this unit has to be built, in a few words, or None if it does not."""
    if recorded is None:
        return "never built"
    if recorded.get("config") != current["config"]:
        return "built differently now"
    if recorded.get("blender") != current["blender"]:
        return f"Blender {recorded.get('blender') or '?'} -> {current['blender']}"
    for role in ("inputs", "outputs"):
        was, now = recorded.get(role) or {}, current[role]
        if was.keys() != now.keys():
            return f"its {role} are not the ones it was built from"
        for path, stamp in was.items():
            if now[path] == stamp:
                continue
            if now[path] is None:
                return f"{path} is gone"
            if stamp is None:
                return f"{path} is here now"
            return f"{path} changed"
    return None


def read_stamps():
    try:
        stamps = json.loads(STAMPS.read_text())
    except (OSError, ValueError):
        return {}
    # A stamp file from a version that recorded something else means nothing
    # here, and guessing at it is how a build gets wrongly skipped.
    if stamps.get("version") != 1:
        return {}
    return stamps.get("units") or {}


def write_stamps(units):
    STAMPS.write_text(json.dumps({"version": 1, "units": units},
                                 indent=1, sort_keys=True) + "\n")


def blender_version(explicit):
    """What `resolve_blender` would pick, as a string for the stamps.

    Asked once and only when something actually needs Blender, and it costs a
    `--version` apiece -- under a tenth of a second. Never fatal: a machine
    with no Blender still has stages that do not want one, and the failure
    belongs to the unit that tries to use it rather than to this.
    """
    sys.path.insert(0, str(TOOLS))
    try:
        from blend_to_glb import resolve_blender
        path, version = resolve_blender(explicit)
    except (ImportError, SystemExit):
        return "unknown"
    return "%s %s" % (os.path.basename(path),
                      ".".join(map(str, version or ["?"])))


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--blender", help="Blender executable")
    parser.add_argument("--only", action="append", choices=STAGES, metavar="STAGE",
                        help=f"run just this stage ({', '.join(STAGES)}); "
                             "repeatable, and order is fixed regardless")
    parser.add_argument("--force", action="store_true",
                        help="build every wanted unit, stamps or no stamps")
    args = parser.parse_args(argv)

    # Fixed order rather than the order they were asked for: `impostors` bakes
    # the actor GLBs, so running it first would bake the previous ones.
    wanted = [stage for stage in STAGES if not args.only or stage in args.only]
    units = [unit for stage in wanted for unit in PLANS[stage]()]

    # `kept` is every stamp there is, including the units this run was not
    # asked for: `--only actors` must not throw away what it knows about the
    # rest. `--force` only stops them being *read*, never overwritten.
    kept = read_stamps()
    version = (blender_version(args.blender)
               if any(unit.blender for unit in units) else "")

    built, skipped = [], []
    with tempfile.TemporaryDirectory(prefix="mario-assets-") as temp:
        staging = Path(temp)
        for unit in units:
            recorded = None if args.force else kept.get(unit.name)
            reason = "asked for" if args.force else why(unit, recorded,
                                                       fingerprint(unit, version))
            if reason is None:
                skipped.append(unit.name)
                continue
            print(f"\n=== {unit.name} === ({reason})", flush=True)
            unit.build(staging, args.blender)
            # Stamped from what is on disk *after* the build, not before: the
            # clip sidecars are inputs this rewrites in place, and the sheets
            # are outputs no two bakes agree on to the byte.
            kept[unit.name] = fingerprint(unit, version)
            # Written as each unit lands, so an interrupted run keeps the work
            # it finished. Half a pipeline is a normal thing to come back to.
            write_stamps(kept)
            built.extend(str(Path(path).relative_to(ROOT))
                         for path in unit.outputs)

    if skipped:
        print(f"\nunchanged, skipped: {', '.join(skipped)}")
    print("\nbuilt:" if built else "\nbuilt nothing; everything was up to date")
    for path in built:
        print(f"  {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
