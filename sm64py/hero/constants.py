"""The Hero's actions, and the numbers his movement is tuned with.

Action ids follow the same convention as the decomp's: the low bits identify
the action and the high bits are flags read by code that does not care which
action it is looking at. Two of those flags are load-bearing here rather than
decorative -- `ACT_FLAG_AIR` and `ACT_FLAG_CONTROL_JUMP_HEIGHT` are what make
`sm64py/mario/steps.py` treat a hero jump as a jump, including cutting it short
when the button is released on the way up.

Every id also carries `HERO`, a bit the decomp never sets. The step functions
compare `m.action` against a handful of Mario's ids (long jump, slide kick,
twirling) to pick a gravity rule; with this bit set no hero action can
accidentally equal one of them and inherit its physics.
"""

from ..mario.constants import (
    ACT_FLAG_AIR,
    ACT_FLAG_ATTACKING,
    ACT_FLAG_CONTROL_JUMP_HEIGHT,
    ACT_FLAG_IDLE,
    ACT_FLAG_MOVING,
    ACT_FLAG_STATIONARY,
)

# Unused by every ACT_* in the decomp, which is the whole point of it.
HERO = 1 << 30

ACT_HERO_IDLE = HERO | ACT_FLAG_STATIONARY | ACT_FLAG_IDLE | 0x01
ACT_HERO_WALKING = HERO | ACT_FLAG_MOVING | 0x02
ACT_HERO_JUMP = HERO | ACT_FLAG_AIR | ACT_FLAG_CONTROL_JUMP_HEIGHT | 0x03
ACT_HERO_FALL = HERO | ACT_FLAG_AIR | 0x04
ACT_HERO_LAND = HERO | ACT_FLAG_STATIONARY | 0x05
ACT_HERO_ATTACK = HERO | ACT_FLAG_STATIONARY | ACT_FLAG_ATTACKING | 0x06
ACT_HERO_SPIN_KICK = HERO | ACT_FLAG_MOVING | ACT_FLAG_ATTACKING | 0x07
ACT_HERO_SWORD = HERO | ACT_FLAG_STATIONARY | 0x08
ACT_HERO_WADING = HERO | ACT_FLAG_MOVING | 0x09
# No CONTROL_JUMP_HEIGHT: that flag is what lets releasing A cut a rise short,
# and here releasing A ends the flight outright.
ACT_HERO_JETPACK = HERO | ACT_FLAG_AIR | 0x0A

ACTION_NAMES = {
    ACT_HERO_IDLE: "idle",
    ACT_HERO_WALKING: "walking",
    ACT_HERO_JUMP: "jump",
    ACT_HERO_FALL: "fall",
    ACT_HERO_LAND: "land",
    ACT_HERO_ATTACK: "attack",
    ACT_HERO_SPIN_KICK: "spin kick",
    ACT_HERO_SWORD: "sword",
    ACT_HERO_WADING: "wading",
    ACT_HERO_JETPACK: "jetpack",
}

# -- ground movement --------------------------------------------------------
#
# Kept in the decomp's units -- per 30 Hz frame, and tuned against the same
# quarter-step collision -- because that is what the level was built for.
# The Hero is quicker off the mark than Mario and now outruns him too: 38
# against Mario's 32, which is the one place his movement is deliberately not
# Mario's.

WALK_ACCEL = 1.4                # per frame, before the taper below
ACCEL_TAPER = 43.0              # acceleration falls away as speed approaches this
MAX_WALK_SPEED = 38.0
MAX_RUN_SPEED = 38.0

# The speed ACCEL_TAPER was tuned at, which is not the same thing as whatever
# the caps above happen to say today: the taper is scaled by the ratio between
# them so the ramp keeps its shape when the top speed moves. Left at the value
# the number was authored against rather than following the caps, so raising
# the top speed does not quietly re-tune the acceleration curve as well.
TAPER_REFERENCE_SPEED = 30.0

# What a fully pressed stick reports. The magnitude is squared into 0..64 and
# halved (see MarioState.update_inputs), so this is the ceiling, and the speed
# caps above are read as a fraction of it rather than clipped against it --
# otherwise no cap above 32 could ever be reached, and the debug console's
# run_speed slider would go dead halfway along.
MAX_STICK_MAG = 32.0
DECELERATION = 1.8              # per frame with no stick
BRAKE_DECELERATION = 3.0        # per frame when reversing hard

# Above this he is running rather than walking, and the run clip takes over.
RUN_SPEED = 15.0

# How fast he turns toward the stick, in s16 angle units per frame. Higher
# than Mario's 0x800: he pivots rather than arcing, which suits a character
# who stops to swing a sword.
TURN_RATE = 0x0C00

# -- jumping ----------------------------------------------------------------

# Mario's own take-off velocity, and for a reason rather than by imitation: it
# puts a jump held to the last frame before the boosters at ~250 units
# (tools/check_movement.py, tools/check_hero.py),
# which is the height the castle grounds were built around. A Hero who jumped
# higher would reach ledges the level does not expect anyone to stand on.
JUMP_VELOCITY = 42.0
# Running lends a little height, the way it does in the original.
JUMP_SPEED_BONUS = 0.15
# Landing harder than this plays the heavy landing instead of the light one.
HEAVY_LANDING_SPEED = 52.0

# -- the jetpack ------------------------------------------------------------
#
# Hold A in the air and he keeps going up. The thrust is applied as an approach
# toward a rise speed rather than as an acceleration, so the boosters have a
# top speed of their own and holding the button does not build up something
# that takes a hundred units of altitude to shed.
#
# The thrust has to beat gravity to be thrust at all: `apply_gravity` takes 4
# units a frame back after every air step, and the approach below runs before
# the step, so anything at or under 4 hovers instead of climbing. 8 climbs
# briskly and still lets the descent take over the moment the button is let go.
JETPACK_THRUST = 8.0
JETPACK_RISE_SPEED = 20.0

# Frames of ordinary jump before the boosters light, so a tap is still a jump
# and a hold turns into flight. Short enough not to feel like a delay -- a
# fifth of a second -- and long enough that the take-off reads as a jump.
JETPACK_DELAY = 6
# Frames the landing pose holds before idle or walking takes over. The clip is
# 24 frames; this is short enough that it never blocks the next input.
LAND_FRAMES = 8

# -- combat -----------------------------------------------------------------

# The two swings, in frames. Taken from the clips themselves rather than
# guessed: 'Attack 1 beta' is 37 frames and 'Attack 2' is 29.
ATTACK1_FRAMES = 37
ATTACK2_FRAMES = 29
SPIN_KICK_FRAMES = 47

# When the second swing can be bought. Pressing B inside this window chains;
# pressing it outside starts over from the first swing.
COMBO_WINDOW_START = 14
COMBO_WINDOW_END = 37

# The lunge. `tools/lock_root_motion.py` takes the authored forward travel out
# of the attack clips so the animation cannot drag him through a wall; this is
# that travel handed back as velocity, where the quarter steps can stop it.
ATTACK_LUNGE_SPEED = 14.0
ATTACK_LUNGE_FRAMES = 8
SPIN_KICK_SPEED = 18.0

# Spinning takes a running start; from a standstill B swings the sword. Kept
# below MAX_RUN_SPEED with room to spare -- at or above it, the check can only
# pass on the one frame he is exactly at top speed, which in practice means
# the spin kick never comes out at all.
SPIN_KICK_MIN_SPEED = 18.0

SWORD_DRAW_FRAMES = 14

# -- water ------------------------------------------------------------------
#
# The Hero has no swimming clips, so there is no swimming: deep water slows him
# to a wade and holds him at the surface rather than putting him under. This is
# the one place his moveset is short of Mario's, and it is a gap in the source
# animation rather than something the controller could paper over.

WADE_SPEED_SCALE = 0.45
# How far below the surface he floats, in game units.
WADE_FLOAT_DEPTH = 60.0
