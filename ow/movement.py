"""Port of AC_SpaceMovementComponent and BFL_ZeroGFunctions.

Two orientations matter, and keeping them apart is the whole trick:

  * the **direction arrow** is where you are actually pointing. Look input
    rotates it immediately, and every jetpack thrust is applied along its axes.
  * the **camera** chases the arrow with an exponential lag (CameraLag).

Because thrust follows the arrow and not the camera, the ship answers the
stick instantly while the view swims after it. That split is what gives the
original its floaty, momentum-heavy feel.

Thrust is queued as a *force* on the player's GravityComponent, into the same
ListForces array gravity uses, so jetpack and gravity are summed by one
integrator rather than fighting each other.
"""

import math

from panda3d.core import LQuaternion, LVecBase3, LVector3

from .constants import CharacterVariables


def local_rotation(yaw, pitch, roll):
    """A rotation expressed in the rotating object's own frame.

    Panda's HPR triple is (heading about Z/up, pitch about X/right,
    roll about Y/forward), all right-handed, all degrees.
    """
    delta = LQuaternion()
    delta.setHpr(LVecBase3(yaw, pitch, roll))
    return delta


def apply_local_rotation(quat, yaw, pitch, roll):
    """Equivalent of Unreal's AddLocalRotation / Panda's setHpr(self, ...).

    Panda composes with row vectors, so the delta goes on the left to be
    applied before the existing orientation -- i.e. in local space.
    """
    result = local_rotation(yaw, pitch, roll) * quat
    result.normalize()
    return result


def slerp(a, b, t):
    """Shortest-arc interpolation, standing in for Unreal's RLerp."""
    a = LQuaternion(a)
    b = LQuaternion(b)
    dot = a.getR() * b.getR() + a.getI() * b.getI() + a.getJ() * b.getJ() + a.getK() * b.getK()
    if dot < 0.0:
        b = LQuaternion(-b.getR(), -b.getI(), -b.getJ(), -b.getK())
        dot = -dot
    if dot > 0.9995:
        # Nearly parallel: lerp and renormalise, slerp is ill-conditioned here.
        out = LQuaternion(
            a.getR() + (b.getR() - a.getR()) * t,
            a.getI() + (b.getI() - a.getI()) * t,
            a.getJ() + (b.getJ() - a.getJ()) * t,
            a.getK() + (b.getK() - a.getK()) * t,
        )
        out.normalize()
        return out
    theta = math.acos(max(-1.0, min(1.0, dot)))
    sin_theta = math.sin(theta)
    wa = math.sin((1.0 - t) * theta) / sin_theta
    wb = math.sin(t * theta) / sin_theta
    out = LQuaternion(
        a.getR() * wa + b.getR() * wb,
        a.getI() * wa + b.getI() * wb,
        a.getJ() * wa + b.getJ() * wb,
        a.getK() * wa + b.getK() * wb,
    )
    out.normalize()
    return out


class InputState:
    """One frame of the five Enhanced Input actions the original defines.

    IA_Move (Axis2D), IA_Look (Axis2D), IA_UpDown (Axis1D),
    IA_Brake (Digital), IA_Roll (Digital).
    """

    def __init__(self):
        self.move = (0.0, 0.0)      # (strafe, forward), -1..1
        self.look = (0.0, 0.0)      # stick deflection, -1..1, scaled by dt
        #: Mouse deltas, already in degrees. A mouse reports a displacement
        #: rather than a deflection, so this path is *not* scaled by dt --
        #: otherwise the same physical movement would turn you further at low
        #: framerates. The original leaves Mouse2D on the dt-scaled path and
        #: inherits that quirk.
        self.look_impulse = (0.0, 0.0)
        self.up_down = 0.0          # -1..1
        self.brake = False
        self.roll = False           # held: look-X rolls instead of yawing

    def clear(self):
        self.move = (0.0, 0.0)
        self.look = (0.0, 0.0)
        self.look_impulse = (0.0, 0.0)
        self.up_down = 0.0
        self.brake = False
        self.roll = False


class SpaceMovementComponent:
    """The player's jetpack, look controls, and lagging camera."""

    def __init__(self, physics_ref, variables=None):
        #: PhysicsRef -- the player's own GravityComponent.
        self.physics_ref = physics_ref
        self.variables = variables or CharacterVariables()

        #: ArrowRef -- true aim. Thrust is applied along these axes.
        self.arrow_quat = LQuaternion()
        self.arrow_quat.setHpr(LVecBase3(0, 0, 0))
        #: CameraRef -- lags behind the arrow by CameraLag.
        self.camera_quat = LQuaternion(self.arrow_quat)

        #: bIsRolling -- true while the roll button is held.
        self.is_rolling = False

    # -- axes of the direction arrow --------------------------------------

    @property
    def forward(self):
        return self.arrow_quat.getForward()

    @property
    def right(self):
        return self.arrow_quat.getRight()

    @property
    def up(self):
        return self.arrow_quat.getUp()

    # -- BFL_ZeroGFunctions.RotateDirectionArrow ---------------------------

    def rotate_direction_arrow(self, yaw, pitch, roll):
        self.arrow_quat = apply_local_rotation(self.arrow_quat, yaw, pitch, roll)

    # -- IA_Look / IA_Roll -------------------------------------------------

    def apply_look(self, state, dt):
        """Look input rotates the arrow in its own frame.

        While IA_Roll is held the horizontal axis rolls instead of yawing --
        the graph branches on "Is the player holding Left Shoulder?".
        """
        v = self.variables
        stick_x, stick_y = state.look
        mouse_x, mouse_y = state.look_impulse

        horizontal = stick_x * (v.roll_speed if self.is_rolling else v.rotation_speed) * dt
        horizontal += mouse_x
        pitch = stick_y * v.rotation_speed * dt + mouse_y

        if self.is_rolling:
            self.rotate_direction_arrow(0.0, pitch, horizontal)
        else:
            # Negative heading turns right: Panda's +H is a left turn.
            self.rotate_direction_arrow(-horizontal, pitch, 0.0)

    # -- IA_Move / IA_UpDown / IA_Brake ------------------------------------

    def apply_thrust(self, state, dt):
        """Queue this frame's jetpack forces onto the player's body."""
        v = self.variables
        body = self.physics_ref

        strafe, forward_axis = state.move
        if strafe or forward_axis:
            # The original normalises, discarding stick magnitude, so thrust
            # is full power in the chosen direction and diagonals aren't fast.
            direction = self.right * strafe + self.forward * forward_axis
            if direction.lengthSquared() > 0.0:
                direction.normalize()
                body.add_force(direction * v.move_acceleration)

        if state.up_down:
            body.add_force(self.up * (state.up_down * v.up_down_acceleration))

        if state.brake:
            body.add_force(self._brake_force(dt))

    def _brake_force(self, dt):
        """Thrust opposing current velocity, clamped so it cannot reverse it.

        The original applies the force unclamped, which lets a hard brake
        overshoot into a slow drift backwards. Clamping is the one behavioural
        change in this port: at the frame where braking would flip the
        velocity, apply only enough to reach a stop.
        """
        v = self.variables
        speed = self.physics_ref.speed
        magnitude = speed.length()
        if magnitude <= 0.0:
            return LVector3(0, 0, 0)

        direction = speed / magnitude
        force = direction * v.brake_acceleration
        mass = self.physics_ref.mass_self or 1.0
        delta_v = (abs(v.brake_acceleration) / mass) * dt
        if delta_v > magnitude:
            force = direction * (-magnitude * mass / dt)
        return force

    # -- Tick: LerpCameraToArrow -------------------------------------------

    def update_camera(self, dt):
        """Ease the camera toward the arrow. Exponential, framerate-correct."""
        alpha = min(1.0, self.variables.camera_lag * dt)
        self.camera_quat = slerp(self.camera_quat, self.arrow_quat, alpha)

    def snap_camera(self):
        self.camera_quat = LQuaternion(self.arrow_quat)

    # -- the whole per-step update ----------------------------------------

    def update(self, state, dt):
        self.is_rolling = state.roll
        self.apply_look(state, dt)
        self.apply_thrust(state, dt)
        self.update_camera(dt)
