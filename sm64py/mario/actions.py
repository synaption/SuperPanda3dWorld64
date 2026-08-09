"""Mario's action state machine.

Every frame exactly one action runs.  An action reads input, decides whether
to hand off to another action, adjusts velocity, and then calls one of the
step functions to actually move.  Actions are registered in ACTIONS and
dispatched by id; an action returning True means it changed action and wants
to run again this frame, which is how transitions resolve without a frame of
lag.
"""

import math

from ..math_util import approach_f32, approach_s32, atan2s, coss, s16, sins
from . import constants as C
from .steps import (
    perform_air_step,
    perform_ground_step,
    stationary_ground_step,
    stop_and_set_height_to_floor,
)

ACTIONS = {}


def action(action_id, anim=None):
    def register(fn):
        ACTIONS[action_id] = (fn, anim or fn.__name__)
        return fn
    return register


# -- transitions ------------------------------------------------------------


# Sounds raised on entering an action. Terrain-dependent ids get the floor
# folded in when they are played, so one entry covers every ground type.
#
# Actions never play anything themselves -- they append to sound_events and
# the front end decides what to do with it, so the simulation is identical
# whether or not there is an audio device.
_ENTRY_SOUNDS = {
    C.ACT_JUMP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAH_WAH_HOO),
    C.ACT_DOUBLE_JUMP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAH_WAH_HOO),
    C.ACT_TRIPLE_JUMP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAHOO),
    C.ACT_BACKFLIP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAH_WAH_HOO),
    C.ACT_SIDE_FLIP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAH_WAH_HOO),
    C.ACT_LONG_JUMP: (C.SOUND_ACTION_TERRAIN_JUMP, C.SOUND_MARIO_YAHOO),
    C.ACT_WALL_KICK_AIR: (C.SOUND_MARIO_YAH_WAH_HOO,),
    C.ACT_DIVE: (C.SOUND_MARIO_YAH_WAH_HOO,),
    C.ACT_JUMP_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_FREEFALL_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_DOUBLE_JUMP_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_TRIPLE_JUMP_LAND: (C.SOUND_ACTION_TERRAIN_HEAVY_LANDING,),
    C.ACT_BACKFLIP_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_SIDE_FLIP_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_LONG_JUMP_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    C.ACT_GROUND_POUND_LAND: (C.SOUND_ACTION_TERRAIN_HEAVY_LANDING,),
    C.ACT_SOFT_BONK: (C.SOUND_MARIO_OOOF,),
    C.ACT_AIR_HIT_WALL: (C.SOUND_MARIO_OOOF,),
    C.ACT_WATER_PLUNGE: (C.SOUND_ACTION_WATER_PLUNGE,),
}


# Below this he is creeping, and the quieter tiptoe sound is used.
TIPTOE_SPEED = 8.0


def step_sounds(m):
    """Raise a footfall on the frames of the walk cycle a foot lands on.

    Driven by the animation rather than by distance travelled. Distance was an
    approximation and a poor one: it fired every 52 units, which at a running
    960 units/sec is 18 footfalls a second against the original's 6.7 -- nearly
    three times too fast. Playing the clip at speed/4 makes the cadence follow
    Mario's speed on its own, with no constant to tune.
    """
    from . import animations
    if not animations.advance_frame(m):
        return
    m.sound_events.append(
        C.SOUND_ACTION_TERRAIN_STEP_TIPTOE if abs(m.forward_vel) < TIPTOE_SPEED
        else C.SOUND_ACTION_TERRAIN_STEP
    )


def set_mario_action(m, act, action_arg=0):
    group = act & C.ACT_GROUP_MASK
    if group == C.ACT_GROUP_MOVING:
        act = _set_action_moving(m, act, action_arg)
    elif group == C.ACT_GROUP_AIRBORNE:
        act = _set_action_airborne(m, act, action_arg)

    # Raised before the action changes, so a landing sound still sees the
    # floor Mario landed on rather than whatever he moves to next.
    if act != m.action:
        m.sound_events.extend(_ENTRY_SOUNDS.get(act, ()))

    m.flags &= ~(C.MARIO_ACTION_SOUND_PLAYED | C.MARIO_MARIO_SOUND_PLAYED)

    m.prev_action = m.action
    m.action = act
    m.action_arg = action_arg
    m.action_state = 0
    m.action_timer = 0
    return True


def _set_action_moving(m, act, action_arg):
    floor_class = m.get_floor_class()
    mag = min(m.intended_mag, 8.0)

    if act == C.ACT_WALKING:
        # Give Mario a nudge of starting speed, unless he'd just slip.
        if floor_class != C.SURFACE_CLASS_VERY_SLIPPERY:
            if 0.0 <= m.forward_vel < mag:
                m.forward_vel = mag
    elif act == C.ACT_SKATING:
        # Whatever he was already doing becomes the glide he starts from, so
        # the skates go on without a jolt at either end.
        m.set_forward_vel(m.forward_vel)
        m.slide_yaw = m.face_angle[1]
    elif act == C.ACT_BEGIN_SLIDING:
        act = C.ACT_BUTT_SLIDE if m.facing_downhill(False) else C.ACT_STOMACH_SLIDE

    return act


def _set_action_airborne(m, act, action_arg):
    if m.squish_timer != 0 or m.quicksand_depth >= 1.0:
        if act == C.ACT_DOUBLE_JUMP:
            act = C.ACT_JUMP

    if act == C.ACT_JUMP:
        m.set_y_vel_based_on_fspeed(42.0, 0.25)
        m.forward_vel *= 0.8
    elif act == C.ACT_DOUBLE_JUMP:
        m.set_y_vel_based_on_fspeed(52.0, 0.25)
        m.forward_vel *= 0.8
    elif act == C.ACT_TRIPLE_JUMP:
        m.set_y_vel_based_on_fspeed(69.0, 0.0)
        m.forward_vel *= 0.8
    elif act == C.ACT_BACKFLIP:
        m.forward_vel = -16.0
        m.set_y_vel_based_on_fspeed(62.0, 0.0)
    elif act == C.ACT_SIDE_FLIP:
        m.set_y_vel_based_on_fspeed(62.0, 0.0)
        m.forward_vel = 8.0
        m.face_angle[1] = m.intended_yaw
    elif act == C.ACT_WALL_KICK_AIR:
        m.set_y_vel_based_on_fspeed(62.0, 0.0)
        m.forward_vel = max(m.forward_vel, 24.0)
        m.wall_kick_timer = 0
    elif act == C.ACT_STEEP_JUMP:
        m.set_y_vel_based_on_fspeed(42.0, 0.25)
        m.face_angle[0] = -0x2000
    elif act == C.ACT_LONG_JUMP:
        m.set_y_vel_based_on_fspeed(30.0, 0.0)
        # Multiplying rather than clamping first is what lets backwards long
        # jumps accumulate speed without bound.
        m.forward_vel *= 1.5
        if m.forward_vel > 48.0:
            m.forward_vel = 48.0
    elif act == C.ACT_DIVE:
        m.set_forward_vel(min(m.forward_vel + 15.0, 48.0))
    elif act == C.ACT_SLIDE_KICK:
        m.vel[1] = 12.0
        m.forward_vel = max(m.forward_vel, 32.0)
    elif act == C.ACT_TWIRLING:
        m.vel[1] = 20.0

    m.peak_height = m.pos[1]
    m.flags |= C.MARIO_JUMPING
    return act


def set_jumping_action(m, act, action_arg=0):
    if m.floor_is_steep():
        _set_steep_jump_action(m)
    else:
        set_mario_action(m, act, action_arg)
    return True


def _set_steep_jump_action(m):
    m.face_angle[0] = 0
    if m.forward_vel > 0.0:
        # Redirect the jump down the slope rather than the way Mario faces.
        angle_temp = s16(m.floor_angle + 0x8000)
        y = m.forward_vel * coss(s16(m.face_angle[1] - angle_temp))
        x = m.forward_vel * sins(s16(m.face_angle[1] - angle_temp)) * 0.75
        m.forward_vel = math.sqrt(x * x + y * y)
        m.face_angle[1] = s16(atan2s(y, x) + angle_temp)

    set_mario_action(m, C.ACT_STEEP_JUMP, 0)


def set_jump_from_landing(m):
    """Chain into a double or triple jump if the timing window is open."""
    if m.floor_is_steep():
        _set_steep_jump_action(m)
    elif m.double_jump_timer == 0 or m.squish_timer != 0:
        set_mario_action(m, C.ACT_JUMP, 0)
    elif m.prev_action in (C.ACT_JUMP_LAND, C.ACT_FREEFALL_LAND, C.ACT_SIDE_FLIP_LAND):
        set_mario_action(m, C.ACT_DOUBLE_JUMP, 0)
    elif m.prev_action == C.ACT_DOUBLE_JUMP_LAND:
        if m.forward_vel > 20.0:
            set_mario_action(m, C.ACT_TRIPLE_JUMP, 0)
        else:
            set_mario_action(m, C.ACT_JUMP, 0)
    else:
        set_mario_action(m, C.ACT_JUMP, 0)

    m.double_jump_timer = 0
    return True


# -- shared predicates ------------------------------------------------------


def should_begin_sliding(m):
    if m.input & C.INPUT_ABOVE_SLIDE:
        return m.forward_vel <= -1.0 or m.facing_downhill(False)
    return False


def analog_stick_held_back(m):
    """True when the stick points far enough away from Mario's facing."""
    return not -0x471C <= s16(m.intended_yaw - m.face_angle[1]) <= 0x471C


def check_ground_dive_or_punch(m):
    if m.input & C.INPUT_B_PRESSED:
        # At speed with the stick pushed, B dives instead of punching.
        if m.forward_vel >= 29.0 and m.controller.stick_mag > 48.0:
            m.vel[1] = 20.0
            return set_mario_action(m, C.ACT_DIVE, 1)
        return set_mario_action(m, C.ACT_MOVE_PUNCHING, 0)
    return False


def check_common_action_exits(m):
    if m.input & C.INPUT_A_PRESSED:
        return set_mario_action(m, C.ACT_JUMP, 0)
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_mario_action(m, C.ACT_WALKING, 0)
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    return False


def begin_braking_action(m):
    if m.forward_vel >= 16.0 and m.floor is not None and m.floor.normal[1] >= 0.17364818:
        return set_mario_action(m, C.ACT_BRAKING, 0)
    return set_mario_action(m, C.ACT_DECELERATING, 0)


# -- speed updates ----------------------------------------------------------


def apply_slope_accel(m):
    steepness = m.get_slope_steepness()
    floor_dyaw = s16(m.floor_angle - m.face_angle[1])

    if m.floor_is_slope():
        accel_by_class = {
            C.SURFACE_CLASS_VERY_SLIPPERY: 5.3,
            C.SURFACE_CLASS_SLIPPERY: 2.7,
            C.SURFACE_CLASS_NOT_SLIPPERY: 0.0,
        }
        slope_accel = accel_by_class.get(m.get_floor_class(), 1.7)

        # Downhill speeds up, uphill slows down.
        if -0x4000 < floor_dyaw < 0x4000:
            m.forward_vel += slope_accel * steepness
        else:
            m.forward_vel -= slope_accel * steepness

    m.slide_yaw = m.face_angle[1]
    m.slide_vel_x = m.forward_vel * sins(m.face_angle[1])
    m.slide_vel_z = m.forward_vel * coss(m.face_angle[1])
    m.vel[0] = m.slide_vel_x
    m.vel[1] = 0.0
    m.vel[2] = m.slide_vel_z


def update_walking_speed(m):
    max_target_speed = 24.0 if (m.floor is not None and m.floor.type == C.SURFACE_SLOW) else 32.0
    target_speed = min(m.intended_mag, max_target_speed)

    if m.forward_vel <= 0.0:
        m.forward_vel += 1.1
    elif m.forward_vel <= target_speed:
        # Acceleration tapers off as speed rises.
        m.forward_vel += 1.1 - m.forward_vel / 43.0
    elif m.floor is not None and m.floor.normal[1] >= 0.95:
        m.forward_vel -= 1.0

    m.forward_vel = min(m.forward_vel, 48.0)

    # Turning is rate-limited, which is why Mario arcs instead of pivoting.
    m.face_angle[1] = s16(
        m.intended_yaw
        - approach_s32(s16(m.intended_yaw - m.face_angle[1]), 0, 0x800, 0x800)
    )
    apply_slope_accel(m)


def update_decelerating_speed(m):
    stopped = False
    m.forward_vel = approach_f32(m.forward_vel, 0.0, 1.0, 1.0)
    if m.forward_vel == 0.0:
        stopped = True
    m.set_forward_vel(m.forward_vel)
    return stopped


def update_air_without_turn(m):
    """Air control: the stick nudges speed forward and sideways, not facing."""
    sideways_speed = 0.0
    drag_threshold = 48.0 if m.action == C.ACT_LONG_JUMP else 32.0
    m.forward_vel = approach_f32(m.forward_vel, 0.0, 0.35, 0.35)

    if m.input & C.INPUT_NONZERO_ANALOG:
        intended_dyaw = s16(m.intended_yaw - m.face_angle[1])
        intended_mag = m.intended_mag / 32.0
        m.forward_vel += intended_mag * coss(intended_dyaw) * 1.5
        sideways_speed = intended_mag * sins(intended_dyaw) * 10.0

    # Air speed is only bled off above the threshold, never hard-capped.
    if m.forward_vel > drag_threshold:
        m.forward_vel -= 1.0
    if m.forward_vel < -16.0:
        m.forward_vel += 2.0

    m.slide_vel_x = m.forward_vel * sins(m.face_angle[1])
    m.slide_vel_z = m.forward_vel * coss(m.face_angle[1])
    m.slide_vel_x += sideways_speed * sins(s16(m.face_angle[1] + 0x4000))
    m.slide_vel_z += sideways_speed * coss(s16(m.face_angle[1] + 0x4000))

    m.vel[0] = m.slide_vel_x
    m.vel[2] = m.slide_vel_z


def update_air_with_turn(m):
    """Air control that also steers Mario's facing (dive, freefall)."""
    drag_threshold = 48.0 if m.action == C.ACT_LONG_JUMP else 32.0
    m.forward_vel = approach_f32(m.forward_vel, 0.0, 0.35, 0.35)

    if m.input & C.INPUT_NONZERO_ANALOG:
        intended_dyaw = s16(m.intended_yaw - m.face_angle[1])
        intended_mag = m.intended_mag / 32.0
        m.forward_vel += 1.5 * coss(intended_dyaw) * intended_mag
        m.face_angle[1] = s16(m.face_angle[1] + 512.0 * sins(intended_dyaw) * intended_mag)

    if m.forward_vel > drag_threshold:
        m.forward_vel -= 1.0
    if m.forward_vel < -16.0:
        m.forward_vel += 2.0

    m.vel[0] = m.slide_vel_x = m.forward_vel * sins(m.face_angle[1])
    m.vel[2] = m.slide_vel_z = m.forward_vel * coss(m.face_angle[1])


def update_sliding_angle(m, accel, loss_factor):
    slope_angle = m.floor_angle
    steepness = m.get_slope_steepness()

    m.slide_vel_x += accel * steepness * sins(slope_angle)
    m.slide_vel_z += accel * steepness * coss(slope_angle)
    m.slide_vel_x *= loss_factor
    m.slide_vel_z *= loss_factor

    m.slide_yaw = atan2s(m.slide_vel_z, m.slide_vel_x)

    # Rotate Mario's facing toward his slide direction a little each frame.
    facing_dyaw = s16(m.face_angle[1] - m.slide_yaw)
    new_dyaw = facing_dyaw
    if 0 < new_dyaw <= 0x4000:
        new_dyaw = max(new_dyaw - 0x200, 0)
    elif -0x4000 < new_dyaw < 0:
        new_dyaw = min(new_dyaw + 0x200, 0)

    m.face_angle[1] = s16(m.slide_yaw + new_dyaw)

    m.vel[0] = m.slide_vel_x
    m.vel[1] = 0.0
    m.vel[2] = m.slide_vel_z
    m.forward_vel = math.hypot(m.slide_vel_x, m.slide_vel_z)


def update_sliding(m, stop_speed):
    intended_dyaw = s16(m.intended_yaw - m.slide_yaw)
    forward = coss(intended_dyaw)
    sideward = sins(intended_dyaw)

    # Pulling back is weaker the faster Mario is already going.
    if forward < 0.0 and m.forward_vel >= 0.0:
        forward *= 0.5 + 0.5 * m.forward_vel / 100.0

    params = {
        C.SURFACE_CLASS_VERY_SLIPPERY: (10.0, 0.98),
        C.SURFACE_CLASS_SLIPPERY: (8.0, 0.96),
        C.SURFACE_CLASS_NOT_SLIPPERY: (5.0, 0.92),
    }
    accel, base_loss = params.get(m.get_floor_class(), (7.0, 0.92))
    loss_factor = m.intended_mag / 32.0 * forward * 0.02 + base_loss

    old_speed = math.hypot(m.slide_vel_x, m.slide_vel_z)

    # Steering rotates the velocity vector, then renormalises back to the
    # old magnitude so turning alone neither gains nor loses speed.
    m.slide_vel_x += m.slide_vel_z * (m.intended_mag / 32.0) * sideward * 0.05
    m.slide_vel_z -= m.slide_vel_x * (m.intended_mag / 32.0) * sideward * 0.05

    new_speed = math.hypot(m.slide_vel_x, m.slide_vel_z)
    if old_speed > 0.0 and new_speed > 0.0:
        m.slide_vel_x *= old_speed / new_speed
        m.slide_vel_z *= old_speed / new_speed

    update_sliding_angle(m, accel, loss_factor)

    if not m.floor_is_slope() and m.forward_vel ** 2 < stop_speed ** 2:
        m.set_forward_vel(0.0)
        return True
    return False


# -- stationary actions -----------------------------------------------------


@action(C.ACT_IDLE, "idle")
def act_idle(m):
    if m.input & C.INPUT_IN_POISON_GAS:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if check_common_action_exits(m):
        return True
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_PUNCHING, 0)
    if m.input & C.INPUT_Z_DOWN:
        return set_mario_action(m, C.ACT_START_CROUCHING, 0)

    stationary_ground_step(m)
    return False


@action(C.ACT_START_CROUCHING, "start_crouching")
def act_start_crouching(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)

    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= 6:
        set_mario_action(m, C.ACT_CROUCHING, 0)
    return False


@action(C.ACT_CROUCHING, "crouching")
def act_crouching(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_BACKFLIP, 0)
    if not (m.input & C.INPUT_Z_DOWN):
        return set_mario_action(m, C.ACT_STOP_CROUCHING, 0)
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_PUNCHING, 9)
    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_mario_action(m, C.ACT_CRAWLING, 0)

    stationary_ground_step(m)
    return False


@action(C.ACT_STOP_CROUCHING, "stop_crouching")
def act_stop_crouching(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_JUMP, 0)

    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= 6:
        set_mario_action(m, C.ACT_IDLE, 0)
    return False


@action(C.ACT_BRAKING_STOP, "braking_stop")
def act_braking_stop(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)
    if not (m.input & C.INPUT_UNKNOWN_5) or (m.input & C.INPUT_NONZERO_ANALOG):
        return set_mario_action(m, C.ACT_IDLE, 0)
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_PUNCHING, 0)

    stationary_ground_step(m)
    return False


@action(C.ACT_PUNCHING, "punching")
def act_punching(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_JUMP, 0)

    m.set_forward_vel(0.0)
    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= 8:
        set_mario_action(m, C.ACT_IDLE, 0)
    return False


@action(C.ACT_GROUND_POUND_LAND, "ground_pound_land")
def act_ground_pound_land(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)

    m.set_forward_vel(0.0)
    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= 16:
        if m.input & C.INPUT_A_PRESSED:
            return set_jumping_action(m, C.ACT_JUMP, 0)
        set_mario_action(m, C.ACT_IDLE, 0)
    return False


@action(C.ACT_BUTT_SLIDE_STOP, "butt_slide_stop")
def act_butt_slide_stop(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)

    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= 10:
        set_mario_action(m, C.ACT_IDLE, 0)
    return False


# -- moving actions ---------------------------------------------------------


@action(C.ACT_WALKING, "walking")
def act_walking(m):
    if should_begin_sliding(m):
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)
    if check_ground_dive_or_punch(m):
        return True
    if m.input & C.INPUT_UNKNOWN_5:
        return begin_braking_action(m)
    # A hard reversal at speed pivots instead of arcing around.
    if analog_stick_held_back(m) and m.forward_vel >= 16.0:
        return set_mario_action(m, C.ACT_TURNING_AROUND, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_BUTT_SLIDE, 0)

    m.action_state = 0
    update_walking_speed(m)
    step_sounds(m)

    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.GROUND_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
        m.action_timer = 0
    return False


@action(C.ACT_TURNING_AROUND, "turning_around")
def act_turning_around(m):
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_SIDE_FLIP, 0)
    if not analog_stick_held_back(m):
        return set_mario_action(m, C.ACT_WALKING, 0)
    if check_ground_dive_or_punch(m):
        return True

    m.forward_vel = approach_f32(m.forward_vel, 0.0, 1.0, 2.0)
    if m.forward_vel <= 0.0:
        return set_mario_action(m, C.ACT_FINISH_TURNING_AROUND, 0)

    apply_slope_accel(m)
    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.GROUND_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
    return False


@action(C.ACT_FINISH_TURNING_AROUND, "finish_turning_around")
def act_finish_turning_around(m):
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_SIDE_FLIP, 0)

    # Snap around to the direction the stick is asking for.
    m.face_angle[1] = s16(m.face_angle[1] + 0x8000)
    m.set_forward_vel(8.0)
    perform_ground_step(m)
    set_mario_action(m, C.ACT_WALKING, 0)
    return False


@action(C.ACT_BRAKING, "braking")
def act_braking(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_ABOVE_SLIDE:
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_MOVE_PUNCHING, 0)

    m.forward_vel = approach_f32(m.forward_vel, 0.0, 2.0, 3.0)
    if m.forward_vel <= 0.0:
        return set_mario_action(m, C.ACT_BRAKING_STOP, 0)

    apply_slope_accel(m)
    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.GROUND_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
    return False


@action(C.ACT_DECELERATING, "decelerating")
def act_decelerating(m):
    if should_begin_sliding(m):
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)
    if check_ground_dive_or_punch(m):
        return True
    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_mario_action(m, C.ACT_WALKING, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_BUTT_SLIDE, 0)

    if update_decelerating_speed(m):
        return set_mario_action(m, C.ACT_IDLE, 0)

    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.GROUND_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
    return False


@action(C.ACT_CRAWLING, "crawling")
def act_crawling(m):
    if should_begin_sliding(m):
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_JUMP, 0)
    if check_ground_dive_or_punch(m):
        return True
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if not (m.input & C.INPUT_Z_DOWN):
        return set_mario_action(m, C.ACT_STOP_CROUCHING, 0)
    if not (m.input & C.INPUT_NONZERO_ANALOG):
        return set_mario_action(m, C.ACT_CROUCHING, 0)

    m.intended_mag *= 0.1
    update_walking_speed(m)

    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.GROUND_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
    return False


def bounce_off_wall(m):
    """Reflect Mario's facing in a wall and send him back at half speed."""
    if m.wall is not None:
        wall_angle = m.wall.yaw
        m.face_angle[1] = s16(wall_angle - s16(m.face_angle[1] - wall_angle))
    m.set_forward_vel(-m.forward_vel * 0.5)


def _common_slide_action(m, stop_action, air_action):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, air_action, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_JUMP, 0)

    stopped = update_sliding(m, 4.0)
    result = perform_ground_step(m)

    if result == C.GROUND_STEP_LEFT_GROUND:
        return set_mario_action(m, air_action, 0)
    if result == C.GROUND_STEP_HIT_WALL:
        bounce_off_wall(m)

    if stopped and not (m.input & C.INPUT_NONZERO_ANALOG):
        return set_mario_action(m, stop_action, 0)
    return False


@action(C.ACT_BUTT_SLIDE, "butt_slide")
def act_butt_slide(m):
    return _common_slide_action(m, C.ACT_BUTT_SLIDE_STOP, C.ACT_FREEFALL)


@action(C.ACT_STOMACH_SLIDE, "stomach_slide")
def act_stomach_slide(m):
    return _common_slide_action(m, C.ACT_IDLE, C.ACT_FREEFALL)


@action(C.ACT_DIVE_SLIDE, "dive_slide")
def act_dive_slide(m):
    return _common_slide_action(m, C.ACT_IDLE, C.ACT_FREEFALL)


# -- skating ----------------------------------------------------------------
#
# Not a decomp action, but it is built out of decomp parts. `update_sliding`
# *is* SM64's ice: momentum that keeps going, a friction that barely bites,
# and steering that rotates the velocity vector rather than the body, so
# turning costs distance instead of speed. Overriding the floor class to
# very-slippery (see MarioState.get_floor_class) is what hands it the numbers
# ice would have had -- 10.0 acceleration, 0.98 retained per frame.
#
# What SM64 has no equivalent of is the push. Nothing in the game makes a
# sliding Mario go faster on the flat, because a slide is something that
# happens to him: he is always either falling down a hill or bleeding off
# speed he already had. A skater is the one supplying the speed, so that is
# the piece that had to be written rather than reused.

# Speed added per tick at full stick, and the speed that eventually buys.
# Deliberately above his running top speed of 32: skates that are slower than
# his own legs would be a downgrade.
SKATE_PUSH = 1.15
SKATE_TOP_SPEED = 44.0

# Below this, with nothing on the stick, he has come to rest. Applied only
# while coasting -- a push starts below it by definition, and checking it
# regardless is a Mario who can never get going.
SKATE_STOP_SPEED = 2.0


@action(C.ACT_SKATING, "skating")
def act_skating(m):
    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jumping_action(m, C.ACT_JUMP, 0)
    if not m.controller.skating:
        # Skates off. Hand back whatever speed he had rather than dropping it,
        # so stepping off the ice at pace carries into a run.
        return set_mario_action(
            m, C.ACT_WALKING if m.forward_vel > 0.0 else C.ACT_IDLE, 0)

    # Pushing or coasting. The two skating clips differ by nothing else, and
    # neither does the physics.
    m.action_state = 1 if m.intended_mag > 0.0 else 0

    if m.action_state and m.forward_vel < SKATE_TOP_SPEED:
        push = SKATE_PUSH * (m.intended_mag / 32.0)
        m.slide_vel_x += push * sins(m.intended_yaw)
        m.slide_vel_z += push * coss(m.intended_yaw)

    update_sliding(m, 0.0 if m.action_state else SKATE_STOP_SPEED)
    result = perform_ground_step(m)

    if result == C.GROUND_STEP_LEFT_GROUND:
        return set_mario_action(m, C.ACT_FREEFALL, 0)
    if result == C.GROUND_STEP_HIT_WALL:
        # Off the boards and back down the rink, the way a slide bounces --
        # stopping dead on a wall is the one thing that would not read as ice.
        bounce_off_wall(m)
    return False


def check_skating(m):
    """Take over whatever ground action he is in, once the skates are on.

    Not a decomp transition. The skate key is a mode rather than a button, so
    there is no action that presses its way in the way Z presses into a butt
    slide. Airborne actions are left alone so a jump finishes as a jump, and
    attacks and slides are left to resolve on their own -- a dive that turned
    into a glide halfway through would just eat the input.
    """
    if not m.controller.skating or m.action == C.ACT_SKATING:
        return
    if (m.action & C.ACT_GROUP_MASK) not in (C.ACT_GROUP_STATIONARY,
                                             C.ACT_GROUP_MOVING):
        return
    if m.action & (C.ACT_FLAG_BUTT_OR_STOMACH_SLIDE | C.ACT_FLAG_ATTACKING):
        return
    set_mario_action(m, C.ACT_SKATING, 0)


@action(C.ACT_MOVE_PUNCHING, "move_punching")
def act_move_punching(m):
    if should_begin_sliding(m):
        return set_mario_action(m, C.ACT_BEGIN_SLIDING, 0)
    if m.input & C.INPUT_A_PRESSED:
        return set_jump_from_landing(m)

    m.forward_vel = approach_f32(m.forward_vel, 0.0, 1.0, 1.0)
    apply_slope_accel(m)

    result = perform_ground_step(m)
    if result == C.GROUND_STEP_LEFT_GROUND:
        set_mario_action(m, C.ACT_FREEFALL, 0)

    m.action_timer += 1
    if m.action_timer >= 8:
        set_mario_action(m, C.ACT_WALKING, 0)
    return False


# Landing is table-driven: (frames, double_jump_timer, end, a_pressed, slide).
#
# `double_jump_timer` is the window, in frames, during which the *next* jump
# chains upward.  Triple jump land sets it to 0, which is what ends the chain
# and sends a fourth jump back to a single.
def set_triple_jump_action(m, act=0, action_arg=0):
    """A-press handler for a double-jump landing: triple only if fast enough."""
    if m.forward_vel > 20.0:
        return set_mario_action(m, C.ACT_TRIPLE_JUMP, 0)
    return set_mario_action(m, C.ACT_JUMP, 0)


class _LandingAction:
    __slots__ = ("num_frames", "double_jump_timer", "end_action",
                 "a_pressed_action", "off_floor_action", "slide_action",
                 "a_press_handler")

    def __init__(self, num_frames, double_jump_timer, end_action,
                 a_pressed_action, off_floor_action, slide_action,
                 a_press_handler=None):
        self.num_frames = num_frames
        self.double_jump_timer = double_jump_timer
        self.end_action = end_action
        self.a_pressed_action = a_pressed_action
        self.off_floor_action = off_floor_action
        self.slide_action = slide_action
        self.a_press_handler = a_press_handler or set_jumping_action


_JUMP_LAND = _LandingAction(4, 5, C.ACT_JUMP_LAND_STOP, C.ACT_DOUBLE_JUMP,
                            C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING)
_FREEFALL_LAND = _LandingAction(4, 5, C.ACT_FREEFALL_LAND_STOP, C.ACT_DOUBLE_JUMP,
                                C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING)
_SIDE_FLIP_LAND = _LandingAction(4, 5, C.ACT_SIDE_FLIP_LAND_STOP, C.ACT_DOUBLE_JUMP,
                                 C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING)
_DOUBLE_JUMP_LAND = _LandingAction(4, 5, C.ACT_DOUBLE_JUMP_LAND_STOP, C.ACT_JUMP,
                                   C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING,
                                   a_press_handler=set_triple_jump_action)
_TRIPLE_JUMP_LAND = _LandingAction(4, 0, C.ACT_TRIPLE_JUMP_LAND_STOP,
                                   C.ACT_UNINITIALIZED, C.ACT_FREEFALL,
                                   C.ACT_BEGIN_SLIDING)
_BACKFLIP_LAND = _LandingAction(4, 0, C.ACT_BACKFLIP_LAND_STOP, C.ACT_BACKFLIP,
                                C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING)
_LONG_JUMP_LAND = _LandingAction(6, 5, C.ACT_LONG_JUMP_LAND_STOP, C.ACT_LONG_JUMP,
                                 C.ACT_FREEFALL, C.ACT_BEGIN_SLIDING)


def _common_landing_cancels(m, landing):
    # Steepness is tested before Mario is confirmed to be on the ground.
    if m.floor is not None and m.floor.normal[1] < 0.2923717:
        m.set_forward_vel(0.0)
        return set_mario_action(m, C.ACT_FREEFALL, 0)

    m.double_jump_timer = landing.double_jump_timer

    if should_begin_sliding(m):
        return set_mario_action(m, landing.slide_action, 0)

    m.action_timer += 1
    if m.action_timer >= landing.num_frames:
        return set_mario_action(m, landing.end_action, 0)

    if m.input & C.INPUT_A_PRESSED:
        return landing.a_press_handler(m, landing.a_pressed_action, 0)

    if m.input & C.INPUT_OFF_FLOOR:
        return set_mario_action(m, landing.off_floor_action, 0)

    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_mario_action(m, C.ACT_WALKING, 0)

    return False


def _common_landing_action(m, landing, air_action):
    if _common_landing_cancels(m, landing):
        return True

    update_decelerating_speed(m)
    if perform_ground_step(m) == C.GROUND_STEP_LEFT_GROUND:
        return set_mario_action(m, air_action, 0)
    return False


@action(C.ACT_JUMP_LAND, "jump_land")
def act_jump_land(m):
    return _common_landing_action(m, _JUMP_LAND, C.ACT_FREEFALL)


@action(C.ACT_FREEFALL_LAND, "freefall_land")
def act_freefall_land(m):
    return _common_landing_action(m, _FREEFALL_LAND, C.ACT_FREEFALL)


@action(C.ACT_DOUBLE_JUMP_LAND, "double_jump_land")
def act_double_jump_land(m):
    return _common_landing_action(m, _DOUBLE_JUMP_LAND, C.ACT_FREEFALL)


@action(C.ACT_SIDE_FLIP_LAND, "side_flip_land")
def act_side_flip_land(m):
    return _common_landing_action(m, _SIDE_FLIP_LAND, C.ACT_FREEFALL)


@action(C.ACT_TRIPLE_JUMP_LAND, "triple_jump_land")
def act_triple_jump_land(m):
    return _common_landing_action(m, _TRIPLE_JUMP_LAND, C.ACT_FREEFALL)


@action(C.ACT_BACKFLIP_LAND, "backflip_land")
def act_backflip_land(m):
    return _common_landing_action(m, _BACKFLIP_LAND, C.ACT_FREEFALL)


@action(C.ACT_LONG_JUMP_LAND, "long_jump_land")
def act_long_jump_land(m):
    return _common_landing_action(m, _LONG_JUMP_LAND, C.ACT_FREEFALL)


# The *_LAND_STOP actions are the settled pose after a landing.  Pressing A
# from here routes through set_jump_from_landing, which reads prev_action --
# that is the path a triple jump actually takes.


def _check_common_landing_cancels(m, act=0):
    if m.input & C.INPUT_A_PRESSED:
        if not act:
            return set_jump_from_landing(m)
        return set_jumping_action(m, act, 0)

    if m.input & (C.INPUT_NONZERO_ANALOG | C.INPUT_OFF_FLOOR | C.INPUT_ABOVE_SLIDE):
        return check_common_action_exits(m)

    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_PUNCHING, 0)

    return False


def _landing_stop_action(m, num_frames=8):
    if _check_common_landing_cancels(m, 0):
        return True

    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= num_frames:
        return set_mario_action(m, C.ACT_IDLE, 0)
    return False


@action(C.ACT_JUMP_LAND_STOP, "jump_land_stop")
def act_jump_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_FREEFALL_LAND_STOP, "freefall_land_stop")
def act_freefall_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_DOUBLE_JUMP_LAND_STOP, "double_jump_land_stop")
def act_double_jump_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_SIDE_FLIP_LAND_STOP, "side_flip_land_stop")
def act_side_flip_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_TRIPLE_JUMP_LAND_STOP, "triple_jump_land_stop")
def act_triple_jump_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_BACKFLIP_LAND_STOP, "backflip_land_stop")
def act_backflip_land_stop(m):
    return _landing_stop_action(m)


@action(C.ACT_LONG_JUMP_LAND_STOP, "long_jump_land_stop")
def act_long_jump_land_stop(m):
    return _landing_stop_action(m)


# -- airborne actions -------------------------------------------------------


def _common_air_action_step(m, land_action, step_arg=0):
    update_air_without_turn(m)
    result = perform_air_step(m, step_arg)

    if result == C.AIR_STEP_LANDED:
        set_mario_action(m, land_action, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        if m.forward_vel > 16.0:
            # Bounce back off the wall.
            if m.wall is not None:
                wall_angle = m.wall.yaw
                m.face_angle[1] = s16(wall_angle - s16(m.face_angle[1] - wall_angle))
            m.face_angle[1] = s16(m.face_angle[1] + 0x8000)
            if m.wall is not None:
                set_mario_action(m, C.ACT_AIR_HIT_WALL, 0)
            else:
                m.set_forward_vel(0.0)
    elif result == C.AIR_STEP_GRABBED_LEDGE:
        set_mario_action(m, C.ACT_LEDGE_GRAB, 0)

    return result


@action(C.ACT_JUMP, "jump")
def act_jump(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_JUMP_LAND, C.AIR_STEP_CHECK_LEDGE_GRAB)
    return False


@action(C.ACT_DOUBLE_JUMP, "double_jump")
def act_double_jump(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_DOUBLE_JUMP_LAND, C.AIR_STEP_CHECK_LEDGE_GRAB)
    return False


@action(C.ACT_TRIPLE_JUMP, "triple_jump")
def act_triple_jump(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_TRIPLE_JUMP_LAND, 0)
    return False


@action(C.ACT_BACKFLIP, "backflip")
def act_backflip(m):
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_BACKFLIP_LAND, 0)
    return False


@action(C.ACT_SIDE_FLIP, "side_flip")
def act_side_flip(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_SIDE_FLIP_LAND, 0)
    return False


@action(C.ACT_FREEFALL, "freefall")
def act_freefall(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_FREEFALL_LAND, C.AIR_STEP_CHECK_LEDGE_GRAB)
    return False


@action(C.ACT_STEEP_JUMP, "steep_jump")
def act_steep_jump(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)

    update_air_without_turn(m)
    result = perform_air_step(m, 0)
    if result == C.AIR_STEP_LANDED:
        if not m.floor_is_steep():
            m.face_angle[0] = 0
            set_mario_action(m, C.ACT_JUMP_LAND, 0)
        else:
            set_mario_action(m, C.ACT_FREEFALL, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        m.set_forward_vel(0.0)
    return False


@action(C.ACT_WALL_KICK_AIR, "wall_kick_air")
def act_wall_kick_air(m):
    if m.input & C.INPUT_B_PRESSED:
        return set_mario_action(m, C.ACT_DIVE, 0)
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_JUMP_LAND, C.AIR_STEP_CHECK_LEDGE_GRAB)
    return False


@action(C.ACT_AIR_HIT_WALL, "air_hit_wall")
def act_air_hit_wall(m):
    m.action_timer += 1
    # A short window after bonking in which A performs a wall kick.
    if m.action_timer <= 2:
        if m.input & C.INPUT_A_PRESSED:
            m.vel[1] = 52.0
            m.face_angle[1] = s16(m.face_angle[1] + 0x8000)
            return set_mario_action(m, C.ACT_WALL_KICK_AIR, 0)
    elif m.forward_vel >= 38.0:
        m.wall_kick_timer = 5
        if m.vel[1] > 0.0:
            m.vel[1] = 0.0
        m.set_forward_vel(-16.0)
        return set_mario_action(m, C.ACT_BACKWARD_AIR_KB, 0)
    else:
        m.wall_kick_timer = 5
        if m.vel[1] > 0.0:
            m.vel[1] = 0.0
        m.set_forward_vel(min(m.forward_vel, -16.0))
        return set_mario_action(m, C.ACT_SOFT_BONK, 0)

    return set_mario_action(m, C.ACT_FREEFALL, 0)


@action(C.ACT_SOFT_BONK, "soft_bonk")
def act_soft_bonk(m):
    _common_air_action_step(m, C.ACT_FREEFALL_LAND, 0)
    return False


@action(C.ACT_BACKWARD_AIR_KB, "backward_air_kb")
def act_backward_air_kb(m):
    _common_air_action_step(m, C.ACT_FREEFALL_LAND, 0)
    return False


@action(C.ACT_LONG_JUMP, "long_jump")
def act_long_jump(m):
    if m.input & C.INPUT_Z_PRESSED:
        return set_mario_action(m, C.ACT_GROUND_POUND, 0)

    _common_air_action_step(m, C.ACT_LONG_JUMP_LAND, C.AIR_STEP_CHECK_LEDGE_GRAB)
    return False


@action(C.ACT_DIVE, "dive")
def act_dive(m):
    if m.action_timer == 0 and m.action_arg == 0:
        m.forward_vel = min(m.forward_vel + 15.0, 48.0)

    update_air_without_turn(m)
    result = perform_air_step(m, 0)

    if result == C.AIR_STEP_NONE:
        # Pitch nose-down while falling.
        if m.vel[1] < 0.0 and m.face_angle[0] > -0x2AAA:
            m.face_angle[0] = s16(m.face_angle[0] - 0x200)
            if m.face_angle[0] < -0x2AAA:
                m.face_angle[0] = -0x2AAA
    elif result == C.AIR_STEP_LANDED:
        if m.floor_is_slope() or m.forward_vel > 16.0:
            m.face_angle[0] = 0
            set_mario_action(m, C.ACT_DIVE_SLIDE, 0)
        else:
            m.face_angle[0] = 0
            set_mario_action(m, C.ACT_FREEFALL_LAND, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        if m.wall is not None:
            wall_angle = m.wall.yaw
            m.face_angle[1] = s16(wall_angle - s16(m.face_angle[1] - wall_angle))
        m.set_forward_vel(-16.0)
        m.face_angle[0] = 0
        set_mario_action(m, C.ACT_BACKWARD_AIR_KB, 0)

    return False


@action(C.ACT_GROUND_POUND, "ground_pound")
def act_ground_pound(m):
    if m.action_state == 0:
        # Hang in the air and spin before dropping.
        if m.action_timer == 0:
            m.vel[1] = -1.0
            m.pos[1] += 20.0

        m.action_timer += 1
        m.vel[1] = -1.0
        m.pos[1] += 20.0

        if m.action_timer >= 8:
            m.action_state = 1
            m.vel[1] = -50.0
            m.set_forward_vel(0.0)
        m.sync_graphics()
        return False

    result = perform_air_step(m, 0)
    if result == C.AIR_STEP_LANDED:
        m.particle_flags |= 1
        set_mario_action(m, C.ACT_GROUND_POUND_LAND, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        m.set_forward_vel(-16.0)
        if m.vel[1] > 0.0:
            m.vel[1] = 0.0
        set_mario_action(m, C.ACT_BACKWARD_AIR_KB, 0)

    return False


@action(C.ACT_SLIDE_KICK, "slide_kick")
def act_slide_kick(m):
    update_air_without_turn(m)
    result = perform_air_step(m, 0)
    if result == C.AIR_STEP_LANDED:
        m.face_angle[0] = 0
        set_mario_action(m, C.ACT_DIVE_SLIDE, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        m.set_forward_vel(-16.0)
        set_mario_action(m, C.ACT_BACKWARD_AIR_KB, 0)
    return False


@action(C.ACT_LEDGE_GRAB, "ledge_grab")
def act_ledge_grab(m):
    if m.input & (C.INPUT_Z_PRESSED | C.INPUT_OFF_FLOOR):
        # Let go and drop.
        m.set_forward_vel(0.0)
        m.vel[1] = 0.0
        m.pos[1] -= 20.0
        return set_mario_action(m, C.ACT_FREEFALL, 0)

    if m.input & C.INPUT_A_PRESSED or (m.input & C.INPUT_NONZERO_ANALOG
                                       and not analog_stick_held_back(m)):
        # Climb up onto the ledge.
        m.pos[0] += 14.0 * sins(m.face_angle[1])
        m.pos[2] += 14.0 * coss(m.face_angle[1])
        m.pos[1] = m.floor_height
        m.set_forward_vel(0.0)
        m.vel[1] = 0.0
        return set_mario_action(m, C.ACT_IDLE, 0)

    m.set_forward_vel(0.0)
    m.vel[1] = 0.0
    m.sync_graphics()
    return False


# -- driver -----------------------------------------------------------------


def execute_action(m):
    """Run Mario for one frame. Returns the action that ended up running."""
    m.update_inputs()

    # Actions raise sounds by appending to this; whatever ran last frame has
    # been consumed by now.
    m.sound_events.clear()

    # Falling below the surface interrupts whatever he was doing. Checked here
    # rather than inside every land action, since it applies to all of them.
    from .water import check_common_water_cancels
    check_common_water_cancels(m)

    # Same reasoning, one layer out: the skates are a mode, so nothing presses
    # its way into them and the check has to happen somewhere every frame.
    check_skating(m)

    # An action returning True changed action and wants the new one to run
    # immediately, so transitions resolve within a single frame.
    for _ in range(8):
        handler = ACTIONS.get(m.action)
        if handler is None:
            set_mario_action(m, C.ACT_IDLE, 0)
            handler = ACTIONS[C.ACT_IDLE]
        fn, anim = handler
        m.anim_name = anim
        if not fn(m):
            break

    # action_timer is owned by the individual actions, not incremented here.
    m.sync_graphics()
    return m.action
