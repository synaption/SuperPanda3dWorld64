"""Which animation each action plays.

Animation ids are the decomp's MARIO_ANIM_* values; the exported glTF names
each clip `anim_XX` after its hex id, so the id is all that needs storing here.

A few actions pick their clip from state rather than a fixed choice -- walking
switches between tiptoe, walk and run by speed, and the rising and falling
halves of a double jump are different clips -- so entries may be a callable.

Clips that did not come out of the decomp have no number to be named after, so
their id is the name itself and they end up as `anim_zombie_walk` and the like.
Everything downstream keys on the id without caring which kind it is.
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
ANIM_DROWNING_PART1 = 0xA5
ANIM_WATER_DYING = 0xA7
ANIM_FALL_FROM_WATER = 0xA9
ANIM_SWIM_PART1 = 0xAA
ANIM_SWIM_PART2 = 0xAB
ANIM_FLUTTERKICK = 0xAC
ANIM_WATER_ACTION_END = 0xAD
ANIM_WATER_IDLE = 0xB2

# Retargeted onto Mario's skeleton from the mesh2motion animation library by
# tools/retarget_anim.py, which is also where the clip names come from.
ANIM_ZOMBIE_WALK = "zombie_walk"
ANIM_ZOMBIE_IDLE = "zombie_idle"

# Authored rather than borrowed, by tools/author_skate.py -- nothing in the
# mesh2motion library skates.
ANIM_SKATE_STRIDE = "skate_stride"
ANIM_SKATE_GLIDE = "skate_glide"


def anim_name(anim_id):
    """Clip name in the exported glTF."""
    if isinstance(anim_id, str):
        return f"anim_{anim_id}"
    return f"anim_{anim_id:02X}"


# -- state-dependent choices ------------------------------------------------


# The thresholds the original switches on, and the slack either side of them.
#
# Speed sawtooths by about a unit a frame -- the walk code adds acceleration
# below its target and subtracts a flat unit above it, and check_movement
# measures the result settling between 31.01 and 32.35 -- so a bare threshold
# is crossed back and forth on consecutive frames wherever Mario happens to
# settle near one. Every crossing restarts the clip from frame zero, which
# reads as the animation stuttering rather than as him changing gait.
TIPTOE_SPEED = 5.0
RUN_ANIM_SPEED = 22.0
GAIT_HYSTERESIS = 2.0


def _walking(m):
    """Tiptoe, walk or run, chosen by how fast Mario is actually going.

    The original chooses on whichever is larger, his speed or how far the
    stick is pushed -- `val04` in `anim_and_audio_for_walk`. That works on a
    stick that reports a range. It does not work on a keyboard, where a
    pressed key is always a full deflection and `intended_mag` is therefore
    pinned at its ceiling of 32: the comparison picks 32 every frame, so the
    run clip comes out on the first frame of movement and plays at its top
    cadence while Mario is still crawling forward at a quarter of that speed.
    That is the "screwed up" walk -- his legs sprinting while he creeps.

    Choosing on speed alone gives back what the original shows on a stick:
    tiptoe, then walk, then run, as he gets up to it.
    """
    speed = abs(m.forward_vel)
    current = m.gait_anim

    # Widen whichever band he is already in, so the sawtooth cannot flicker:
    # the gait he is already showing keeps its threshold on the far side.
    slack = GAIT_HYSTERESIS
    tiptoe_at = TIPTOE_SPEED + slack if current == ANIM_TIPTOE else TIPTOE_SPEED
    run_at = RUN_ANIM_SPEED - slack if current == ANIM_RUNNING else RUN_ANIM_SPEED

    if speed < tiptoe_at:
        chosen = ANIM_TIPTOE
    elif speed > run_at:
        chosen = ANIM_RUNNING
    else:
        chosen = ANIM_WALKING
    m.gait_anim = chosen
    return chosen


def _double_jump(m):
    return ANIM_DOUBLE_JUMP_RISE if m.vel[1] >= 0.0 else ANIM_DOUBLE_JUMP_FALL


def _long_jump(m):
    return ANIM_FAST_LONGJUMP if m.forward_vel > 16.0 else ANIM_SLOW_LONGJUMP


def _ground_pound(m):
    # The spin comes first, then the drop.
    return ANIM_START_GROUND_POUND if m.action_state == 0 else ANIM_GROUND_POUND


def _skating(m):
    """Pushing or coasting, which is what the action already tracks."""
    return ANIM_SKATE_STRIDE if m.action_state else ANIM_SKATE_GLIDE


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
    C.ACT_SKATING: _skating,
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

    # submerged
    #
    # The stroke is two clips, not one: PART1 is the arms sweeping out and
    # PART2 the glide as they recover. Breaststroke runs a 14-frame timer and
    # PART1 is exactly 13 frames, so it plays through once per stroke with no
    # rate scaling -- the clip and the action were authored to the same length.
    C.ACT_BREASTSTROKE: ANIM_SWIM_PART1,
    C.ACT_SWIMMING_END: ANIM_SWIM_PART2,
    C.ACT_FLUTTER_KICK: ANIM_FLUTTERKICK,
    C.ACT_WATER_IDLE: ANIM_WATER_IDLE,
    C.ACT_WATER_ACTION_END: ANIM_WATER_ACTION_END,
    # Plunging keeps the falling pose until he settles.
    C.ACT_WATER_PLUNGE: ANIM_GENERAL_FALL,
    C.ACT_WATER_JUMP: ANIM_FALL_FROM_WATER,
}

# What holding the zombie key replaces, and nothing else.
#
# Only the actions that keep Mario upright on the ground are covered. Jumping,
# diving and swimming are left alone deliberately: there is no zombie clip for
# any of them, and the alternative to leaving them be is a shamble that drops
# out the moment he leaves the floor and comes back when he lands.
#
# Turning around is in here because it is what walking passes through to
# reverse. Without it the shamble flickers back to Mario's own turn clip for
# the few frames that takes.
ZOMBIE_ANIMATIONS = {
    C.ACT_IDLE: ANIM_ZOMBIE_IDLE,
    C.ACT_BRAKING_STOP: ANIM_ZOMBIE_IDLE,

    C.ACT_WALKING: ANIM_ZOMBIE_WALK,
    C.ACT_DECELERATING: ANIM_ZOMBIE_WALK,
    C.ACT_BRAKING: ANIM_ZOMBIE_WALK,
    C.ACT_TURNING_AROUND: ANIM_ZOMBIE_WALK,
    C.ACT_FINISH_TURNING_AROUND: ANIM_ZOMBIE_WALK,
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
    # The stroke halves each play once through and are re-triggered by the
    # action, not by looping: letting PART1 repeat would restart the arm sweep
    # in the middle of the glide.
    ANIM_SWIM_PART1, ANIM_SWIM_PART2, ANIM_WATER_ACTION_END,
    ANIM_FALL_FROM_WATER, ANIM_WATER_DYING,
}

# Swimming holds Mario off the ground by definition, so a grounding check has
# nothing to say about these.
SUBMERGED_ANIMS = {
    ANIM_SWIM_PART1, ANIM_SWIM_PART2, ANIM_FLUTTERKICK, ANIM_WATER_IDLE,
    ANIM_WATER_ACTION_END, ANIM_FALL_FROM_WATER, ANIM_WATER_DYING,
    ANIM_DROWNING_PART1,
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
    # Chosen so one shamble cycle takes as long as one walk cycle at the same
    # speed: 4 * 77/40, the walk's divisor scaled by the ratio of clip
    # lengths. Any other number makes the feet slide worse than Mario's own
    # walk does, and matching his walk is the most that is available -- the
    # retargeted clip is a fixed stride and the walk it stands in for is not.
    ANIM_ZOMBIE_WALK: 7.7,
    # Picked by eye rather than by stride length, and the reason is the point
    # of the thing: a blade slides. Matching the clip's foot travel to the
    # ground he covers -- which is what keeps a walk cycle honest -- would
    # have the stride blurring past at skating speed. This is roughly a cycle
    # every three quarters of a second at a cruise.
    ANIM_SKATE_STRIDE: 12.0,
}

# Slowest an animation is allowed to crawl along at.
MIN_PLAY_RATE = 1.0 / 16.0


def play_rate(m, anim_id):
    """Playback multiplier for a clip, given Mario's current speed.

    The original drives this from whichever is larger, his speed or how hard
    the stick is pushed. On a keyboard the second of those is a constant --
    see `_walking` -- and taking the larger of the two pins every cycle at the
    cadence of a full sprint however slowly he is moving, which is what made
    the walk look broken. His actual speed is the half of that comparison
    which still means something here.
    """
    if anim_id == ANIM_CRAWLING:
        # The original scales the crawl by the stick alone, which on a
        # keyboard is 32 and would whirr; his crawl tops out near 10, and a
        # matching cadence is what the doubling is there to give.
        return max(abs(m.forward_vel) * 2.0, MIN_PLAY_RATE)

    divisor = SPEED_SCALED.get(anim_id)
    if divisor is None:
        return 1.0

    speed = max(abs(m.forward_vel), 4.0)
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


def frame_count(clip_name):
    entry = _clip_metadata.get(clip_name)
    return int(entry.get("frames", 0)) if entry else 0


def loop_range(clip_name):
    """(loop_start, loop_end) for a clip, falling back to its full length.

    Animation headers carry their own loop points, and for most clips they
    span the whole thing. Where they do not, repeating the entire clip
    replays a lead-in that was only ever meant to play once.
    """
    entry = _clip_metadata.get(clip_name)
    if not entry:
        return 0, 0
    frames = int(entry.get("frames", 0))
    start = int(entry.get("loop_start", 0))
    end = int(entry.get("loop_end", frames)) or frames
    return start, min(end, frames) if frames else end


# Above this speed the flutter kick stops re-asserting its clip, so whatever
# was already playing keeps running -- Mario streamlines instead of kicking.
FLUTTER_KICK_ANIM_SPEED = 14.0


# The two frames of each cycle a foot lands on. These are the original's own
# numbers, and they are why the cadence comes out right without tuning: the
# clip is played back at speed/4, so the interval between footfalls follows
# Mario's speed automatically. Both frames sit at roughly 1/8 and 5/8 through
# their clip, one per foot.
STEP_FRAMES = {
    ANIM_WALKING: (10, 49),
    ANIM_RUNNING: (9, 45),
    ANIM_TIPTOE: (14, 72),
    ANIM_START_TIPTOE: (7, 22),
    ANIM_CRAWLING: (26, 79),
    # Not the decomp's numbers, since the clip is not the decomp's. These are
    # the frames tools/retarget_anim.py measured each ankle reaching furthest
    # forward, which is where the foot comes down.
    ANIM_ZOMBIE_WALK: (0, 19),
}


def action_anim(m):
    """The clip the current action calls for.

    Every read of the table goes through here, so the zombie substitution
    cannot end up applied to what is drawn but not to the footfall timing or
    the clip length actions measure themselves against.
    """
    if m.controller.zombie and m.action in ZOMBIE_ANIMATIONS:
        return ZOMBIE_ANIMATIONS[m.action]
    entry = ACTION_ANIMATIONS.get(m.action, ANIM_A_POSE)
    return entry(m) if callable(entry) else entry


def advance_frame(m):
    """Advance the playing clip and report whether a foot just landed.

    The simulation tracks its own animation frame rather than asking the
    renderer, so footfalls stay in step with the clip whether or not anything
    is being drawn.
    """
    anim = action_anim(m)

    steps = STEP_FRAMES.get(anim)
    length = frame_count(anim_name(anim))
    if steps is None or length <= 0:
        m.anim_frame = 0.0
        return False

    rate = play_rate(m, anim)
    previous = m.anim_frame
    current = previous + rate
    m.anim_frame = current % length

    # A footfall counts if it falls in the span just covered. The span can be
    # longer than a whole cycle at speed, so it is walked rather than tested
    # as a single interval.
    for frame in steps:
        for turn in range(int(current // length) + 1):
            if previous < frame + turn * length <= current:
                return True
    return False


def action_frame_count(m):
    """Length of the clip the current action plays, in frames.

    Actions that run until their animation finishes need this, and asking here
    rather than being told by the renderer keeps the simulation self-contained.
    Returns 0 when no metadata has been loaded, which makes such an action end
    immediately rather than hang.
    """
    return frame_count(anim_name(action_anim(m)))


def resolve(m):
    """Clip to play for the current action.

    Returns (clip_name, should_loop, play_rate), or None when the action
    deliberately leaves the current clip alone.
    """
    if m.action == C.ACT_FLUTTER_KICK and m.forward_vel >= FLUTTER_KICK_ANIM_SPEED:
        return None

    anim = action_anim(m)
    return (anim_name(anim), anim not in NON_LOOPING, play_rate(m, anim))
