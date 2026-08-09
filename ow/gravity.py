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
noticeable between planets -- do not "fix" it.
"""

from panda3d.core import LVector3d

from .constants import GRAVITY_CONSTANT


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
        gravity_constant=GRAVITY_CONSTANT,
    ):
        self.name = name
        self.mass_self = float(mass)
        self.position = vec3d(position)
        #: "Speed" in the original -- a velocity vector, not a magnitude.
        self.speed = vec3d(initial_speed)
        self.initial_speed = vec3d(initial_speed)
        self.radius = float(radius)
        self.is_planet = is_planet
        self.gravity_constant = gravity_constant

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
        """Fg = (G * m * M) / r, directed at `other`. Zero if coincident."""
        delta = other.position - self.position
        distance = delta.length()
        if distance <= 0.0:
            return LVector3d(0, 0, 0)
        magnitude = (self.gravity_constant * self.mass_self * other.mass_self) / distance
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
        for other in bodies:
            if other is self:
                continue
            force = self.gravitational_force_toward(other)
            self.list_forces.append(force)
            total += force
        self.gravity_force = total

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

        if self.mass_self != 0.0 and self.is_zero_g:
            self.acceleration = net / self.mass_self
            self.speed += self.acceleration * dt
            self.position += self.speed * dt
        else:
            self.acceleration = LVector3d(0, 0, 0)

        self.list_forces.clear()


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
