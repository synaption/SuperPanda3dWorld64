"""Hold the batched surface queries to the scalar ones they replace.

find_floors and find_walls answer a whole array of points at once, for the
crowd simulation to lean on. They are only worth anything if they agree with
find_floor and find_wall_collisions to the last unit, since the whole point is
that the game plays the same with thousands of enemies as with three. This runs
both paths over a spread of random points -- in bounds and out, over floors and
off the edge of the map -- and reports any disagreement.

    python3 tools/check_batch_surfaces.py
"""

import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py import surfaces  # noqa: E402
from sm64py.surfaces import WallCollisionData  # noqa: E402

COLLISION = os.path.join(ROOT, "assets", "castle_grounds", "collision.npz")


def scalar_floor_id(surf):
    return surf.index if surf is not None else -1


def check_floors(surf, xs, ys, zs):
    heights, ids = surf.find_floors(xs, ys, zs)
    bad_h = bad_id = 0
    worst = 0.0
    for i in range(len(xs)):
        h, s = surf.find_floor(xs[i], ys[i], zs[i])
        if scalar_floor_id(s) != ids[i]:
            bad_id += 1
        # HEIGHT_NONE from both is a match; otherwise compare the number.
        if abs(h - heights[i]) > 1e-6:
            bad_h += 1
            worst = max(worst, abs(h - heights[i]))
    return bad_h, bad_id, worst


def check_walls(surf, xs, ys, zs, radius, offset):
    n = len(xs)
    ox, oz, counts = surf.find_walls(
        xs, ys, zs, np.full(n, offset), np.full(n, radius))
    bad = 0
    worst = 0.0
    for i in range(n):
        data = WallCollisionData(xs[i], ys[i], zs[i], offset, radius)
        c = surf.find_wall_collisions(data)
        if c != counts[i]:
            bad += 1
            continue
        d = abs(data.x - ox[i]) + abs(data.z - oz[i])
        if d > 1e-6:
            bad += 1
            worst = max(worst, d)
    return bad, worst


def main():
    surf = surfaces.load(COLLISION)
    rng = np.random.default_rng(0)

    n = 8000
    # A spread that lands on the level, off its edge, and out of bounds, so the
    # boundary rejection and the empty-cell paths are exercised too.
    xs = rng.uniform(-9000, 9000, n)
    ys = rng.uniform(-1000, 4000, n)
    zs = rng.uniform(-9000, 9000, n)

    failures = 0

    bad_h, bad_id, worst = check_floors(surf, xs, ys, zs)
    ok = bad_h == 0 and bad_id == 0
    failures += not ok
    print(f"[{'ok  ' if ok else 'FAIL'}] find_floors vs find_floor: "
          f"{n} points, {bad_h} height, {bad_id} id mismatches"
          f"{'' if ok else f', worst {worst:g}'}")

    for radius, offset in ((150.0, 30.0), (50.0, 60.0), (200.0, 0.0)):
        bad, worst = check_walls(surf, xs, ys, zs, radius, offset)
        ok = bad == 0
        failures += not ok
        print(f"[{'ok  ' if ok else 'FAIL'}] find_walls vs find_wall_collisions "
              f"(r={radius:g}, off={offset:g}): {bad} mismatches"
              f"{'' if ok else f', worst {worst:g}'}")

    # A dense clump in one spot, so a single cell holds many points at once --
    # the grouping path the crowd actually hits.
    cx = rng.uniform(-9000, 9000, n) * 0 + 1500.0 + rng.uniform(-200, 200, n)
    cz = rng.uniform(-9000, 9000, n) * 0 + 1500.0 + rng.uniform(-200, 200, n)
    cy = rng.uniform(0, 1000, n)
    bad_h, bad_id, _ = check_floors(surf, cx, cy, cz)
    ok = bad_h == 0 and bad_id == 0
    failures += not ok
    print(f"[{'ok  ' if ok else 'FAIL'}] find_floors, clustered in one cell: "
          f"{bad_h} height, {bad_id} id mismatches")

    print(f"\n{'all good' if failures == 0 else str(failures) + ' FAILED'}")
    return failures


if __name__ == "__main__":
    sys.exit(1 if main() else 0)
