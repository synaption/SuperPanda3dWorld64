"""Rebuild assets/hero/hero.glb from assets/hero/TheHero.blend, headless.

    python3 tools/build_hero.py

This is the one to run after editing the Hero in Blender. A plain export is
not enough and, worse, not obviously wrong -- tools/blend_to_glb.py will
happily write a .glb from TheHero.blend that loads in the game as a black,
unanimated statue. Two separate stages stand between the .blend and something
the game can use, and this runs both:

**The export has to know about the rig** (tools/export_hero_gltf.py): the
armature ships in REST pose position, so every clip otherwise exports as the
bind pose held still; Rigify's 240 bones have to be cut to the 53 that deform;
and the `Eyes` object holds its own action that would come out as a phantom
21st animation. Without `export_def_bones` the file also drags in ~230 WGT-
widget objects as scene roots.

**The .glb has to be adopted** (tools/adopt_blender_export.py): Blender writes
the Hero's Emission-node material as emissive-only, with a black base colour,
which Panda3D renders as a flat silhouette; and the clip frame counts in
hero_clips.json have to be resynced, since the game reads timing from the
sidecar rather than from the .glb.

**The clips have to be pinned to the spot** (tools/lock_root_motion.py): they
were authored on a stage, so `Attack 2` alone carries 2.8 units of forward
lunge. Left in, the character slides across the ground and snaps back on every
transition while his physics position stays put. This stage is easy to forget
because a file missing it looks perfectly correct in a model viewer.

The Blender in WSL must be 5.x -- TheHero.blend is saved by 5.x and 4.x cannot
open it at all. The newest one found wins; see blend_to_glb.resolve_blender.
"""

import argparse
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import adopt_blender_export  # noqa: E402
import lock_root_motion  # noqa: E402
from blend_to_glb import resolve_blender  # noqa: E402

BLEND = os.path.join(ROOT, "assets", "hero", "TheHero.blend")
OUT = os.path.join(ROOT, "assets", "hero", "hero.glb")
SIDECAR = os.path.join(ROOT, "assets", "hero", "hero_clips.json")

# Rigify names the Hero's armature `rig`; adopt_blender_export defaults to
# `armature`, which is what the decomp exporter emits for everyone else.
SKELETON_ROOT = "rig"


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--blend", default=BLEND)
    ap.add_argument("--out", default=OUT)
    ap.add_argument("--sidecar", default=SIDECAR)
    ap.add_argument("--blender", default=os.environ.get("BLENDER"))
    ap.add_argument("--keep-raw", action="store_true",
                    help="keep the intermediate un-adopted .glb for inspection")
    ap.add_argument("--no-lock", action="store_true",
                    help="leave the authored root motion in the clips")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args(argv[1:])

    if not os.path.isfile(args.blend):
        sys.exit("no such file: %s" % args.blend)

    blender, version = resolve_blender(args.blender)
    if version and version[0] < 5:
        sys.exit("%s is %s; TheHero.blend is a 5.x file and needs Blender 5.x"
                 % (blender, ".".join(map(str, version))))
    print("blender: %s (%s)" % (blender, ".".join(map(str, version or ["?"]))))

    raw_dir = tempfile.mkdtemp(prefix="hero-export-")
    raw = os.path.join(raw_dir, "hero_raw.glb")

    cmd = [blender, "--background", "--factory-startup", args.blend,
           "--python", os.path.join(HERE, "export_hero_gltf.py"),
           "--python-exit-code", "1", "--", "--out", raw]
    proc = subprocess.run(cmd, capture_output=not args.verbose, text=True)
    if proc.returncode != 0 or not os.path.exists(raw):
        if not args.verbose:
            sys.stderr.write((proc.stdout or "") + (proc.stderr or ""))
        sys.exit("export failed (blender exit %d)" % proc.returncode)
    print("exported %s (%.0f KB)" % (raw, os.path.getsize(raw) / 1024))

    # Same module the Windows-side workflow calls, so the two paths cannot
    # produce differently-adopted files.
    rc = adopt_blender_export.main([
        "adopt", raw, "--out", args.out, "--sidecar", args.sidecar,
        "--skeleton-root", SKELETON_ROOT,
    ])
    if rc:
        sys.exit("adopt failed")

    if not args.no_lock:
        rc = lock_root_motion.main(["lock", args.out])

    if args.keep_raw:
        print("raw export kept at %s" % raw)
    else:
        os.remove(raw)
        os.rmdir(raw_dir)
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
