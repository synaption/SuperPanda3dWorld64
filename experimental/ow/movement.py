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

from panda3d.core import LMatrix3, LQuaternion, LVecBase3, LVector3

from .constants import (
    GROUND_ACCELERATION,
    GROUND_ALIGN_RATE,
    GROUND_CAMERA_LAG,
    JUMP_LOCKOUT,
    JUMP_SPEED,
    MAX_GROUND_PITCH,
    WALK_SPEED,
    CharacterVariables,
)
from .gravity import vec3d


def vec3f(value):
    """Single-precision copy. Orientation maths runs in float32 alongside the
    quaternions; only position and velocity are kept in double."""
    return LVector3(float(value[0]), float(value[1]), float(value[2]))


def basis_quat(forward, up):
    """Orientation with +Z along `up` and +Y as near `forward` as it can be.

    Panda is right-handed with rows (+X right, +Y forward, +Z up), so
    right = forward x up.
    """
    up = vec3f(up)
    up.normalize()
    forward = vec3f(forward)
    forward -= up * forward.dot(up)
    if forward.lengthSquared() < 1e-12:
        # Looking straight along `up`: any perpendicular will do.
        seed = LVector3(0, 0, 1) if abs(up.getZ()) < 0.9 else LVector3(0, 1, 0)
        forward = seed - up * seed.dot(up)
    forward.normalize()
    right = forward.cross(up)
    right.normalize()
    matrix = LMatrix3(
        right.getX(), right.getY(), right.getZ(),
        forward.getX(), forward.getY(), forward.getZ(),
        up.getX(), up.getY(), up.getZ(),
    )
    quat = LQuaternion()
    quat.setFromMatrix(matrix)
    quat.normalize()
    return quat


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

        # -- walking. Not in the original, which has no ground mode at all.
        #: True while standing on a body.
        self.grounded = False
        self.ground_body = None
        self.ground_normal = None
        #: Facing, as a unit vector in the surface's tangent plane. Kept apart
        #: from the arrow so that looking up and down does not tilt the
        #: direction you walk in.
        self.walk_forward = LVector3(0, 1, 0)
        #: Look elevation on foot, degrees, clamped away from straight up.
        self.ground_pitch = 0.0
        self._jump_timer = 0.0
        #: A held jump input must be released before it can become jetpack
        #: thrust. Otherwise holding Space for a normal jump launches the
        #: player into flight as soon as they leave the ground.
        self._jump_requires_release = False

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

    def apply_thrust(self, state, dt, suppress_upward_thrust=False):
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

        if state.up_down and not (suppress_upward_thrust and state.up_down > 0.0):
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
        rate = GROUND_CAMERA_LAG if self.grounded else self.variables.camera_lag
        alpha = min(1.0, rate * dt)
        self.camera_quat = slerp(self.camera_quat, self.arrow_quat, alpha)

    def snap_camera(self):
        self.camera_quat = LQuaternion(self.arrow_quat)

    # -- walking -----------------------------------------------------------

    def _look_deltas(self, state, dt):
        """Combine stick deflection and mouse displacement into degrees."""
        v = self.variables
        stick_x, stick_y = state.look
        mouse_x, mouse_y = state.look_impulse
        return (
            stick_x * v.rotation_speed * dt + mouse_x,
            stick_y * v.rotation_speed * dt + mouse_y,
        )

    def enter_walk(self, normal):
        """Land: keep facing roughly where you were looking, feet to the ground."""
        normal = vec3f(normal)
        forward = self.arrow_quat.getForward()
        planar = forward - normal * forward.dot(normal)
        if planar.lengthSquared() < 1e-12:
            planar = self.arrow_quat.getUp()
            planar -= normal * planar.dot(normal)
        planar.normalize()
        self.walk_forward = planar
        # Preserve how far up or down you were looking.
        self.ground_pitch = max(
            -MAX_GROUND_PITCH,
            min(MAX_GROUND_PITCH, math.degrees(math.asin(
                max(-1.0, min(1.0, forward.dot(normal)))))),
        )

    def walk_target_quat(self, normal):
        base = basis_quat(self.walk_forward, normal)
        return apply_local_rotation(base, 0.0, self.ground_pitch, 0.0)

    def _walk(self, state, dt, body, normal):
        """Steer, walk and jump across a surface."""
        normal_f = vec3f(normal)
        look_x, look_y = self._look_deltas(state, dt)

        # Yaw turns your facing about the surface normal; pitch only tilts the
        # view, and is clamped so you cannot roll over your own feet.
        if look_x:
            turn = LQuaternion()
            turn.setFromAxisAngle(-look_x, normal_f)
            self.walk_forward = turn.xform(self.walk_forward)
        # Re-flatten every step, not just when turning: the surface curves away
        # underneath as you walk, and a facing left in the old tangent plane
        # would aim slightly outward and gently launch you off the planet.
        self.walk_forward -= normal_f * self.walk_forward.dot(normal_f)
        if self.walk_forward.lengthSquared() < 1e-12:
            self.walk_forward = basis_quat(self.arrow_quat.getForward(), normal_f).getForward()
        self.walk_forward.normalize()
        self.ground_pitch = max(
            -MAX_GROUND_PITCH, min(MAX_GROUND_PITCH, self.ground_pitch + look_y)
        )

        # Swing the body round to the surface rather than snapping to it.
        target = self.walk_target_quat(normal_f)
        self.arrow_quat = slerp(self.arrow_quat, target, min(1.0, GROUND_ALIGN_RATE * dt))

        # Ground control. One acceleration serves as both drive and friction:
        # with no input the target is a standstill, so you skid to a stop.
        right = self.walk_forward.cross(normal_f)
        right.normalize()
        strafe, forward_axis = state.move
        wish = right * strafe + self.walk_forward * forward_axis
        if wish.lengthSquared() > 0.0:
            wish.normalize()
        desired = wish * WALK_SPEED

        speed = self.physics_ref.speed
        into = speed.dot(normal)
        tangential = speed - normal * into
        # Standing absorbs the pull into the surface instead of banking it.
        # Carrying it would let gravity build up between contacts and bounce.
        into = max(0.0, into)
        change = vec3d(desired) - tangential
        limit = GROUND_ACCELERATION * dt
        if change.length() > limit:
            change *= limit / change.length()
        self.physics_ref.speed = tangential + change + normal * into

        if state.up_down > 0.0 and not self._jump_requires_release:
            self.physics_ref.speed += normal * JUMP_SPEED
            self._jump_timer = JUMP_LOCKOUT
            self._jump_requires_release = True
            self.grounded = False

    # -- the whole per-step update ----------------------------------------

    def update(self, state, dt, ground_body=None, ground_normal=None):
        if state.up_down <= 0.0:
            self._jump_requires_release = False
        self._jump_timer = max(0.0, self._jump_timer - dt)
        grounded = ground_body is not None and self._jump_timer <= 0.0

        if grounded and not self.grounded:
            self.enter_walk(ground_normal)
        self.grounded = grounded
        self.ground_body = ground_body if grounded else None
        self.ground_normal = ground_normal if grounded else None

        # No rolling on foot -- the horizontal axis is needed for turning.
        self.is_rolling = state.roll and not grounded

        if grounded:
            self._walk(state, dt, ground_body, ground_normal)
        else:
            self.apply_look(state, dt)
            self.apply_thrust(
                state, dt, suppress_upward_thrust=self._jump_requires_release
            )
        self.update_camera(dt)
