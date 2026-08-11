"""Render a crowd of impostor sprites offscreen, to check the runtime path.

Spawns a line of one enemy at every heading and a field of many behind it,
draws one frame, and writes it to a PNG. What it is looking for: that the
shader builds, that the instancing draws at all, that the sprites face the
camera, and that the one whose heading points at the camera is the one showing
its face -- which is how the bake's angles get lined up with the game's.

    python3 tools/check_impostors.py [--count N] [--out PATH]
"""

import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

IMPOSTORS = os.path.join(ROOT, "assets", "impostors")


class Ground:
    """A flat floor at y = 0, enough for objects to snap onto."""

    def find_floor(self, *args, **kwargs):
        return 0.0, self

    def find_water_level(self, *args, **kwargs):
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=2000,
                    help="how many extra sprites to spread out behind the row")
    ap.add_argument("--model", default="goomba")
    ap.add_argument("--out", default=os.path.join(
        HERE, "..", "impostor_check.png"))
    args = ap.parse_args()

    from panda3d.core import loadPrcFileData
    loadPrcFileData("", "win-size 900 600")
    loadPrcFileData("", "window-type offscreen")
    loadPrcFileData("", "audio-library-name null")
    loadPrcFileData("", "sync-video 0")

    from direct.showbase.ShowBase import ShowBase
    from sm64py import objects
    from sm64py.impostor import ImpostorSet

    base = ShowBase()
    base.set_background_color(0.32, 0.60, 0.86, 1.0)

    world = objects.ObjectSet(Ground())
    cls = {"goomba": objects.Goomba, "scuttlebug": objects.Scuttlebug}[args.model]
    scale = cls.draw_scale

    # Four across the front, at headings 0/90/180/270, for reading the angle
    # calibration: game yaw 0 faces the camera here, so the leftmost should show
    # its face and the third along its back.
    for i in range(4):
        world.spawn(cls, -1200.0 + i * 800.0, 0.0, -800.0, i * (65536 // 4))

    # A crowd behind them, to prove the instancing draws many at once.
    import random
    random.seed(1)
    for _ in range(args.count):
        x = random.uniform(-3000.0, 3000.0)
        z = random.uniform(-3500.0, -1600.0)
        obj = world.spawn(cls, x, 0.0, z, random.randint(0, 65535))
        obj.timer = random.randint(0, 30)

    field = ImpostorSet(IMPOSTORS, base.render, {args.model: scale})

    # Camera in front of the row, looking at it, in Panda space; the game-space
    # position handed to update is the same point run back through from_panda.
    from sm64py.math_util import from_panda
    base.camera.set_pos(0.0, -2600.0, 900.0)
    base.camera.look_at(0.0, 800.0, 150.0)
    cam_game = from_panda(0.0, -2600.0, 900.0)

    drawn = field.update(world, cam_game)

    base.graphics_engine.render_frame()
    base.graphics_engine.render_frame()
    out = os.path.abspath(args.out)
    base.win.save_screenshot(out)
    print(f"drew {drawn} {args.model} sprites -> {out}")


if __name__ == "__main__":
    main()
