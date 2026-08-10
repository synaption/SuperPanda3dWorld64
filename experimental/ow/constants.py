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


# -- gravity sourcing ---------------------------------------------------------
# The original sums the pull of every body at once. With the linear falloff
# that leaves the net field tens of degrees off the local vertical on a
# surface, so you slide. GRAVITY_NEAREST takes only the closest body, which
# points gravity straight down wherever you stand and makes walking possible.
GRAVITY_NEAREST = "nearest"
GRAVITY_ALL = "all"          # what AC_GravityComponent actually does
DEFAULT_GRAVITY_MODE = GRAVITY_NEAREST


# -- walking ------------------------------------------------------------------
#: Top speed on foot, cm/s.
WALK_SPEED = 600.0
#: How hard you accelerate toward that -- and, with no input, decelerate to a
#: stop. One constant does both, so it doubles as ground friction.
GROUND_ACCELERATION = 3000.0
#: Upward kick when jumping, cm/s. About 1.5 m of hop at Hearth's gravity.
JUMP_SPEED = 700.0
#: Counted as standing on a surface within this gap, cm. Walking a sphere in
#: straight tangent steps lifts you very slightly off it between contacts, so
#: this has to clear that hop or `grounded` flickers and control drops out.
GROUND_TOLERANCE = 15.0
#: Ignore the ground for this long after jumping, s, so a jump can clear it.
JUMP_LOCKOUT = 0.15
#: How fast your feet swing round to the surface on landing, per second.
GROUND_ALIGN_RATE = 8.0
#: Look limit on foot, degrees from the horizon.
MAX_GROUND_PITCH = 85.0
#: Camera follow rate while walking, per second. CameraLag's 2.25 is a ~0.44 s
#: time constant -- lovely for drifting in space, unusably sluggish for
#: mouse-look on foot. The *arrow* still eases round to the surface at
#: GROUND_ALIGN_RATE, so landing keeps its roll-upright.
GROUND_CAMERA_LAG = 20.0

#: Physics runs on a fixed step. The original ticks on the render frame, which
#: makes orbits drift with framerate; a fixed step keeps the demo system stable
#: without changing any of the arithmetic.
FIXED_TIMESTEP = 1.0 / 60.0
MAX_STEPS_PER_FRAME = 8
