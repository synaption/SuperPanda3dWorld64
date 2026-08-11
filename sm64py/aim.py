"""Turn the upper body toward what the player is aiming at.

This is docs/aim.md's procedural layer, and only that layer: the clips still
say how an attack moves, and this says which way the motion is pointed. The
authored animation is never overridden, because the bone this writes to --
AIM_TORSO, inserted by tools/aim_rig.py -- carries no keyframes at all. It sits
between the skeleton root and everything above the hips, so rotating it turns
the spine, the head, the arms and the cape as one piece while the clips
underneath keep playing whatever they were playing.

What that buys is the thing the Hero could not do before: aim somewhere other
than where he is walking. The legs run in the direction he is moving, the torso
turns toward the crosshair, and the two are no longer the same number.

Three rules shape it, all from the doc:

**The torso has limits.** It twists to `yaw_limit` and no further. Past that
the character has to turn his feet, which is what `body_turn` asks for -- it
returns the excess rather than turning him, since who is allowed to write to a
character's facing is the caller's business and not this file's.

**It does not snap.** The torso is sprung toward the aim rather than assigned
to it, and how fast is a per-weapon number: docs/aim.md's "Pistol fast, Rifle
fast, Cannon slow" is the `response` field of a profile. A hard mount that
whipped instantly to the crosshair would read as the camera dragging a doll
around rather than as a character looking.

**Melee gives it up as it commits.** `tracking` scales the whole correction, so
an attack can track hard during the windup and not at all once the blade is
moving. See `melee_tracking`.

The rest of the doc -- distributing the turn across spine and shoulders, aim
offset poses, left-hand IK -- needs a skeleton whose arms are parented to its
chest, which the Hero's export is not. tools/aim_rig.py says why at length.
"""

import math

from .math_util import atan2s, degrees_to_s16, s16, s16_to_degrees

# -- profiles -----------------------------------------------------------------


class AimProfile:
    """How one weapon aims. docs/aim.md's "part of weapon feel"."""

    def __init__(self, yaw_limit=60.0, pitch_min=-45.0, pitch_max=60.0,
                 response=0.12, pitch_share=0.55, comfort_yaw=20.0,
                 turn_rate=6.0):
        # How far the torso twists before the feet have to help, in degrees.
        self.yaw_limit = yaw_limit
        # Down and up. Aiming up leans him back, so up is the larger number:
        # a body bends further backwards from the hips than it folds forwards
        # over them without the legs joining in.
        self.pitch_min = pitch_min
        self.pitch_max = pitch_max
        # Spring time in seconds -- smaller is snappier. A rifle is fast, a
        # cannon is slow, and that difference is most of what the two feel like.
        self.response = response
        # How much of the aim's elevation the torso actually takes. Full pitch
        # rotates the whole upper body by the angle of the shot, which for a
        # steep one lays him flat; the rest of the angle belongs to the arms,
        # in a clip or an aim-offset pose that does not exist yet.
        self.pitch_share = pitch_share
        # Standing still, he squares up until the twist is back inside this.
        # Without it he would stand at a 59-degree twist indefinitely, which
        # the limit permits and no person does.
        self.comfort_yaw = comfort_yaw
        # How fast the feet come round, in turns per second at full deflection.
        self.turn_rate = turn_rate


# The Hero has no weapons yet, so there is one profile and it is a rifle's:
# quick, because everything about the camera it is attached to is quick.
DEFAULT = AimProfile()


# -- melee commitment ---------------------------------------------------------

# docs/aim.md's tracking curve, as (normalised time, tracking) knees. The
# attack steers freely while he is winding up, keeps some of it as the swing
# starts, almost none once the blade is live, and none at all on the recovery.
# Interpolated between the knees rather than stepped, so the correction bleeds
# away over the swing instead of switching off on one frame.
MELEE_TRACKING = ((0.00, 1.00), (0.25, 0.60), (0.55, 0.15), (0.75, 0.00))


def melee_tracking(normalised_time, curve=MELEE_TRACKING):
    """How much of the aim an attack still follows, this far into itself."""
    t = min(max(normalised_time, 0.0), 1.0)
    previous_time, previous_value = curve[0]
    if t <= previous_time:
        return previous_value
    for time, value in curve[1:]:
        if t <= time:
            span = time - previous_time
            share = (t - previous_time) / span if span else 1.0
            return previous_value + (value - previous_value) * share
        previous_time, previous_value = time, value
    return previous_value


# -- the controller -----------------------------------------------------------


def _spring(current, velocity, target, smooth_time, dt):
    """One step of a critically damped spring. See camera.smooth_damp.

    Copied in shape rather than imported so this module does not depend on the
    camera; it is the same algebra without the maximum-speed clamp.
    """
    smooth_time = max(smooth_time, 1e-4)
    omega = 2.0 / smooth_time
    x = omega * dt
    decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x)
    change = current - target
    temp = (velocity + omega * change) * dt
    velocity = (velocity - omega * temp) * decay
    return target + (change + temp) * decay, velocity


class AimController:
    """Where the upper body is pointed, and what that costs the feet.

    Fed a world-space direction, it produces the character-local yaw and pitch
    of that direction, springs the torso toward them within the profile's
    limits, and writes the result to the AIM_TORSO joint.
    """

    def __init__(self, joint, profile=DEFAULT):
        # A Panda3D NodePath from Actor.controlJoint, or None -- a character
        # whose skeleton has no pivot still answers every question below, it
        # simply has nothing to draw the answer on.
        self.joint = joint
        self.profile = profile

        self.target_yaw = 0.0           # where the aim is, character-local
        self.target_pitch = 0.0
        self.yaw = 0.0                  # where the torso actually is
        self.pitch = 0.0
        self._yaw_velocity = 0.0
        self._pitch_velocity = 0.0

        # How much of the correction to apply at all: the sights blending in,
        # and a melee attack committing, both come through here.
        self.tracking = 0.0

    # -- input ---------------------------------------------------------------

    def set_aim_direction(self, direction, facing_yaw):
        """Aim along a world-space direction, from a character facing this way.

        `direction` is in the game's coordinates -- y up, yaw 0 along +z -- so
        it can come from the camera's ray or from a locked-on target's position
        without either knowing about the other. `facing_yaw` is the s16 the
        character's feet are pointing along, which is what makes the result
        local: the same crosshair means a different twist depending on which
        way he is standing.
        """
        x, y, z = direction
        flat = math.hypot(x, z)
        self.target_yaw = s16_to_degrees(s16(atan2s(z, x) - facing_yaw))
        # Positive is up. Straight up is 90 and straight down is -90, both of
        # which the clamp below has opinions about.
        self.target_pitch = math.degrees(math.atan2(y, flat)) if flat or y else 0.0

    def set_tracking(self, strength):
        self.tracking = min(max(float(strength), 0.0), 1.0)

    # -- output --------------------------------------------------------------

    @property
    def local_yaw(self):
        """The clamped twist the torso is being asked for, in degrees."""
        limit = self.profile.yaw_limit
        return min(max(self.target_yaw, -limit), limit) * self.tracking

    @property
    def local_pitch(self):
        profile = self.profile
        wanted = self.target_pitch * profile.pitch_share
        return (min(max(wanted, profile.pitch_min), profile.pitch_max)
                * self.tracking)

    def twist_available(self, moving):
        """How much of the aim the torso is allowed to absorb, in degrees.

        Moving, the full twist the profile allows: his legs are busy carrying
        him somewhere and turning them would send him there sideways. Standing,
        only as much as is comfortable, so he squares up rather than staying
        wrung out at the waist indefinitely.

        None at all for a character with no pivot to twist -- Mario, whose
        skeleton tools/aim_rig.py has never been near. He aims by turning
        round, which is what the whole of this number being zero means.
        """
        if self.joint is None:
            return 0.0
        return self.profile.yaw_limit if moving else self.profile.comfort_yaw

    def body_turn(self, dt, moving):
        """How far the feet should come round this frame, in s16 units.

        docs/aim.md: the torso takes what it can and the character turns for
        the rest.

        Returns a signed s16 delta for the caller to apply to its own facing,
        or 0. Nothing here writes to the character.
        """
        profile = self.profile
        if self.tracking <= 0.0:
            return 0
        excess = abs(self.target_yaw) - self.twist_available(moving)
        if excess <= 0.0:
            return 0

        # Rate-limited by the excess itself, so the turn eases out as he comes
        # round rather than stopping dead the moment it is inside the limit.
        step = excess * min(profile.turn_rate * dt, 1.0) * self.tracking
        return degrees_to_s16(math.copysign(step, self.target_yaw))

    # -- per frame -----------------------------------------------------------

    def update(self, dt):
        """Spring the torso toward the aim and write it to the joint."""
        profile = self.profile
        self.yaw, self._yaw_velocity = _spring(
            self.yaw, self._yaw_velocity, self.local_yaw, profile.response, dt)
        self.pitch, self._pitch_velocity = _spring(
            self.pitch, self._pitch_velocity, self.local_pitch,
            profile.response, dt)

        if self.joint is not None:
            # Heading is the character's own yaw sense: the joint sits under a
            # node the game has already turned to his facing, so a positive
            # heading here is a positive turn there. Pitch leans him back,
            # which is what aiming up does to a body pivoting at the hips.
            self.joint.set_hpr(self.yaw, self.pitch, 0.0)

    def reset(self):
        """Drop the twist. For a character being put somewhere else."""
        self.target_yaw = self.target_pitch = 0.0
        self.yaw = self.pitch = 0.0
        self._yaw_velocity = self._pitch_velocity = 0.0
        self.tracking = 0.0
        if self.joint is not None:
            self.joint.set_hpr(0.0, 0.0, 0.0)
