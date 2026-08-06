"""A following camera in the spirit of Lakitu's.

This is deliberately a simplification.  The original camera is a large state
machine with per-area modes, cutscene handling and hand-authored triggers;
what matters for the movement system is that the camera trails Mario, that
the player can swing it around him, and that its yaw is what the analog stick
is measured against.  Those are the parts implemented here.
"""

import math

from .math_util import atan2s, coss, degrees_to_s16, s16, sins, to_panda
from .surfaces import WallCollisionData

# Distance and height the camera prefers to sit at behind Mario.  The height
# is kept low so the view stays close to horizontal and the horizon -- and
# whatever Mario is running toward -- stays visible.
DEFAULT_DISTANCE = 1500.0
DEFAULT_HEIGHT = 230.0

MIN_DISTANCE = 250.0
PITCH_MIN = -0.30
PITCH_MAX = 0.65

# Smoothing rates, per second, for the camera's yaw and focus height.
YAW_RATE = 8.0
HEIGHT_RATE = 6.0


def _blend(rate, dt):
    """Frame-rate independent smoothing factor.

    A plain `rate * dt` changes how fast the camera settles when the frame
    rate changes, and passes any jitter in dt straight through to the motion.
    Exponential decay gives the same settling time at any frame rate.
    """
    return 1.0 - math.exp(-rate * dt)


class FollowCamera:
    def __init__(self, surfaces, mario):
        self.surfaces = surfaces
        self.mario = mario

        self.yaw = s16(mario.face_angle[1] + 0x8000)
        self.pitch = 0.06
        self.distance = DEFAULT_DISTANCE
        self.height = DEFAULT_HEIGHT

        self.pos = [0.0, 0.0, 0.0]
        self.focus = [0.0, 0.0, 0.0]

        # Yaw the player is steering toward; the actual yaw eases into it.
        self.target_yaw = self.yaw
        self._initialised = False

    def rotate(self, delta_degrees):
        self.target_yaw = s16(self.target_yaw + degrees_to_s16(delta_degrees))

    def tilt(self, delta):
        self.pitch = min(max(self.pitch + delta, PITCH_MIN), PITCH_MAX)

    def update(self, dt, target_pos=None, recenter=False):
        m = self.mario
        # Follow the interpolated render position when one is supplied; the
        # raw simulation position only moves in 30 Hz steps and would judder.
        pos = target_pos if target_pos is not None else m.pos

        if recenter:
            # Snap around behind Mario, the way pressing R does.
            self.target_yaw = s16(m.face_angle[1] + 0x8000)

        # Ease the yaw toward its target on the short way round.
        delta = s16(self.target_yaw - self.yaw)
        self.yaw = s16(self.yaw + delta * _blend(YAW_RATE, dt))

        focus_x = pos[0]
        focus_y = pos[1] + 120.0
        focus_z = pos[2]

        # Ease the focus height so stairs and slopes do not jolt the view.
        if self._initialised:
            self.focus[0] = focus_x
            self.focus[2] = focus_z
            self.focus[1] += (focus_y - self.focus[1]) * _blend(HEIGHT_RATE, dt)
        else:
            self.focus = [focus_x, focus_y, focus_z]
            self._initialised = True

        horizontal = self.distance * math.cos(self.pitch)
        desired = [
            self.focus[0] + horizontal * sins(self.yaw),
            self.focus[1] + self.height + self.distance * math.sin(self.pitch),
            self.focus[2] + horizontal * coss(self.yaw),
        ]

        self.pos = self._resolve_collisions(desired)

    def _resolve_collisions(self, desired):
        """Keep the camera out of walls and above the floor."""
        data = WallCollisionData(desired[0], desired[1], desired[2], 0.0, 100.0)
        self.surfaces.find_wall_collisions(data, for_camera=True)
        x, y, z = data.x, desired[1], data.z

        # Duck under a ceiling first, then lift clear of the floor.  The floor
        # gets the final say: in a gap too tight for both, being slightly
        # inside the ceiling is survivable, dropping through the ground is not.
        ceil_height, ceil = self.surfaces.find_ceil(x, y, z, for_camera=True)
        if ceil is not None and y > ceil_height - 50.0:
            y = ceil_height - 50.0

        floor_height, floor = self.surfaces.find_floor(x, y, z, for_camera=True)
        if floor is not None and y < floor_height + 125.0:
            y = floor_height + 125.0

        # Never end up inside Mario.
        dx, dz = x - self.focus[0], z - self.focus[2]
        dist = math.hypot(dx, dz)
        if dist < MIN_DISTANCE and dist > 0.001:
            scale = MIN_DISTANCE / dist
            x = self.focus[0] + dx * scale
            z = self.focus[2] + dz * scale

        return [x, y, z]

    @property
    def mario_yaw(self):
        """The yaw the analog stick should be interpreted relative to.

        Stick-up must send Mario away from the camera, and the camera sits
        behind him, so this is the camera's yaw turned around.
        """
        return s16(self.yaw + 0x8000)

    def apply_to(self, node):
        """Point a Panda3D camera node at Mario from the current position."""
        node.set_pos(*to_panda(*self.pos))
        node.look_at(*to_panda(self.focus[0], self.focus[1] + 60.0, self.focus[2]))
