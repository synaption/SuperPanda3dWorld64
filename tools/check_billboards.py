"""Check that billboarded actor parts really do turn to face the camera.

Two things get checked, because either alone has been misleading before:

  * through the game's own ObjectRenderer, that the joints are claimed and
    that their transforms actually change when the camera moves. A billboard
    that never moves is the failure this project shipped for several rounds,
    and it is invisible in a still screenshot.

  * through the workbench, that each quad holds its width around an orbit.
    One joint at a time -- measuring a whole actor lets leg geometry dominate
    the count, and measuring all three of a scuttlebug's billboards together
    measures how far apart they are rather than how wide each one is.

    python3 tools/check_billboards.py
"""

import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

ACTORS = os.path.join(ROOT, "assets", "actors")

# Below this, a quad is collapsing toward a line somewhere around the orbit
# rather than holding flat-on to the camera. Comfortably clear of the ~0.7 an
# off-axis quad measures from perspective alone, and far above the ~0.06 an
# unaimed one measures.
MIN_RATIO = 0.5


def check_tracking():
    """Do the joints move when the camera does, in the real render path?"""
    from panda3d.core import loadPrcFileData
    loadPrcFileData("", "window-type offscreen")
    loadPrcFileData("", "audio-library-name null")
    from direct.showbase.ShowBase import ShowBase

    base = ShowBase()
    from sm64py.level import ObjectRenderer
    from sm64py.objects import Goomba, ObjectSet, Scuttlebug

    class Ground:
        """Just enough floor for the objects to spawn onto."""

        def find_floor(self, *args, **kwargs):
            return 0.0, None

    objects = ObjectSet(Ground())
    objects.spawn(Goomba, 500.0, 0.0, 500.0)
    objects.spawn(Scuttlebug, -500.0, 0.0, 500.0)

    renderer = ObjectRenderer(ACTORS, base.loader, base.render)
    renderer.build(objects)

    renderer.sync((0.0, 300.0, 3000.0))
    north = [tuple(round(v, 1) for v in rig.control.get_hpr())
             for rig in renderer.billboards]
    renderer.sync((3000.0, 300.0, 0.0))
    east = [tuple(round(v, 1) for v in rig.control.get_hpr())
            for rig in renderer.billboards]

    results = []
    for rig, before, after in zip(renderer.billboards, north, east):
        results.append({
            "actor": rig.actor_name,
            "joint": rig.name,
            "from": before,
            "to": after,
            "pass": before != after,
        })
    return results


def check_widths(actor, joints):
    """Does each quad hold its apparent width all the way round?"""
    results = []
    for joint in joints:
        out = subprocess.run(
            [sys.executable, os.path.join(HERE, "workbench.py"),
             os.path.join(ACTORS, actor + ".glb"),
             "--isolate", joint + "$", "--billboard", "--frame", "0",
             "--orbit", "12", "--size", "480x400", "--json"],
            capture_output=True, text=True, cwd=ROOT)
        try:
            report = json.loads(out.stdout[out.stdout.index("{"):])
        except ValueError:
            results.append({"actor": actor, "joint": joint, "pass": False,
                            "reason": "workbench produced no report"})
            continue
        check = report["checks"]["billboard"]
        results.append({
            "actor": actor, "joint": joint,
            "ratio": check["ratio"],
            "widths": [e["screen_width"] for e in report["orbit"]],
            "pass": check["ratio"] >= MIN_RATIO,
        })
    return results


def main():
    failures = 0

    print("tracking the camera, through ObjectRenderer:")
    for row in check_tracking():
        mark = "ok  " if row["pass"] else "FAIL"
        failures += not row["pass"]
        print(f"  [{mark}] {row['actor']}/{row['joint']}: "
              f"{row['from']} -> {row['to']}")

    print("\nholding width around an orbit, one quad at a time:")
    for actor, joints in (("goomba", ["billboard_4"]),
                          ("scuttlebug", ["billboard_19", "billboard_24",
                                          "billboard_30"])):
        for row in check_widths(actor, joints):
            mark = "ok  " if row["pass"] else "FAIL"
            failures += not row["pass"]
            detail = (f"ratio {row['ratio']:.3f} (min {MIN_RATIO})"
                      if "ratio" in row else row.get("reason", ""))
            print(f"  [{mark}] {row['actor']}/{row['joint']}: {detail}")

    print(f"\n{failures} failure(s)" if failures else "\nall good")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
