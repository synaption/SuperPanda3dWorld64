"""Hold the impostor's angle pick to an independent geometric ground truth.

The bug this guards: the row that decides which way a sprite faces was chosen
with the camera bearing subtracted instead of added. Head on that cannot be
told apart -- azimuth is 180 and its sign vanishes mod 360 -- so the four-pose
calibration looked right while every off-axis enemy faced the wrong way.

Ground truth here is built from vectors, not from the formula under test: for a
baked row the model faces a known direction (local -Y turned by the row's
heading) and is seen from -Y; the signed bearing of the camera off the model's
face characterises the cell. The runtime object has its own face direction and
its own direction to the camera, hence its own signed bearing. The right cell
is the one whose bearing matches. If the formula picks that cell for a spread
of headings and camera positions -- not just head on -- the sign is right.

    python3 tools/check_impostor_angles.py
"""

import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py.impostor import ImpostorField  # noqa: E402
from sm64py.math_util import to_panda  # noqa: E402


def face_dir(heading_deg):
    """Panda-plane direction a model faces, given its heading.

    The art faces local -Y, so set_h(0) points it at -Y (at the bake camera).
    """
    h = np.radians(heading_deg)
    return np.array([np.sin(h), -np.cos(h)])


def signed_bearing(face, to_cam):
    """Angle of the camera off the face, signed, in degrees."""
    cross = face[0] * to_cam[1] - face[1] * to_cam[0]
    dot = face[0] * to_cam[0] + face[1] * to_cam[1]
    return np.degrees(np.arctan2(cross, dot))


def truth_row(yaw, cx, cy, camx, camy, angles):
    """Which baked row physically shows this object to this camera."""
    rt = signed_bearing(face_dir(yaw), np.array([camx - cx, camy - cy]))
    # Each baked row r is the model at heading r*step seen from -Y.
    step = 360.0 / angles
    best, best_d = 0, 1e9
    for r in range(angles):
        bk = signed_bearing(face_dir(r * step), np.array([0.0, -1.0]))
        d = abs((bk - rt + 180.0) % 360.0 - 180.0)
        if d < best_d:
            best, best_d = r, d
    return best


class FakeField:
    """The row math from ImpostorField, run without loading an atlas."""

    angle_offset = ImpostorField.angle_offset
    angle_sign = ImpostorField.angle_sign

    def row(self, yaw, cx, cy, cz, camx, camy, camz, angles, cols):
        azimuth = np.degrees(np.arctan2(camx - cx, camy - cy))
        rel = self.angle_sign * yaw + azimuth + self.angle_offset
        return int(np.mod(np.round(rel * angles / 360.0), angles))


def main():
    rng = np.random.default_rng(0)
    field = FakeField()
    angles, cols = 16, 16

    n = 20000
    yaw = rng.uniform(0, 360, n)
    # Objects scattered on the ground; camera anywhere around and above them,
    # so plenty of samples are well off the head-on axis the old test used.
    ox = rng.uniform(-4000, 4000, n)
    oz = rng.uniform(-4000, 4000, n)
    camx = rng.uniform(-6000, 6000, n)
    camz = rng.uniform(-6000, 6000, n)
    camy_up = rng.uniform(200, 3000, n)

    bad = offaxis_bad = 0
    for i in range(n):
        cx, cy, cz = to_panda(ox[i], 0.0, oz[i])
        cam = to_panda(camx[i], camy_up[i], camz[i])
        got = field.row(yaw[i], cx, cy, cz, cam[0], cam[1], cam[2],
                        angles, cols)
        want = truth_row(yaw[i], cx, cy, cam[0], cam[1], angles)
        # A tie between two adjacent cells (the object sits on a bin edge) is
        # not a failure; only a pick more than one step off is.
        diff = abs((got - want + angles // 2) % angles - angles // 2)
        if diff > 1:
            bad += 1
            # Is the camera far enough off head-on that the sign would show?
            azc = np.degrees(np.arctan2(cam[0] - cx, cam[1] - cy))
            if abs((azc - 180.0 + 180.0) % 360.0 - 180.0) > 30.0:
                offaxis_bad += 1

    ok = bad == 0
    print(f"[{'ok  ' if ok else 'FAIL'}] impostor row vs geometric truth: "
          f"{n} cases, {bad} wrong ({offaxis_bad} of them off head-on)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
