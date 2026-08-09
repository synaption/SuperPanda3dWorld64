"""Tuning values, lifted verbatim from the Unreal project.

Every number here was read out of the binary uassets rather than re-guessed,
so the port starts from the same feel as the original:

    DA_CharacterVariables   -> CharacterVariables below
    AC_GravityComponent CDO -> GRAVITY_CONSTANT

Units are Unreal's: centimetres, degrees, seconds. Keeping them means the
constants transfer without a conversion factor hiding a bug.
"""

from dataclasses import dataclass

# AC_GravityComponent, "Gravity Constant": a constant number that impacts the
# strength of gravity.
GRAVITY_CONSTANT = 1.0e7

# The player's own mass. Gravitational acceleration is G*M/r either way -- the
# attracted body's mass cancels -- but the jetpack numbers below are *forces*,
# so a mass of 1 is what makes them read as accelerations in cm/s^2.
PLAYER_MASS = 1.0

# Actors that gravity applies to are tagged this way in the Unreal level.
PHYSICS_OBJECT_TAG = "PhysicsObject"

# BP_Planet's root is a USphereComponent ("CollisionBox", profile BlockAll) at
# its default radius of 32, with the visible mesh parented under it:
# /Engine/BasicShapes/Sphere is radius 50 and carries RelativeScale3D 0.65, so
# it draws at 32.5 -- deliberately sized to sit on the collision sphere. Both
# then scale by BP_Planet.Scale.
#: Collision radius per unit of BP_Planet.Scale.
PLANET_COLLISION_UNIT_RADIUS = 32.0
# The authored mesh scale of 0.65 draws at 32.5, aiming at the collider's 32
# but overshooting by 1.6% -- 0.64 would have matched exactly. At planet scale
# that gap is metres: on Hearth the mesh stands 226 cm proud of the surface you
# actually stop on, so landing buries the camera inside the sphere and you see
# nothing but back-faces. We draw at the collision radius instead, which is
# plainly what the 0.65 was reaching for.
#: Drawn radius per unit of BP_Planet.Scale.
PLANET_MESH_UNIT_RADIUS = PLANET_COLLISION_UNIT_RADIUS
#: What the Unreal asset actually specifies, kept for reference.
PLANET_AUTHORED_MESH_UNIT_RADIUS = 50.0 * 0.65

#: BP_Player's root sphere, at the same engine default and unscaled.
PLAYER_COLLISION_RADIUS = 32.0

#: Pushed this far past a contact, so resting on a surface does not re-trigger
#: the same hit on the following step.
COLLISION_SKIN = 0.5


@dataclass
class CharacterVariables:
    """DA_CharacterVariables. Exposed so it can be tweaked without touching
    the movement code, exactly as the data asset was."""

    #: Jetpack force along the direction arrow's forward/right axes.
    move_acceleration: float = 3000.0
    #: Jetpack force along the direction arrow's up axis.
    up_down_acceleration: float = 3000.0
    #: Force opposing the current velocity while braking. Negative by design.
    brake_acceleration: float = -4500.0
    #: How quickly the camera catches up to the direction arrow, per second.
    camera_lag: float = 2.25
    #: Look speed, degrees per second at full stick deflection.
    rotation_speed: float = 130.0
    #: Roll speed, degrees per second, used while the roll button is held.
    roll_speed: float = 130.0


#: Physics runs on a fixed step. The original ticks on the render frame, which
#: makes orbits drift with framerate; a fixed step keeps the demo system stable
#: without changing any of the arithmetic.
FIXED_TIMESTEP = 1.0 / 60.0
MAX_STEPS_PER_FRAME = 8
