"""Port of AC_GravityComponent.

The original is an actor component attached to the player and to every planet.
Each tick it walks every other actor tagged "PhysicsObject", turns the pull
from each into a force, sums them, and integrates its owner's position.

The one thing worth reading twice is the falloff. The Unreal graph carries this
comment on the Fg node:

    Use physics formula: Fg = (G * m * M) / r^2. We substitute r^2 for just r
    for gameplay reasons: a delicate balance between feeling gravity from afar
    vs. on-planet.

So the falloff is linear, not inverse-square. That is deliberate and it is what
makes the demo system's surface gravity land near 1 g while still being
noticeable between planets.  The port retains that behaviour close to a body,
then clamps it at a chosen surface clearance so distant gravity is constant.
"""

import math

from panda3d.core import LVector3d

from .constants import (
    COLLISION_SKIN,
    DEFAULT_GRAVITY_MODE,
    GRAVITY_CONSTANT,
    GRAVITY_LINEAR_FALLOFF_DISTANCE,
    GRAVITY_NEAREST,
    GROUND_TOLERANCE,
)


def sweep_sphere(origin, delta, centre, radius):
    """First t in [0,1] where a point travelling origin->origin+delta touches
    the sphere (centre, radius). None if it never does.

    The moving body's own radius is folded into `radius` by the caller, which
    is the standard reduction of sphere-vs-sphere to point-vs-sphere.
    """
    m = origin - centre
    a = delta.dot(delta)
    if a <= 0.0:
        return None
    c = m.dot(m) - radius * radius
    if c <= 0.0:
        return 0.0  # already interpenetrating
    b = 2.0 * m.dot(delta)
    if b >= 0.0:
        return None  # moving away
    discriminant = b * b - 4.0 * a * c
    if discriminant < 0.0:
        return None
    t = (-b - math.sqrt(discriminant)) / (2.0 * a)
    return t if 0.0 <= t <= 1.0 else None


def vec3d(value):
    """Coerce anything vector-shaped to a double-precision vector.

    Positions here reach ~5e5 cm and are integrated over long runs; Panda's
    default vectors are float32, which loses roughly a centimetre of precision
    at that magnitude and lets momentum drift visibly. Orientation stays in
    float32 -- it is bounded and feeds the scene graph directly.
    """
    return LVector3d(float(value[0]), float(value[1]), float(value[2]))


class GravityComponent:
    """One gravitating body. Holds its own position and velocity.

    Position and velocity live here rather than on a scene node so the
    simulation can run headless; the renderer reads them back each frame.
    """

    def __init__(
        self,
        name,
        mass,
        position=(0.0, 0.0, 0.0),
        initial_speed=(0.0, 0.0, 0.0),
        radius=0.0,
        is_planet=False,
        collides=True,
        gravity_constant=GRAVITY_CONSTANT,
        gravity_linear_falloff_distance=GRAVITY_LINEAR_FALLOFF_DISTANCE,
        gravity_mode=DEFAULT_GRAVITY_MODE,
    ):
        self.name = name
        self.mass_self = float(mass)
        self.position = vec3d(position)
        #: "Speed" in the original -- a velocity vector, not a magnitude.
        self.speed = vec3d(initial_speed)
        self.initial_speed = vec3d(initial_speed)
        self.radius = float(radius)
        self.is_planet = is_planet
        #: BlockAll on both the planets' CollisionBox and the player's root.
        self.collides = collides
        self.previous_position = vec3d(position)
        self.gravity_constant = gravity_constant
        #: Surface clearance where the 1 / r field becomes a constant pull.
        #: Set to ``math.inf`` to retain unlimited 1 / r falloff.
        self.gravity_linear_falloff_distance = float(gravity_linear_falloff_distance)
        #: GRAVITY_NEAREST or GRAVITY_ALL; see gravity_sources().
        self.gravity_mode = gravity_mode

        #: ListForces: everything pushing on this body this tick. Jetpack
        #: thrust lands here too, via add_force().
        self.list_forces = []
        self.net_force = LVector3d(0, 0, 0)
        self.acceleration = LVector3d(0, 0, 0)
        #: Gravity alone, kept apart from thrust so a readout can report the
        #: pull the player is actually under rather than pull plus jetpack.
        self.gravity_force = LVector3d(0, 0, 0)
        #: When False the body is inert -- the original's bIsZeroG toggle,
        #: there so another movement system can take over.
        self.is_zero_g = True

    # -- BFL_ZeroGFunctions.Add Force to Array -----------------------------

    def add_force(self, force):
        """Queue a force for this tick. Cleared after every integration."""
        self.list_forces.append(vec3d(force))

    # -- CalculateFg -------------------------------------------------------

    def gravitational_force_toward(self, other):
        """Pull toward ``other``: 1 / r near it, constant beyond its range.

        The range is measured from the other body's surface, which makes the
        same tuning meaningful for planets of different sizes.  Clamping the
        denominator makes the transition continuous: at the range boundary,
        both formulas yield exactly the same force.
        """
        delta = other.position - self.position
        distance = delta.length()
        if distance <= 0.0:
            return LVector3d(0, 0, 0)
        linear_limit = other.radius + other.gravity_linear_falloff_distance
        effective_distance = min(distance, linear_limit)
        magnitude = (
            self.gravity_constant * self.mass_self * other.mass_self
        ) / effective_distance
        return (delta / distance) * magnitude

    def accumulate_gravity(self, bodies, planets_attract_each_other=False):
        """Add the pull of every other body to this tick's force list.

        With `planets_attract_each_other` off -- the original's default -- a
        planet feels nothing at all, not merely nothing from other planets.
        The variable's tooltip is explicit: "Disables gravity between planets
        if False. If False, only player will be affected by gravity." Letting
        planets still feel the *player* would be wrong twice over: the demo
        level gives nothing an orbital velocity, so any mutual attraction
        collapses the system, and at these constants a mass-1 player drags a
        mass-3 planet around at metres per second.
        """
        total = LVector3d(0, 0, 0)
        if self.is_planet and not planets_attract_each_other:
            self.gravity_force = total
            return
        for other in self.gravity_sources(bodies):
            force = self.gravitational_force_toward(other)
            self.list_forces.append(force)
            total += force
        self.gravity_force = total

    def gravity_sources(self, bodies):
        """Which bodies pull on this one.

        In GRAVITY_ALL -- what the Unreal component does -- that is everything.
        In GRAVITY_NEAREST only the closest surface pulls, so gravity always
        points straight down at whatever you are standing on. That is the one
        change that makes surfaces walkable; summing every body leaves the
        field tens of degrees off vertical and slides you off.
        """
        others = [b for b in bodies if b is not self]
        if self.gravity_mode != GRAVITY_NEAREST or not others:
            return others
        return [min(others, key=lambda b: (b.position - self.position).length() - b.radius)]

    # -- standing on something ---------------------------------------------

    def find_ground(self, bodies, tolerance=GROUND_TOLERANCE):
        """The body this one is resting on, if any, with its surface normal.

        A proximity test rather than a reading of the last sweep: resting
        contact is re-established every step by gravity, and testing distance
        keeps `grounded` from flickering off on the steps in between.
        """
        if not self.collides:
            return None, None
        for other in bodies:
            if other is self or not other.collides or other.radius <= 0.0:
                continue
            offset = self.position - other.position
            distance = offset.length()
            if distance <= 0.0:
                continue
            if distance - (self.radius + other.radius) <= tolerance:
                return other, offset / distance
        return None, None

    # -- Tick --------------------------------------------------------------

    def integrate(self, dt):
        """Sum forces -> acceleration -> velocity -> position, then clear.

        Semi-implicit Euler, matching the original's node order (speed is
        updated before it is used to move).
        """
        net = LVector3d(0, 0, 0)
        for force in self.list_forces:
            net += force
        self.net_force = net

        self.previous_position = LVector3d(self.position)
        if self.mass_self != 0.0 and self.is_zero_g:
            self.acceleration = net / self.mass_self
            self.speed += self.acceleration * dt
            self.position += self.speed * dt
        else:
            self.acceleration = LVector3d(0, 0, 0)

        self.list_forces.clear()

    # -- the sweep on K2_AddActorWorldOffset -------------------------------

    def resolve_collision(self, bodies, max_contacts=3):
        """Stop at the first surface hit on the way to the new position.

        The original moves with `K2_AddActorWorldOffset(bSweep=true)` against
        `BlockAll` sphere colliders, so movement is swept and blocked. Unreal
        leaves the velocity untouched on a blocking hit, which means resting on
        a planet keeps accumulating speed into it -- seconds of that and you
        launch when you finally thrust away. Here the inbound (normal)
        component is removed on contact and the tangential part kept, so you
        settle onto a surface and can still slide along it.
        """
        if not self.collides:
            return
        remaining = self.position - self.previous_position
        position = LVector3d(self.previous_position)

        for _ in range(max_contacts):
            if remaining.lengthSquared() <= 0.0:
                break
            nearest_t, hit = None, None
            for other in bodies:
                if other is self or not other.collides or other.radius <= 0.0:
                    continue
                t = sweep_sphere(
                    position, remaining, other.position, self.radius + other.radius
                )
                if t is not None and (nearest_t is None or t < nearest_t):
                    nearest_t, hit = t, other
            if hit is None:
                position += remaining
                break

            position += remaining * nearest_t
            normal = position - hit.position
            length = normal.length()
            if length <= 0.0:
                break
            normal /= length
            # Sit exactly on the surface, plus a skin to avoid re-contact.
            position = hit.position + normal * (self.radius + hit.radius + COLLISION_SKIN)
            into = self.speed.dot(normal)
            if into < 0.0:
                self.speed -= normal * into
            leftover = remaining * (1.0 - nearest_t)
            remaining = leftover - normal * leftover.dot(normal)
        else:
            position += remaining

        self.position = position


class GravityWorld:
    """The set of "PhysicsObject" actors, and the tick that advances them."""

    def __init__(self, planets_attract_each_other=False):
        self.bodies = []
        self.planets_attract_each_other = planets_attract_each_other

    def add(self, body):
        self.bodies.append(body)
        return body

    def step(self, dt):
        """Advance every body by dt.

        Gravity for all bodies is accumulated before any of them move, so the
        result does not depend on list order. Forces already queued by the
        jetpack this tick are preserved.
        """
        for body in self.bodies:
            body.accumulate_gravity(self.bodies, self.planets_attract_each_other)
        for body in self.bodies:
            body.integrate(dt)
        # Sweeping runs after every body has moved, so a contact is resolved
        # against this step's positions rather than a half-updated world.
        for body in self.bodies:
            body.resolve_collision(self.bodies)

    # -- BFL_ZeroGFunctions."Enable/Disable Zero-G" ------------------------

    def set_zero_g(self, enabled):
        for body in self.bodies:
            body.is_zero_g = enabled

    def nearest_body(self, position, exclude=None):
        """Closest body to `position`, by surface distance. Used by the HUD."""
        best, best_gap = None, None
        for body in self.bodies:
            if body is exclude:
                continue
            gap = (body.position - position).length() - body.radius
            if best_gap is None or gap < best_gap:
                best, best_gap = body, gap
        return best, best_gap
