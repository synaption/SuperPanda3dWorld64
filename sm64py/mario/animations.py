"""Which animation each action plays.

Animation ids are the decomp's MARIO_ANIM_* values; the exported glTF names
each clip `anim_XX` after its hex id, so the id is all that needs storing here.

A few actions pick their clip from state rather than a fixed choice -- walking
switches between tiptoe, walk and run by speed, and the rising and falling
halves of a double jump are different clips -- so entries may be a callable.
"""

import json

from . import constants as C

# -- animation ids ----------------------------------------------------------
ANIM_FALL_OVER_BACKWARDS = 0x01
ANIM_BACKWARD_AIR_KB = 0x02
ANIM_BACKFLIP = 0x04
ANIM_FAST_LONGJUMP = 0x13
ANIM_SLOW_LONGJUMP = 0x14
ANIM_A_POSE = 0x0E
ANIM_IDLE_ON_LEDGE = 0x33
ANIM_GROUND_POUND_LANDING = 0x3A
ANIM_START_GROUND_POUND = 0x3C
ANIM_GROUND_POUND = 0x3D
ANIM_GENERAL_LAND = 0x57
ANIM_FIRST_PUNCH = 0x67
ANIM_SECOND_PUNCH = 0x68
ANIM_FIRST_PUNCH_FAST = 0x69
ANIM_SLIDEFLIP_LAND = 0xBE
ANIM_WALKING = 0x48
ANIM_LAND_FROM_DOUBLE_JUMP = 0x4B
ANIM_DOUBLE_JUMP_FALL = 0x4C
ANIM_SINGLE_JUMP = 0x4D
ANIM_LAND_FROM_SINGLE_JUMP = 0x4E
ANIM_AIR_KICK = 0x4F
ANIM_DOUBLE_JUMP_RISE = 0x50
ANIM_GENERAL_FALL = 0x56
ANIM_RUNNING = 0x72
ANIM_SOFT_BACK_KB = 0x74
ANIM_DIVE = 0x88
ANIM_SLIDE_KICK = 0x8C
ANIM_STOP_SLIDE = 0x8F
ANIM_SLIDE = 0x91
ANIM_TIPTOE = 0x92
ANIM_STOP_CROUCHING = 0x96
ANIM_START_CROUCHING = 0x97
ANIM_CROUCHING = 0x98
ANIM_CRAWLING = 0x99
ANIM_TURNING_PART1 = 0xBC
ANIM_TURNING_PART2 = 0xBD
ANIM_SLIDEFLIP = 0xBF
ANIM_TRIPLE_JUMP_LAND = 0xC0
ANIM_TRIPLE_JUMP = 0xC1
ANIM_IDLE_HEAD_CENTER = 0xC5
ANIM_START_TIPTOE = 0xCA


def anim_name(anim_id):
    """Clip name in the exported glTF."""
    return f"anim_{anim_id:02X}"


# -- state-dependent choices ------------------------------------------------


def _walking(m):
    """Tiptoe, walk or run, chosen by speed the way the original does."""
    speed = max(abs(m.forward_vel), m.intended_mag)
    if speed < 5.0:
        return ANIM_TIPTOE
    if speed > 22.0:
        return ANIM_RUNNING
    return ANIM_WALKING


def _double_jump(m):
    return ANIM_DOUBLE_JUMP_RISE if m.vel[1] >= 0.0 else ANIM_DOUBLE_JUMP_FALL


def _long_jump(m):
    return ANIM_FAST_LONGJUMP if m.forward_vel > 16.0 else ANIM_SLOW_LONGJUMP


def _ground_pound(m):
    # The spin comes first, then the drop.
    return ANIM_START_GROUND_POUND if m.action_state == 0 else ANIM_GROUND_POUND


ACTION_ANIMATIONS = {
    # stationary
    C.ACT_IDLE: ANIM_IDLE_HEAD_CENTER,
    C.ACT_BRAKING_STOP: ANIM_IDLE_HEAD_CENTER,
    C.ACT_START_CROUCHING: ANIM_START_CROUCHING,
    C.ACT_CROUCHING: ANIM_CROUCHING,
    C.ACT_STOP_CROUCHING: ANIM_STOP_CROUCHING,
    C.ACT_PUNCHING: ANIM_FIRST_PUNCH,
    C.ACT_GROUND_POUND_LAND: ANIM_GROUND_POUND_LANDING,
    C.ACT_BUTT_SLIDE_STOP: ANIM_STOP_SLIDE,

    # moving
    C.ACT_WALKING: _walking,
    C.ACT_DECELERATING: ANIM_WALKING,
    C.ACT_BRAKING: ANIM_STOP_SLIDE,
    C.ACT_TURNING_AROUND: ANIM_TURNING_PART1,
    C.ACT_FINISH_TURNING_AROUND: ANIM_TURNING_PART2,
    C.ACT_CRAWLING: ANIM_CRAWLING,
    C.ACT_BUTT_SLIDE: ANIM_SLIDE,
    C.ACT_STOMACH_SLIDE: ANIM_DIVE,
    C.ACT_DIVE_SLIDE: ANIM_DIVE,
    C.ACT_MOVE_PUNCHING: ANIM_FIRST_PUNCH_FAST,

    # landings
    C.ACT_JUMP_LAND: ANIM_LAND_FROM_SINGLE_JUMP,
    C.ACT_JUMP_LAND_STOP: ANIM_LAND_FROM_SINGLE_JUMP,
    C.ACT_FREEFALL_LAND: ANIM_LAND_FROM_SINGLE_JUMP,
    C.ACT_FREEFALL_LAND_STOP: ANIM_LAND_FROM_SINGLE_JUMP,
    C.ACT_DOUBLE_JUMP_LAND: ANIM_LAND_FROM_DOUBLE_JUMP,
    C.ACT_DOUBLE_JUMP_LAND_STOP: ANIM_LAND_FROM_DOUBLE_JUMP,
    C.ACT_TRIPLE_JUMP_LAND: ANIM_TRIPLE_JUMP_LAND,
    C.ACT_TRIPLE_JUMP_LAND_STOP: ANIM_TRIPLE_JUMP_LAND,
    C.ACT_BACKFLIP_LAND: ANIM_TRIPLE_JUMP_LAND,
    C.ACT_BACKFLIP_LAND_STOP: ANIM_TRIPLE_JUMP_LAND,
    C.ACT_SIDE_FLIP_LAND: ANIM_SLIDEFLIP_LAND,
    C.ACT_SIDE_FLIP_LAND_STOP: ANIM_SLIDEFLIP_LAND,
    C.ACT_LONG_JUMP_LAND: ANIM_LAND_FROM_SINGLE_JUMP,
    C.ACT_LONG_JUMP_LAND_STOP: ANIM_LAND_FROM_SINGLE_JUMP,

    # airborne
    C.ACT_JUMP: ANIM_SINGLE_JUMP,
    C.ACT_DOUBLE_JUMP: _double_jump,
    C.ACT_TRIPLE_JUMP: ANIM_TRIPLE_JUMP,
    C.ACT_BACKFLIP: ANIM_BACKFLIP,
    C.ACT_SIDE_FLIP: ANIM_SLIDEFLIP,
    C.ACT_STEEP_JUMP: ANIM_SINGLE_JUMP,
    C.ACT_WALL_KICK_AIR: ANIM_SLIDEFLIP,
    C.ACT_LONG_JUMP: _long_jump,
    C.ACT_FREEFALL: ANIM_GENERAL_FALL,
    C.ACT_DIVE: ANIM_DIVE,
    C.ACT_GROUND_POUND: _ground_pound,
    C.ACT_SLIDE_KICK: ANIM_SLIDE_KICK,
    C.ACT_AIR_HIT_WALL: ANIM_BACKWARD_AIR_KB,
    C.ACT_SOFT_BONK: ANIM_SOFT_BACK_KB,
    C.ACT_BACKWARD_AIR_KB: ANIM_BACKWARD_AIR_KB,
    C.ACT_LEDGE_GRAB: ANIM_IDLE_ON_LEDGE,
}

# Clips that should hold on their last frame rather than repeat.
NON_LOOPING = {
    ANIM_SINGLE_JUMP, ANIM_DOUBLE_JUMP_RISE, ANIM_DOUBLE_JUMP_FALL,
    ANIM_TRIPLE_JUMP, ANIM_BACKFLIP, ANIM_SLIDEFLIP, ANIM_GENERAL_FALL,
    ANIM_DIVE, ANIM_SLIDE_KICK, ANIM_START_GROUND_POUND, ANIM_GROUND_POUND,
    ANIM_LAND_FROM_SINGLE_JUMP, ANIM_LAND_FROM_DOUBLE_JUMP,
    ANIM_TRIPLE_JUMP_LAND, ANIM_START_CROUCHING, ANIM_STOP_CROUCHING,
    ANIM_TURNING_PART1, ANIM_TURNING_PART2, ANIM_AIR_KICK,
    ANIM_BACKWARD_AIR_KB, ANIM_SOFT_BACK_KB, ANIM_STOP_SLIDE,
    ANIM_FAST_LONGJUMP, ANIM_SLOW_LONGJUMP,
    ANIM_GROUND_POUND_LANDING, ANIM_SLIDEFLIP_LAND, ANIM_GENERAL_LAND,
    ANIM_FIRST_PUNCH, ANIM_SECOND_PUNCH, ANIM_FIRST_PUNCH_FAST,
}

# Actions whose animation is authored below Mario's logical position on
# purpose, so a grounding check should not flag them. Hanging from a ledge is
# the obvious one: his position is the ledge top and his body dangles beneath.
EXPECTED_BELOW_GROUND = {C.ACT_LEDGE_GRAB}


# Clips whose playback rate follows Mario's speed, and the divisor each uses.
#
# The original scales these per frame rather than playing them at a fixed
# rate, which is what makes a run cycle keep up with a 32-unit stride instead
# of looking like a slow-motion walk.
SPEED_SCALED = {
    ANIM_START_TIPTOE: 4.0,
    ANIM_TIPTOE: 1.0,
    ANIM_WALKING: 4.0,
    ANIM_RUNNING: 4.0,
}

# Slowest an animation is allowed to crawl along at.
MIN_PLAY_RATE = 1.0 / 16.0


def play_rate(m, anim_id):
    """Playback multiplier for a clip, given Mario's current speed."""
    if anim_id == ANIM_CRAWLING:
        return max(m.intended_mag * 2.0, MIN_PLAY_RATE)

    divisor = SPEED_SCALED.get(anim_id)
    if divisor is None:
        return 1.0

    # The original drives this from whichever is larger: how fast Mario is
    # actually going, or how hard the stick is pushed.
    speed = max(abs(m.forward_vel), m.intended_mag, 4.0)
    return max(speed / divisor, MIN_PLAY_RATE)


_clip_metadata = {}


def load_clip_metadata(path):
    """Load the sidecar the exporter writes next to a .glb.

    It carries the per-clip start frame and loop points, which glTF has no
    place for. The start frame is not cosmetic: several clips have lead-in
    frames the game never shows, and playing them from zero sinks Mario
    through the floor during a landing.
    """
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


def resolve(m):
    """Return (clip_name, should_loop, play_rate) for Mario's current action."""
    entry = ACTION_ANIMATIONS.get(m.action, ANIM_A_POSE)
    anim_id = entry(m) if callable(entry) else entry
    return (anim_name(anim_id),
            anim_id not in NON_LOOPING,
            play_rate(m, anim_id))
