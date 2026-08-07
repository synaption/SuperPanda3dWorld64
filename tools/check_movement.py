"""Measure the movement numbers the README quotes, so they can be re-checked.

Every figure in the README's "Verified behaviour" section came from a run like
this one. Leaving them as prose meant they could drift silently as the action
code changed -- and at least one had: walking was documented as capping at
exactly 32.0 when what it actually does is oscillate.

Ground truth is a synthetic flat floor rather than the level, so terrain cannot
confuse the reading. Two details of that floor are easy to get wrong and both
fail quietly:

  * winding. Reversed, every triangle is a ceiling, `find_floor` returns the
    death plane, and Mario falls through a floor that looks fine in the data.
  * size. Collision samples truncate to s16, so a plane wider than +/-32768
    wraps and the floor vanishes partway across it.

    python3 tools/check_movement.py
"""

import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py.camera import FollowCamera  # noqa: E402
from sm64py.mario import (Controller, MarioState,  # noqa: E402
                          constants as C, execute_action)
from sm64py.surfaces import SurfaceSet  # noqa: E402

TICK_DT = 1.0 / 30.0

# Comfortably inside the s16 range collision samples truncate to.
PLANE = 30000

COLLISION = os.path.join(ROOT, "assets", "castle_grounds", "collision.npz")

# Where the level script puts Mario. Mirrors app/main.py.
SPAWN = (-1328.0, 260.0, 4664.0)


def flat_ground():
    """A floor big enough to run on and wound so it faces upward."""
    vertices = np.array([[-PLANE, 0, -PLANE], [PLANE, 0, -PLANE],
                         [PLANE, 0, PLANE], [-PLANE, 0, PLANE]], dtype=np.int32)
    triangles = np.array([[0, 2, 1], [0, 3, 2]], dtype=np.int32)
    zeros = np.zeros(2, dtype=np.int32)
    return SurfaceSet(vertices, triangles, zeros, zeros, None)


class Run:
    """One simulation, stepped the way app/main.py steps it."""

    def __init__(self, surfaces, pos=(0.0, 10.0, 0.0), yaw=0.0):
        self.surfaces = surfaces
        self.controller = Controller()
        self.mario = MarioState(surfaces, self.controller)
        self.mario.spawn(pos[0], pos[1], pos[2], yaw)
        self.camera = FollowCamera(surfaces, self.mario)

    def tick(self, forward=0.0, buttons=0):
        # Negative stick_y is forward: app/main.py feeds both axes mirrored,
        # because the heading is built as atan2s(-stick_y, stick_x) and then
        # rotated by the camera yaw.
        self.controller.set_stick(0.0, -forward)
        self.controller.set_buttons(buttons)
        self.mario.camera_yaw = self.camera.mario_yaw
        execute_action(self.mario)
        self.camera.update(TICK_DT)

    @property
    def height(self):
        return self.mario.pos[1]

    @property
    def grounded(self):
        return self.mario.pos[1] <= 0.01


def walking_speed(ticks=200, settle=100):
    """Forward speed once walking has settled."""
    run = Run(flat_ground())
    speeds = []
    for _ in range(ticks):
        run.tick(forward=1.0)
        speeds.append(run.mario.forward_vel)
    tail = speeds[settle:]
    return {
        "min": min(tail), "max": max(tail),
        "mean": sum(tail) / len(tail),
        "ticks_to_settle": next(i for i, v in enumerate(speeds) if v >= min(tail)),
    }


def jump_height(hold=True, ticks=120):
    """How high a standing jump goes, holding A or releasing it."""
    run = Run(flat_ground())
    peak = 0.0
    launched = False
    for i in range(ticks):
        buttons = 0
        if not launched:
            buttons = C.A_BUTTON
            if not run.grounded:
                launched = True
        elif hold:
            buttons = C.A_BUTTON
        run.tick(buttons=buttons)
        peak = max(peak, run.height)
    return peak


def jump_chain(ticks=400):
    """Peak height of each jump in a single -> double -> triple chain.

    A has to be pressed on the frame Mario lands, which is what makes the next
    jump in the chain rather than a fresh one.
    """
    run = Run(flat_ground())
    peaks = []
    current = 0.0
    airborne = False
    for _ in range(ticks):
        press = run.grounded and len(peaks) < 3
        run.tick(forward=1.0, buttons=C.A_BUTTON if press else 0)

        if not run.grounded:
            airborne = True
            current = max(current, run.height)
        elif airborne:
            peaks.append(current)
            current = 0.0
            airborne = False
    return peaks


def level_numbers():
    """Figures taken from the converted level rather than from a simulation."""
    if not os.path.exists(COLLISION):
        return None
    data = np.load(COLLISION)
    surfaces = SurfaceSet(data["vertices"], data["tri_verts"], data["tri_type"],
                          data["tri_force"], data["water_boxes"])
    floor, _ = surfaces.find_floor(SPAWN[0], SPAWN[1] + 100.0, SPAWN[2])
    return {
        "vertices": len(data["vertices"]),
        "triangles": len(data["tri_verts"]),
        "water_boxes": len(data["water_boxes"]),
        "spawn_floor": floor,
    }


def main():
    walk = walking_speed()
    print("walking on flat ground")
    print(f"  settles into a sawtooth between {walk['min']:.2f} and "
          f"{walk['max']:.2f}, mean {walk['mean']:.2f}")
    print(f"  reaches it after {walk['ticks_to_settle']} ticks")

    held = jump_height(hold=True)
    released = jump_height(hold=False)
    print("\nstanding jump")
    print(f"  A held      {held:.1f} units")
    print(f"  A released  {released:.1f} units")

    peaks = jump_chain()
    print("\njump chain (running)")
    for index, peak in enumerate(peaks, 1):
        print(f"  jump {index}      {peak:.1f} units")

    level = level_numbers()
    print("\nconverted level")
    if level is None:
        print(f"  no collision data at {COLLISION}; run tools/parse_collision.py")
    else:
        print(f"  {level['vertices']} vertices, {level['triangles']} triangles, "
              f"{level['water_boxes']} water boxes")
        print(f"  floor under the spawn point: {level['spawn_floor']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
