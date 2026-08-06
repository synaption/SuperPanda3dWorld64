"""Movement integration: quarter steps, wall pushback, landing and ledges.

Mario never moves a whole frame's velocity at once.  Each frame is split into
four quarter steps, and every quarter step independently resolves walls, then
the floor, then the ceiling.  That is why he can round a corner smoothly at
speed, and also why very fast movement can pass through thin geometry -- the
sub-step is still a teleport, just a shorter one.
"""

from ..math_util import atan2s, s16
from ..surfaces import WallCollisionData
from . import constants as C


def find_wall_collisions(m, pos, offset_y, radius):
    """Push `pos` (a mutable 3-list) out of nearby walls; returns hit count."""
    data = WallCollisionData(pos[0], pos[1], pos[2], offset_y, radius)
    num = m.surfaces.find_wall_collisions(data)
    pos[0] = data.x
    pos[2] = data.z
    return num


def resolve_and_return_wall_collisions(m, pos, offset, radius):
    """Push `pos` clear of walls and return the first wall hit, if any."""
    data = WallCollisionData(pos[0], pos[1], pos[2], offset, radius)
    m.surfaces.find_wall_collisions(data)
    pos[0] = data.x
    pos[2] = data.z
    return data.walls[0] if data.walls else None


def stop_and_set_height_to_floor(m):
    m.set_forward_vel(0.0)
    m.vel[1] = 0.0
    m.pos[1] = m.floor_height
    m.sync_graphics()


# -- ground -----------------------------------------------------------------


def stationary_ground_step(m):
    m.set_forward_vel(0.0)
    m.pos[1] = m.floor_height
    m.sync_graphics()
    return C.GROUND_STEP_NONE


def _perform_ground_quarter_step(m, next_pos):
    # The lower check keeps Mario off small steps; the upper one is what
    # actually counts as "hitting a wall" for gameplay purposes.
    resolve_and_return_wall_collisions(m, next_pos, 30.0, 24.0)
    upper_wall = resolve_and_return_wall_collisions(m, next_pos, 60.0, 50.0)

    floor_height, floor = m.surfaces.find_floor(*next_pos)
    ceil_height, _ = m.find_ceil(next_pos, floor_height)

    m.wall = upper_wall

    if floor is None:
        return C.GROUND_STEP_HIT_WALL_STOP_QSTEPS

    if next_pos[1] > floor_height + 100.0:
        # Walked off a ledge.
        if next_pos[1] + 160.0 >= ceil_height:
            return C.GROUND_STEP_HIT_WALL_STOP_QSTEPS

        m.pos[:] = next_pos
        m.floor = floor
        m.floor_height = floor_height
        return C.GROUND_STEP_LEFT_GROUND

    # Not enough headroom to stand here.
    if floor_height + 160.0 >= ceil_height:
        return C.GROUND_STEP_HIT_WALL_STOP_QSTEPS

    m.pos[0], m.pos[1], m.pos[2] = next_pos[0], floor_height, next_pos[2]
    m.floor = floor
    m.floor_height = floor_height

    if upper_wall is not None:
        wall_dyaw = s16(upper_wall.yaw - m.face_angle[1])
        # Glancing hits let Mario slide along the wall instead of stopping.
        if 0x2AAA <= wall_dyaw <= 0x5555:
            return C.GROUND_STEP_NONE
        if -0x5555 <= wall_dyaw <= -0x2AAA:
            return C.GROUND_STEP_NONE
        return C.GROUND_STEP_HIT_WALL_CONTINUE_QSTEPS

    return C.GROUND_STEP_NONE


def perform_ground_step(m):
    step_result = C.GROUND_STEP_NONE

    for _ in range(4):
        # Scaling by the floor normal's Y slows Mario down going up a slope
        # and speeds him up going down it.
        ny = m.floor.normal[1] if m.floor is not None else 1.0
        next_pos = [
            m.pos[0] + ny * (m.vel[0] / 4.0),
            m.pos[1],
            m.pos[2] + ny * (m.vel[2] / 4.0),
        ]

        step_result = _perform_ground_quarter_step(m, next_pos)
        if step_result in (C.GROUND_STEP_LEFT_GROUND,
                           C.GROUND_STEP_HIT_WALL_STOP_QSTEPS):
            break

    m.sync_graphics()

    if step_result == C.GROUND_STEP_HIT_WALL_CONTINUE_QSTEPS:
        step_result = C.GROUND_STEP_HIT_WALL
    return step_result


# -- air --------------------------------------------------------------------


def check_ledge_grab(m, wall, intended_pos, next_pos):
    if m.vel[1] > 0:
        return False

    displacement_x = next_pos[0] - intended_pos[0]
    displacement_z = next_pos[2] - intended_pos[2]

    # Only grab if the wall pushed Mario back against his own motion.
    if displacement_x * m.vel[0] + displacement_z * m.vel[2] > 0.0:
        return False

    # The floor search starts well above Mario, so a ledge somewhat higher
    # than expected can be caught.
    ledge_x = next_pos[0] - wall.normal[0] * 60.0
    ledge_z = next_pos[2] - wall.normal[2] * 60.0
    ledge_y, ledge_floor = m.surfaces.find_floor(
        ledge_x, next_pos[1] + 160.0, ledge_z
    )

    if ledge_floor is None or ledge_y - next_pos[1] <= 100.0:
        return False

    m.pos[:] = [ledge_x, ledge_y, ledge_z]
    m.floor = ledge_floor
    m.floor_height = ledge_y
    m.floor_angle = atan2s(ledge_floor.normal[2], ledge_floor.normal[0])
    m.face_angle[0] = 0
    m.face_angle[1] = s16(wall.yaw + 0x8000)
    return True


def _perform_air_quarter_step(m, intended_pos, step_arg):
    next_pos = list(intended_pos)

    upper_wall = resolve_and_return_wall_collisions(m, next_pos, 150.0, 50.0)
    lower_wall = resolve_and_return_wall_collisions(m, next_pos, 30.0, 50.0)

    floor_height, floor = m.surfaces.find_floor(*next_pos)
    ceil_height, ceil = m.find_ceil(next_pos, floor_height)

    m.wall = None

    if floor is None:
        # Out of bounds: only a downward move counts as landing.
        if next_pos[1] <= m.floor_height:
            m.pos[1] = m.floor_height
            return C.AIR_STEP_LANDED
        m.pos[1] = next_pos[1]
        return C.AIR_STEP_HIT_WALL

    if next_pos[1] <= floor_height:
        if ceil_height - floor_height > 160.0:
            m.pos[0], m.pos[2] = next_pos[0], next_pos[2]
            m.floor = floor
            m.floor_height = floor_height
        m.pos[1] = floor_height
        return C.AIR_STEP_LANDED

    if next_pos[1] + 160.0 > ceil_height:
        # Bonked the ceiling: keep horizontal motion but stop rising.
        if m.vel[1] >= 0.0:
            m.vel[1] = 0.0
            if step_arg & C.AIR_STEP_CHECK_HANG and ceil is not None:
                if ceil.type == C.SURFACE_HANGABLE:
                    return C.AIR_STEP_GRABBED_CEILING
            return C.AIR_STEP_NONE
        # Falling into a ceiling still lets Mario move down and across.
        if next_pos[1] <= floor_height:
            m.pos[1] = floor_height
            return C.AIR_STEP_LANDED
        m.pos[1] = next_pos[1]
        return C.AIR_STEP_HIT_WALL

    m.pos[:] = next_pos
    m.floor = floor
    m.floor_height = floor_height

    if upper_wall is not None or lower_wall is not None:
        wall = upper_wall if upper_wall is not None else lower_wall
        wall_dyaw = s16(wall.yaw - m.face_angle[1])

        if step_arg & C.AIR_STEP_CHECK_LEDGE_GRAB and upper_wall is None:
            if check_ledge_grab(m, wall, intended_pos, next_pos):
                return C.AIR_STEP_GRABBED_LEDGE

        m.wall = wall
        # Only a fairly head-on hit counts; glancing contact is ignored.
        if wall_dyaw < -0x6000 or wall_dyaw > 0x6000:
            return C.AIR_STEP_HIT_WALL

    return C.AIR_STEP_NONE


def apply_gravity(m):
    if m.action in (C.ACT_LONG_JUMP, C.ACT_SLIDE_KICK):
        m.vel[1] -= 2.0
        m.vel[1] = max(m.vel[1], -75.0)
    elif m.action == C.ACT_TWIRLING and m.vel[1] < 0.0:
        m.vel[1] -= 4.0
        m.vel[1] = max(m.vel[1], -75.0)
    elif _should_strengthen_gravity_for_jump_ascent(m):
        # Releasing A partway up cuts the jump short.
        m.vel[1] /= 4.0
    elif (m.flags & C.MARIO_WING_CAP) and m.vel[1] < 0.0 and (m.input & C.INPUT_A_DOWN):
        m.vel[1] -= 2.0
        if m.vel[1] < -37.5:
            m.vel[1] += 4.0
            if m.vel[1] > -37.5:
                m.vel[1] = -37.5
    else:
        m.vel[1] -= 4.0
        m.vel[1] = max(m.vel[1], -75.0)


def _should_strengthen_gravity_for_jump_ascent(m):
    """Releasing A early while still rising fast cuts the jump short."""
    if not (m.flags & C.MARIO_JUMPING):
        return False
    if m.action & (C.ACT_FLAG_INTANGIBLE | C.ACT_FLAG_INVULNERABLE):
        return False
    if m.input & C.INPUT_A_DOWN or m.vel[1] <= 20.0:
        return False
    return bool(m.action & C.ACT_FLAG_CONTROL_JUMP_HEIGHT)


def perform_air_step(m, step_arg=0):
    step_result = C.AIR_STEP_NONE
    m.wall = None

    for _ in range(4):
        intended_pos = [
            m.pos[0] + m.vel[0] / 4.0,
            m.pos[1] + m.vel[1] / 4.0,
            m.pos[2] + m.vel[2] / 4.0,
        ]

        quarter_result = _perform_air_quarter_step(m, intended_pos, step_arg)
        if quarter_result != C.AIR_STEP_NONE:
            step_result = quarter_result

        if quarter_result in (C.AIR_STEP_LANDED, C.AIR_STEP_GRABBED_LEDGE,
                              C.AIR_STEP_GRABBED_CEILING,
                              C.AIR_STEP_HIT_LAVA_WALL):
            break

    if m.vel[1] >= 0.0:
        m.peak_height = m.pos[1]

    apply_gravity(m)
    m.sync_graphics()

    return step_result
