"""Which clip each of the Hero's actions plays.

Unlike Mario's, these names are not ids -- they are the action names out of
Blender, spaces, capitals, trailing space and all. They are quoted verbatim
rather than tidied up, because the .glb is the source of truth and renaming
them here would only mean a lookup that silently misses. `Idle ` really does
end in a space.

The interface is the one `app/main.py` already drives Mario through:
`load_clip_metadata`, `start_frame`, and a `resolve` returning
(clip, should_loop, play_rate). Nothing in the front end needs to know which
character it is animating.
"""

import json

from . import constants as H

# -- clips ------------------------------------------------------------------

IDLE = "Idle "
IDLE_VAR = "idle var"
WALK = "walk Var1"
RUN = "Running normal"
JUMP_RISE = "jump up"
JUMP_FALL = "jump down"
LAND = "jump Impact"
LAND_HEAVY = "jump Heavy"
ATTACK1 = "Attack 1 beta"
ATTACK2 = "Attack 2"
SPIN_KICK = "Spin Kick"
SWORD_DRAW = "sword draw"

# Clips that play once and hold their last pose. Everything else cycles.
NON_LOOPING = {
    JUMP_RISE, LAND, LAND_HEAVY, ATTACK1, ATTACK2, SPIN_KICK, SWORD_DRAW,
    IDLE_VAR,
}

# How long he stands still before the idle fidget plays instead, in frames.
# Long enough that it reads as him getting bored rather than as a twitch.
IDLE_VAR_AFTER = 240


# -- state-dependent choices ------------------------------------------------


def _idle(m):
    """The fidget, once he has been standing still for a while."""
    if m.action_timer > IDLE_VAR_AFTER:
        return IDLE_VAR
    return IDLE


def _walking(m):
    """Walk or run, chosen by speed the way Mario's cycle is."""
    speed = max(abs(m.forward_vel), m.intended_mag)
    return RUN if speed > H.RUN_SPEED else WALK


def _landing(m):
    return LAND_HEAVY if m.action_arg else LAND


def _attack(m):
    return ATTACK2 if m.combo_index else ATTACK1


ACTION_ANIMATIONS = {
    H.ACT_HERO_IDLE: _idle,
    H.ACT_HERO_WALKING: _walking,
    H.ACT_HERO_JUMP: JUMP_RISE,
    H.ACT_HERO_FALL: JUMP_FALL,
    H.ACT_HERO_LAND: _landing,
    H.ACT_HERO_ATTACK: _attack,
    H.ACT_HERO_SPIN_KICK: SPIN_KICK,
    H.ACT_HERO_SWORD: SWORD_DRAW,
    # Wading has no clip of its own, so it borrows the walk. Played slowly by
    # the rate below, it reads as pushing through water well enough.
    H.ACT_HERO_WADING: WALK,
}


# -- playback rate ----------------------------------------------------------

# Cycle clips are played at a rate proportional to speed, so the stride keeps
# up with the ground he covers instead of sliding.
#
# These are measured off the clips rather than borrowed from Mario's. Copying
# his divisors and scaling them by clip length -- which is how the retargeted
# zombie walk was fitted -- assumes the two characters take the same size step,
# and they do not: Mario's walk is 77 frames of a cartoon stride, the Hero's is
# 40 frames of a human one.
#
# What the number has to be is set by the geometry. For the feet not to slide,
# one cycle of the clip must play in exactly the time he takes to cover one
# stride, which works out as
#
#     divisor = stride / (30 * clip_duration)
#
# with the stride in game units. Measuring the planted foot's travel relative
# to the spine gives 160 units per cycle for the walk over 1.30s, and 267 over
# 1.37s for the run. tools/check_hero.py recomputes both from the .glb and
# fails if they drift, so a re-exported clip cannot quietly start skating.
SPEED_SCALED = {
    WALK: 4.11,
    RUN: 6.52,
}

MIN_PLAY_RATE = 1.0 / 16.0

# Nothing plays faster than this, however fast he is moving -- and this is the
# knob to turn if the legs look wrong.
#
# 1.0 means "exactly as authored in Blender": his run cycle is 42 frames at
# 30 fps, a slow, heavy 1.4 seconds, and at this cap that is what plays. It is
# the floor of the scale rather than a tuned number -- there is no speeding up
# left to remove.
#
# The reason a cap is needed at all is that the two things a locomotion clip
# should do cannot both be had here. Planting the feet means one cycle per
# stride covered, and at his top speed of 38 units a frame that is better than
# four cycles a second, because his stride is a human 267 units and Mario's
# speed was never meant for a character this size. So the cadence wins and the
# foot contact gives: he slides at speed. The divisors above still govern while
# he is moving slowly enough for the cap not to bind.
MAX_PLAY_RATE = 1.0


def play_rate(m, clip):
    divisor = SPEED_SCALED.get(clip)
    if divisor is None:
        return 1.0
    if m.action == H.ACT_HERO_WADING:
        # Wading is the walk clip at a fraction of its cadence; tying it to
        # speed as well would leave it crawling almost to a stop.
        return 0.6
    speed = max(abs(m.forward_vel), m.intended_mag, 4.0)
    return min(max(speed / divisor, MIN_PLAY_RATE), MAX_PLAY_RATE)


# -- clip metadata ----------------------------------------------------------

_clip_metadata = {}


def load_clip_metadata(path):
    """Load the sidecar written next to hero.glb."""
    global _clip_metadata
    try:
        with open(path, "r", encoding="utf-8") as fh:
            _clip_metadata = json.load(fh)
    except (OSError, ValueError):
        _clip_metadata = {}
    return _clip_metadata


def start_frame(clip_name):
    entry = _clip_metadata.get(clip_name)
    return int(entry.get("start_frame", 0)) if entry else 0


def frame_count(clip_name):
    entry = _clip_metadata.get(clip_name)
    return int(entry.get("frames", 0)) if entry else 0


def action_anim(m):
    entry = ACTION_ANIMATIONS.get(m.action, IDLE)
    return entry(m) if callable(entry) else entry


def resolve(m):
    """(clip, should_loop, play_rate) for the current action."""
    clip = action_anim(m)
    return clip, clip not in NON_LOOPING, play_rate(m, clip)
