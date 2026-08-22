"""Export a .blend to .glb from the command line, headless.

    python3 tools/blend_to_glb.py assets/hero/TheHero.blend
    python3 tools/blend_to_glb.py assets/hero/TheHero.blend -o /tmp/hero.glb

Unlike tools/export_hero_gltf.py, which knows the Hero's rig, this drives the
project-local Blender 5.2 install and makes no assumptions about the file.
It is the generic "just give me a glb" path.

The script is run twice. The first run is a normal python3 process, where `import
bpy` fails; it builds a Blender command line and re-invokes itself as the
`--python` argument. The second run is inside Blender, where bpy exists, and does
the export. Keeping both halves in one file means the export options and the
command that produces them cannot drift apart.

    blender --background <file> --python <this file> -- --out <glb>

`--background` is what makes it headless: no window, no GL context, no display
needed, which is the only way it can run under WSL without an X server.
"""

import argparse
import glob
import os
import re
import shutil
import subprocess
import sys

try:
    import bpy
except ImportError:  # Outside Blender -- we are the launcher half.
    bpy = None


HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PROJECT_BLENDER = os.path.join(
    ROOT, "blender-5.2.0-linux-x64", "blender")


# ---------------------------------------------------------------------------
# Inside Blender
# ---------------------------------------------------------------------------

def aim_material_outputs_at_everything():
    """Point every material's active output at ALL, in this session only.

    The glTF exporter only follows a Material Output node targeted at ALL or
    CYCLES. Given an EEVEE-only one it finds no surface, and rather than saying
    so it writes glTF's default material: `baseColorFactor` white,
    `metallicFactor` 1.0. On screen that is a white plastic actor, and nothing
    anywhere in the export prints a warning.

    Old files are how you get one. `assets/actors/ant.blend` came from the
    Blender-Internal era, so Blender rebuilds its material graph on load -- the
    tree is named `Material Node Tree Versioning` -- with an EEVEE-targeted
    output and a stray CYCLES one hanging off an unused Diffuse BSDF. Repairing
    the .blend works until somebody saves over it from a session opened before
    the repair, which is exactly what happened once already.

    So it is done here instead, where it cannot be undone: the *active* output
    is promoted, because that is the one Blender itself renders through and the
    one whose result the author was looking at, and any rival is demoted so the
    choice stays unambiguous. Nothing is written back to the .blend.
    """
    for material in bpy.data.materials:
        tree = material.node_tree
        if tree is None:
            continue
        outputs = [node for node in tree.nodes if node.type == "OUTPUT_MATERIAL"]
        active = next((node for node in outputs if node.is_active_output), None)
        if active is None or active.target == "ALL":
            continue
        for node in outputs:
            if node is not active and node.target == "ALL":
                node.target = "EEVEE"
        active.target = "ALL"
        print("retargeted %r's active output to ALL" % material.name)


def export_inside_blender(argv):
    """Run in the Blender process. argv is everything after the `--`."""
    ap = argparse.ArgumentParser(prog="blend_to_glb (in-blender)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--selection-only", action="store_true")
    ap.add_argument("--no-animations", action="store_true")
    ap.add_argument("--apply-modifiers", action="store_true")
    args = ap.parse_args(argv)

    # Bundled and on by default, but --factory-startup means we cannot rely on
    # the user's preferences having kept it that way.
    import addon_utils
    addon_utils.enable("io_scene_gltf2", default_set=False, persistent=False)

    aim_material_outputs_at_everything()

    # The exporter reads evaluated data; pose or edit mode leaves parts of it
    # stale. A file saved in edit mode opens in edit mode.
    if bpy.context.object and bpy.context.object.mode != "OBJECT":
        bpy.ops.object.mode_set(mode="OBJECT")

    options = dict(
        filepath=args.out,
        export_format="GLB",
        use_selection=args.selection_only,
        # glTF is Y-up, Blender is Z-up. Every consumer of a .glb expects the
        # conversion to have happened.
        export_yup=True,
        export_apply=args.apply_modifiers,
        export_animations=not args.no_animations,
    )
    properties = bpy.ops.export_scene.gltf.get_rna_type().properties
    if "export_armature_object_remove" in properties:
        # Blender 5.2 can omit its implementation-only Armature object node
        # while retaining the actual skin joints and animation hierarchy.
        options["export_armature_object_remove"] = True
    bpy.ops.export_scene.gltf(**options)

    if not os.path.exists(args.out):
        raise SystemExit("exporter reported success but wrote no file")
    print("WROTE %s (%d bytes)" % (args.out, os.path.getsize(args.out)))


# ---------------------------------------------------------------------------
# Outside Blender
# ---------------------------------------------------------------------------

def blender_version(path):
    """(major, minor, patch) as reported by `blender --version`, or None."""
    try:
        out = subprocess.run([path, "--version"], capture_output=True, text=True,
                             timeout=60).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    m = re.search(r"Blender\s+(\d+)\.(\d+)\.(\d+)", out)
    return tuple(int(g) for g in m.groups()) if m else None


def resolve_blender(explicit):
    """The newest Blender we can find, unless one was named outright.

    Version matters more than it looks: a .blend saved by 5.x cannot be opened
    by 4.x at all (the header format changed), and the apt build on noble is
    4.0.2. The snap installs to /snap/bin, which sits *after* /usr/bin on PATH,
    so resolving a bare "blender" would quietly keep picking the older one.
    Hence: collect every candidate, ask each its version, take the highest.
    """
    if explicit:
        found = shutil.which(explicit) or explicit
        if not os.path.isfile(found):
            sys.exit("blender not found: %s" % explicit)
        return found, blender_version(found)

    candidates = [PROJECT_BLENDER, "/snap/bin/blender", shutil.which("blender"),
                  "/usr/local/bin/blender"]
    candidates += sorted(glob.glob("/opt/blender*/blender"))
    seen, best = set(), []
    for path in candidates:
        if not path or not os.path.isfile(path):
            continue
        real = os.path.realpath(path)
        if real in seen:
            continue
        seen.add(real)
        best.append((blender_version(path) or (0, 0, 0), path))
    if not best:
        sys.exit("no blender found (install one, or pass --blender / set $BLENDER)")
    best.sort()
    version, path = best[-1]
    return path, (version if version != (0, 0, 0) else None)


def launch(argv):
    ap = argparse.ArgumentParser(
        prog="blend_to_glb.py",
        description="Export a .blend file to .glb using headless Blender.",
    )
    ap.add_argument("blend", help="path to the .blend file")
    ap.add_argument("-o", "--out",
                    help="output .glb (default: alongside the .blend)")
    ap.add_argument("--blender", default=os.environ.get("BLENDER"),
                    help="blender executable (default: $BLENDER, else newest "
                         "candidate, preferring the project Blender 5.2)")
    ap.add_argument("--selection-only", action="store_true",
                    help="export only what was selected when the file was saved")
    ap.add_argument("--no-animations", action="store_true",
                    help="skip animations")
    ap.add_argument("--apply-modifiers", action="store_true",
                    help="apply modifiers to meshes on export")
    ap.add_argument("-v", "--verbose", action="store_true",
                    help="show Blender's output even when the export succeeds")
    args = ap.parse_args(argv)

    blend = os.path.abspath(args.blend)
    if not os.path.isfile(blend):
        sys.exit("no such file: %s" % blend)

    blender, version = resolve_blender(args.blender)
    if args.verbose:
        print("using %s (%s)" % (blender, ".".join(map(str, version or ["?"]))))

    out = os.path.abspath(args.out) if args.out \
        else os.path.splitext(blend)[0] + ".glb"
    out_dir = os.path.dirname(out)
    if out_dir and not os.path.isdir(out_dir):
        os.makedirs(out_dir, exist_ok=True)

    inner = ["--out", out]
    if args.selection_only:
        inner.append("--selection-only")
    if args.no_animations:
        inner.append("--no-animations")
    if args.apply_modifiers:
        inner.append("--apply-modifiers")

    cmd = [
        blender,
        "--background",
        "-noaudio",
        # Addons and startup scripts from the user's profile can raise on load
        # and take the export down with them; nothing here needs them.
        "--factory-startup",
        blend,
        "--python", os.path.abspath(__file__),
        # Without this, a traceback inside the --python script still exits 0
        # and the failure goes unnoticed by whatever called us.
        "--python-exit-code", "1",
        "--",
    ] + inner

    proc = subprocess.run(cmd, capture_output=not args.verbose, text=True)
    if proc.returncode != 0:
        chatter = (proc.stdout or "") + (proc.stderr or "")
        if not args.verbose:
            sys.stderr.write(chatter)
        # Blender says "not a blend file" for a file it merely cannot read --
        # including one saved by a version newer than itself, which is the
        # likely case here and not at all what that wording suggests.
        if "not a blend file" in chatter:
            sys.stderr.write(
                "\nhint: %s is %s. If this .blend was saved by a newer Blender,\n"
                "      that version cannot read it -- the header format changed in 5.x.\n"
                "      Install a newer Blender and re-run, or pass --blender /path/to/it.\n"
                % (blender, ".".join(map(str, version or ["unknown"]))))
        sys.exit("export failed (blender exit %d)" % proc.returncode)

    print(out)


if __name__ == "__main__":
    if bpy is None:
        launch(sys.argv[1:])
    else:
        # Blender puts our arguments after a bare `--`, which it ignores itself.
        argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
        export_inside_blender(argv)
