"""N64-style fixed-point angle math.

Angles are 16-bit binary angles: 0x10000 is a full turn, stored signed in
[-0x8000, 0x8000).  The original engine looks sine up in a 4096-entry table
indexed by the top 12 bits of the angle, so the quantisation is reproduced
here instead of calling math.sin directly -- it is what makes a long slide or
a slow turn drift the way it does on hardware.
"""

import math

# Entry i covers binary angle i * 16.
_SINE_TABLE = [math.sin(i * math.tau / 4096.0) for i in range(4096)]

DEGREES_PER_UNIT = 360.0 / 65536.0


def s16(value):
    """Wrap to the signed 16-bit range angles are stored in."""
    value = int(value) & 0xFFFF
    return value - 0x10000 if value >= 0x8000 else value


def u16(value):
    return int(value) & 0xFFFF


def sins(angle):
    return _SINE_TABLE[(int(angle) & 0xFFFF) >> 4]


def coss(angle):
    return _SINE_TABLE[((int(angle) + 0x4000) & 0xFFFF) >> 4]


def atan2s(z, x):
    """Binary angle of the vector (x, z), keeping the engine's argument order.

    Yaw 0 faces +Z and 0x4000 faces +X, which is why the components arrive
    swapped relative to the usual atan2.
    """
    return s16(round(math.atan2(x, z) * 65536.0 / math.tau))


def s16_to_degrees(angle):
    return s16(angle) * DEGREES_PER_UNIT


def degrees_to_s16(degrees):
    return s16(round(degrees / DEGREES_PER_UNIT))


def approach_s32(current, target, inc, dec):
    if current < target:
        current += inc
        if current > target:
            current = target
    else:
        current -= dec
        if current < target:
            current = target
    return int(current)


def approach_f32(current, target, inc, dec):
    if current < target:
        current += inc
        if current > target:
            current = target
    else:
        current -= dec
        if current < target:
            current = target
    return float(current)


def clamp(value, lo, hi):
    return lo if value < lo else (hi if value > hi else value)


# -- Panda3D bridge ---------------------------------------------------------
#
# SM64 is Y-up right-handed with +Z pointing back toward the camera.  Panda3D
# is Z-up right-handed with +Y pointing away from the camera.  Rotating about
# X by -90 degrees maps one to the other and preserves handedness, so a yaw
# stays a yaw and no geometry needs winding fixes.


def to_panda(x, y, z):
    """SM64 world position -> Panda3D world position."""
    return (x, -z, y)


def from_panda(x, y, z):
    """Panda3D world position -> SM64 world position."""
    return (x, z, -y)


def yaw_to_panda_h(angle):
    """SM64 facing angle -> Panda3D heading, in degrees."""
    return s16_to_degrees(angle)
