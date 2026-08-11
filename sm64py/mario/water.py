"""Swimming: the submerged action group.

Water is not collision.  It is a set of axis-aligned boxes queried by (x, z)
for a surface height, so "underwater" is a comparison against that height
rather than anything the surface engine reports.  Movement below it runs on
its own step function -- there is no quarter-stepping, no gravity, and the
walls are tested at a different height than on land.

Mario swims along the direction he faces, so pitch matters here in a way it
never does on the ground: face_angle[0] is driven by the stick and steers
him up and down, and face_angle[2] rolls him into turns.  Both reach the
renderer through gfx_angle.
"""

from ..math_util import approach_f32, approach_s32, coss, s16, sins
from . import constants as C
from .actions import ACTIONS, action, set_mario_action
from .steps import resolve_and_return_wall_collisions

MIN_SWIM_STRENGTH = C.MIN_SWIM_STRENGTH
MAX_SWIM_STRENGTH = C.MAX_SWIM_STRENGTH
SWIM_STRENGTH_GAIN = C.SWIM_STRENGTH_GAIN

MAX_SWIM_SPEED = 28.0

# Mario floats this far under the surface rather than breaking through it.
SURFACE_OFFSET = 80.0
# Falling past this depth below the surface starts a plunge.
PLUNGE_DEPTH = 100.0
# Vertical room the water step insists on between floor and ceiling.
WATER_HEADROOM = 160.0

# Step outcomes, mirroring the ground and air step results.
WATER_STEP_NONE = 0
WATER_STEP_HIT_FLOOR = 1
WATER_STEP_HIT_CEILING = 2
WATER_STEP_HIT_WALL = 3
WATER_STEP_CANCELLED = 4


# -- helpers ----------------------------------------------------------------


def swimming_near_surface(m):
    scale = m.motion_scale
    return (m.water_level - SURFACE_OFFSET * scale) - m.pos[1] < 400.0 * scale


def get_buoyancy(m):
    """Vertical drift added to Mario's swimming velocity.

    Near the surface it is positive, which is what floats him back up to the
    waterline when he stops swimming; deeper down a stationary action sinks
    instead.
    """
    if swimming_near_surface(m):
        return 1.25
    if not (m.action & C.ACT_FLAG_MOVING):
        return -2.0
    return 0.0


def update_swimming_yaw(m):
    target = -int(10.0 * m.controller.stick_x)
    vel = m.angle_vel[1]

    # Reversing gets a hard kick out of the old direction before easing toward
    # the new one, so turns bite immediately instead of coasting through zero.
    if target > 0:
        if vel < 0:
            vel = min(vel + 0x40, 0x10)
        else:
            vel = approach_s32(vel, target, 0x10, 0x20)
    elif target < 0:
        if vel > 0:
            vel = max(vel - 0x40, -0x10)
        else:
            vel = approach_s32(vel, target, 0x20, 0x10)
    else:
        vel = approach_s32(vel, 0, 0x40, 0x40)

    m.angle_vel[1] = vel
    m.face_angle[1] = s16(m.face_angle[1] + vel)
    # Banking: the roll is just the turn rate scaled up.
    m.face_angle[2] = s16(-vel * 8)


def update_swimming_pitch(m):
    # Pushing the stick forward dives. The sign looks inverted against the
    # original because this port feeds the stick mirrored -- see Controller --
    # and unlike the yaw, there is no camera rotation here to cancel it back.
    target = int(252.0 * m.controller.stick_y)
    # Pitching down is slower than pitching up.
    rate = 0x100 if m.face_angle[0] < 0 else 0x200

    if m.face_angle[0] < target:
        m.face_angle[0] = min(m.face_angle[0] + rate, target)
    elif m.face_angle[0] > target:
        m.face_angle[0] = max(m.face_angle[0] - rate, target)


def update_swimming_speed(m, decel_threshold):
    buoyancy = get_buoyancy(m)

    if m.action & C.ACT_FLAG_STATIONARY:
        m.forward_vel -= 2.0

    m.forward_vel = min(max(m.forward_vel, 0.0), MAX_SWIM_SPEED)

    # Above the threshold the stroke is spent and speed bleeds off; the
    # threshold rises with swim strength, so chained strokes coast further.
    if m.forward_vel > decel_threshold:
        m.forward_vel -= 0.5

    m.vel[0] = m.forward_vel * coss(m.face_angle[0]) * sins(m.face_angle[1])
    m.vel[1] = m.forward_vel * sins(m.face_angle[0]) + buoyancy
    m.vel[2] = m.forward_vel * coss(m.face_angle[0]) * coss(m.face_angle[1])


def perform_water_step(m):
    """Move Mario by his velocity, clamped to stay under the surface."""
    next_pos = [m.pos[i] + m.vel[i] * m.motion_scale for i in range(3)]

    ceiling = m.water_level - SURFACE_OFFSET * m.motion_scale
    if next_pos[1] > ceiling:
        next_pos[1] = ceiling
        m.vel[1] = 0.0

    return _perform_water_full_step(m, next_pos)


def _perform_water_full_step(m, next_pos):
    # Walls are tested from higher up his body than on the ground.
    wall = resolve_and_return_wall_collisions(
        m, next_pos, 10.0 * m.motion_scale, 110.0 * m.motion_scale)
    floor_height, floor = m.surfaces.find_floor(*next_pos)
    ceil_height, _ = m.find_ceil(next_pos, floor_height)

    if floor is None:
        return WATER_STEP_CANCELLED

    if next_pos[1] >= floor_height:
        if ceil_height - next_pos[1] >= WATER_HEADROOM * m.motion_scale:
            m.pos = list(next_pos)
            m.floor, m.floor_height = floor, floor_height
            return WATER_STEP_HIT_WALL if wall is not None else WATER_STEP_NONE

        if ceil_height - floor_height < WATER_HEADROOM * m.motion_scale:
            return WATER_STEP_CANCELLED

        # Too tight to fit: pulled down to hang off the ceiling instead.
        m.pos = [next_pos[0], ceil_height - WATER_HEADROOM * m.motion_scale,
                 next_pos[2]]
        m.floor, m.floor_height = floor, floor_height
        return WATER_STEP_HIT_CEILING

    if ceil_height - floor_height < WATER_HEADROOM * m.motion_scale:
        return WATER_STEP_CANCELLED

    m.pos = [next_pos[0], floor_height, next_pos[2]]
    m.floor, m.floor_height = floor, floor_height
    return WATER_STEP_HIT_FLOOR


def common_swimming_step(m, swim_strength):
    update_swimming_yaw(m)
    update_swimming_pitch(m)
    update_swimming_speed(m, swim_strength / 10.0)

    result = perform_water_step(m)

    if result == WATER_STEP_HIT_CEILING:
        if m.face_angle[0] > -0x3000:
            m.face_angle[0] = s16(m.face_angle[0] - 0x100)
    elif result == WATER_STEP_HIT_WALL:
        # Nose into a wall with no vertical input and he slides along it.
        if m.controller.stick_y == 0.0:
            if m.face_angle[0] > 0:
                m.face_angle[0] = min(s16(m.face_angle[0] + 0x200), 0x3F00)
            else:
                m.face_angle[0] = max(s16(m.face_angle[0] - 0x200), -0x3F00)

    return result


def set_water_plunge_action(m):
    """Enter the water, shedding most of the speed carried in."""
    m.forward_vel /= 4.0
    m.vel[1] /= 2.0
    m.pos[1] = m.water_level - PLUNGE_DEPTH * m.motion_scale
    m.face_angle[2] = 0
    m.angle_vel = [0, 0, 0]

    # A dive keeps its pitch so he knifes in; anything else levels out.
    if not (m.action & C.ACT_FLAG_DIVING):
        m.face_angle[0] = 0

    return set_mario_action(m, C.ACT_WATER_PLUNGE, 0)


def check_water_jump(m):
    """Leave the water upward when the player pulls back at the surface."""
    probe = int(m.pos[1] + 1.5)
    if not (m.input & C.INPUT_A_PRESSED):
        return False
    # Pulling back, which is positive here because the stick is mirrored --
    # see update_swimming_pitch.
    if (probe >= m.water_level - SURFACE_OFFSET * m.motion_scale
            and m.face_angle[0] >= 0
            and m.controller.stick_y > 60.0):
        m.angle_vel = [0, 0, 0]
        m.vel[1] = 62.0
        return set_mario_action(m, C.ACT_WATER_JUMP, 0)
    return False


def _stationary_slow_down(m):
    """Coast to a stop and level out, drifting toward neutral buoyancy."""
    buoyancy = get_buoyancy(m)
    m.angle_vel[0] = 0
    m.angle_vel[1] = 0

    m.forward_vel = approach_f32(m.forward_vel, 0.0, 1.0, 1.0)
    m.vel[1] = approach_f32(m.vel[1], buoyancy, 2.0, 1.0)

    m.face_angle[0] = approach_s32(m.face_angle[0], 0, 0x200, 0x200)
    m.face_angle[2] = approach_s32(m.face_angle[2], 0, 0x100, 0x100)

    m.vel[0] = m.forward_vel * coss(m.face_angle[0]) * sins(m.face_angle[1])
    m.vel[2] = m.forward_vel * coss(m.face_angle[0]) * coss(m.face_angle[1])


# -- actions ----------------------------------------------------------------


@action(C.ACT_WATER_PLUNGE, "water_plunge")
def act_water_plunge(m):
    # Diving in, or holding A on the way down, comes up swimming rather than
    # drifting to a stop.
    swim_out = bool(m.prev_action & C.ACT_FLAG_DIVING) or bool(m.input & C.INPUT_A_DOWN)
    end_vel_y = 0.0 if swimming_near_surface(m) else -5.0

    m.action_timer += 1
    _stationary_slow_down(m)
    result = perform_water_step(m)

    if m.action_state == 0:
        m.action_state = 1
        m.particle_flags |= C.PARTICLE_WATER_SPLASH

    # Settled once he lands, stops sinking, or simply runs out of patience.
    if (result == WATER_STEP_HIT_FLOOR or m.vel[1] >= end_vel_y
            or m.action_timer > 20):
        return set_mario_action(
            m, C.ACT_FLUTTER_KICK if swim_out else C.ACT_WATER_ACTION_END, 0)
    return False


@action(C.ACT_WATER_IDLE, "water_idle")
def act_water_idle(m):
    if m.input & C.INPUT_A_PRESSED:
        return set_mario_action(m, C.ACT_BREASTSTROKE, 0)
    if check_water_jump(m):
        return True
    common_swimming_step(m, MIN_SWIM_STRENGTH)
    return False


@action(C.ACT_WATER_ACTION_END, "water_action_end")
def act_water_action_end(m):
    if m.input & C.INPUT_A_PRESSED:
        return set_mario_action(m, C.ACT_BREASTSTROKE, 0)
    if check_water_jump(m):
        return True

    m.action_timer += 1
    common_swimming_step(m, MIN_SWIM_STRENGTH)
    # This is the settle back to a neutral float; once the clip has played
    # through there is nothing left to show. The length comes from the clip
    # metadata rather than from the renderer, so the simulation behaves the
    # same with no front end attached.
    from . import animations
    if m.action_timer >= animations.action_frame_count(m):
        set_mario_action(m, C.ACT_WATER_IDLE, 0)
    return False


@action(C.ACT_BREASTSTROKE, "breaststroke")
def act_breaststroke(m):
    # Arg 0 is a fresh stroke; arg 1 means this one was chained out of the
    # recovery, which is what keeps the strength built up there.
    if m.action_arg == 0:
        m.swim_strength = MIN_SWIM_STRENGTH

    m.action_timer += 1
    if m.action_timer == 14:
        return set_mario_action(m, C.ACT_FLUTTER_KICK, 0)

    if check_water_jump(m):
        return True

    # Two pushes per stroke: the arms sweeping out, then the legs closing.
    if m.action_timer < 6:
        m.forward_vel += 0.5
    if m.action_timer >= 9:
        m.forward_vel += 1.5

    if m.action_timer >= 2:
        # Pressing A again while the arms are still sweeping arms the restart.
        if m.action_timer < 6 and (m.input & C.INPUT_A_PRESSED):
            m.action_state = 1

        # Halfway through, an armed restart rewinds the clip and begins the
        # next stroke without ever leaving this action, so a held A reads as
        # one continuous cycle rather than a visible retrigger.
        if m.action_timer == 9 and m.action_state == 1:
            m.anim_reset = True
            m.action_state = 0
            m.action_timer = 1
            m.swim_strength = MIN_SWIM_STRENGTH

    if m.action_timer == 1:
        m.sound_events.append(
            C.SOUND_ACTION_SWIM if m.swim_strength == MIN_SWIM_STRENGTH
            else C.SOUND_ACTION_SWIM_FAST
        )

    common_swimming_step(m, m.swim_strength)
    return False


@action(C.ACT_SWIMMING_END, "swimming_end")
def act_swimming_end(m):
    if m.action_timer >= 15:
        return set_mario_action(m, C.ACT_WATER_ACTION_END, 0)
    if check_water_jump(m):
        return True

    # Pressing A again during the recovery chains into a stronger stroke.
    if (m.input & C.INPUT_A_DOWN) and m.action_timer >= 7:
        if m.action_timer == 7 and m.swim_strength < MAX_SWIM_STRENGTH:
            m.swim_strength += SWIM_STRENGTH_GAIN
        return set_mario_action(m, C.ACT_BREASTSTROKE, 1)

    if m.action_timer >= 7:
        m.swim_strength = MIN_SWIM_STRENGTH

    m.action_timer += 1
    m.forward_vel -= 0.25
    common_swimming_step(m, m.swim_strength)
    return False


@action(C.ACT_FLUTTER_KICK, "flutter_kick")
def act_flutter_kick(m):
    if not (m.input & C.INPUT_A_DOWN):
        if m.action_timer == 0 and m.swim_strength < MAX_SWIM_STRENGTH:
            m.swim_strength += SWIM_STRENGTH_GAIN
        return set_mario_action(m, C.ACT_SWIMMING_END, 0)

    m.forward_vel = approach_f32(m.forward_vel, 12.0, 0.1, 0.15)
    m.action_timer = 1
    m.swim_strength = MIN_SWIM_STRENGTH

    common_swimming_step(m, m.swim_strength)
    return False


@action(C.ACT_WATER_JUMP, "water_jump")
def act_water_jump(m):
    from .steps import perform_air_step

    if m.action_state == 0:
        m.action_state = 1
        m.sound_events.append(C.SOUND_MARIO_HOOHOO)

    result = perform_air_step(m)
    if result == C.AIR_STEP_LANDED:
        set_mario_action(m, C.ACT_JUMP_LAND, 0)
    elif result == C.AIR_STEP_HIT_WALL:
        m.set_forward_vel(15.0)
    # Falling back toward the water means the jump failed to clear it.
    elif (m.vel[1] < 0.0
          and m.pos[1] < m.water_level - PLUNGE_DEPTH * m.motion_scale):
        return set_water_plunge_action(m)
    return False


def check_common_water_cancels(m):
    """Enter the water if Mario has fallen far enough below the surface.

    Called for the non-submerged groups; the submerged actions manage the
    surface themselves.
    """
    if m.action & C.ACT_FLAG_SWIMMING:
        return False
    if m.action == C.ACT_WATER_JUMP:
        return False
    if m.pos[1] < m.water_level - PLUNGE_DEPTH * m.motion_scale:
        return set_water_plunge_action(m)
    return False


SUBMERGED_ACTIONS = frozenset({
    C.ACT_WATER_PLUNGE, C.ACT_WATER_IDLE, C.ACT_WATER_ACTION_END,
    C.ACT_BREASTSTROKE, C.ACT_SWIMMING_END, C.ACT_FLUTTER_KICK,
})

assert SUBMERGED_ACTIONS <= set(ACTIONS), "a submerged action failed to register"
