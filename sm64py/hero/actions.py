"""The Hero's action state machine.

Built to the moveset he actually has clips for, which is why it is a tenth the
size of Mario's. He walks, runs, jumps, falls, lands, swings a sword twice in a
chain, spin kicks out of a run, and wades. He does not crawl, ground pound,
long jump, dive, slide, wall kick, grab a ledge or swim -- not because those
were dropped, but because there is no animation for any of them and an action
with no pose to draw is a worse thing to have than no action.

The shape follows the decomp's: one function per action, registered in ACTIONS
and dispatched by id, returning True to say "I changed action, run the new one
this frame" so a transition costs no visible frame. Movement is handed to
`sm64py/mario/steps.py`, so walls, slopes, quarter steps and gravity behave
exactly as they do for Mario.
"""

from ..math_util import approach_f32, approach_s32, coss, s16, sins
from ..mario import constants as C
from ..mario.steps import (
    perform_air_step,
    perform_ground_step,
    stationary_ground_step,
)
from . import constants as H

ACTIONS = {}


def action(action_id, anim=None):
    def register(fn):
        ACTIONS[action_id] = (fn, anim or fn.__name__)
        return fn
    return register


# Sounds raised on entering an action, in the same append-and-forget style the
# Mario actions use: the simulation never plays anything itself, so it runs
# identically with or without an audio device.
_ENTRY_SOUNDS = {
    # The Hero has no voice sample of his own.  Reusing Mario's call here made
    # him sound like Mario whenever he jumped, so his jump is deliberately
    # silent; Mario's separate action table still raises both of his sounds.
    H.ACT_HERO_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    H.ACT_HERO_ATTACK: (C.SOUND_MARIO_YAH_WAH_HOO,),
    H.ACT_HERO_SPIN_KICK: (C.SOUND_MARIO_YAHOO,),
}


def set_hero_action(m, act, action_arg=0):
    if act != m.action:
        m.sound_events.extend(_ENTRY_SOUNDS.get(act, ()))

    m.prev_action = m.action
    m.action = act
    m.action_arg = action_arg
    m.action_state = 0
    m.action_timer = 0
    # Every action change restarts its clip. Mario's machine leaves this to
    # individual actions because several of them deliberately continue a clip
    # across a transition; nothing in this moveset does.
    m.anim_reset = True
    return True


# -- shared movement --------------------------------------------------------


def update_ground_speed(m):
    """Accelerate toward the stick, and turn toward it.

    The acceleration taper is the original's idea and worth keeping: without
    it, speed ramps linearly and the difference between a nudge and a shove
    disappears within a few frames.
    """
    # How far the stick is pressed as a fraction of its own ceiling, times the
    # cap. Clipping the stick's magnitude against the cap instead -- which is
    # what the decomp does, and what this used to do -- makes every cap above
    # 32 the same cap, since the stick never reports more than that. At the
    # default 30 the two agree at a full press and differ by 6% at a half one.
    target = min(m.intended_mag / H.MAX_STICK_MAG, 1.0) * H.MAX_WALK_SPEED

    if m.forward_vel <= 0.0:
        m.forward_vel += H.WALK_ACCEL
    elif m.forward_vel <= target:
        # The taper was written against a 30-unit top speed; above it the
        # acceleration would taper to nothing before the target was reached,
        # so it scales with the cap and keeps its shape.
        taper = H.ACCEL_TAPER * max(
            1.0, H.MAX_WALK_SPEED / H.TAPER_REFERENCE_SPEED)
        m.forward_vel += H.WALK_ACCEL - m.forward_vel / taper
    else:
        m.forward_vel -= H.DECELERATION

    m.forward_vel = min(m.forward_vel, H.MAX_RUN_SPEED)

    # Turning toward the stick rather than snapping to it, but faster than
    # Mario turns -- see TURN_RATE.
    m.face_angle[1] = s16(
        m.intended_yaw
        - approach_s32(s16(m.intended_yaw - m.face_angle[1]), 0,
                       H.TURN_RATE, H.TURN_RATE)
    )
    m.set_forward_vel(m.forward_vel)


def in_deep_water(m):
    return m.pos[1] < m.water_level - H.WADE_FLOAT_DEPTH


def check_common_exits(m):
    """Transitions every grounded action shares."""
    if m.input & C.INPUT_A_PRESSED:
        return set_hero_action(m, H.ACT_HERO_JUMP, 0)
    if m.input & C.INPUT_OFF_FLOOR:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)
    if in_deep_water(m):
        return set_hero_action(m, H.ACT_HERO_WADING, 0)
    return False


def check_attack_input(m):
    """B swings the sword, or spin kicks if he is already moving."""
    if not (m.input & C.INPUT_B_PRESSED):
        return False
    if m.forward_vel >= H.SPIN_KICK_MIN_SPEED:
        return set_hero_action(m, H.ACT_HERO_SPIN_KICK, 0)
    m.combo_index = 0
    return set_hero_action(m, H.ACT_HERO_ATTACK, 0)


# -- grounded ---------------------------------------------------------------


@action(H.ACT_HERO_IDLE, "idle")
def act_idle(m):
    if check_common_exits(m):
        return True
    if check_attack_input(m):
        return True
    if m.input & C.INPUT_Z_PRESSED:
        return set_hero_action(m, H.ACT_HERO_SWORD, 0)
    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_hero_action(m, H.ACT_HERO_WALKING, 0)

    m.forward_vel = approach_f32(m.forward_vel, 0.0, H.DECELERATION,
                                 H.DECELERATION)
    stationary_ground_step(m)
    m.action_timer += 1
    return False


@action(H.ACT_HERO_WALKING, "walking")
def act_walking(m):
    if check_common_exits(m):
        return True
    if check_attack_input(m):
        return True

    if not (m.input & C.INPUT_NONZERO_ANALOG):
        # Coast to a stop rather than cutting speed dead, then hand back to
        # idle once he has actually stopped.
        m.forward_vel = approach_f32(m.forward_vel, 0.0, H.BRAKE_DECELERATION,
                                     H.BRAKE_DECELERATION)
        m.set_forward_vel(m.forward_vel)
        if m.forward_vel <= 0.0:
            return set_hero_action(m, H.ACT_HERO_IDLE, 0)
    else:
        update_ground_speed(m)

    step = perform_ground_step(m)
    if step == C.GROUND_STEP_LEFT_GROUND:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)
    if step == C.GROUND_STEP_HIT_WALL:
        # Walls stop him rather than bouncing him: there is no bonk animation
        # in the set, and sliding to a halt against a wall reads fine.
        m.forward_vel = 0.0
        m.set_forward_vel(0.0)

    m.action_timer += 1
    return False


@action(H.ACT_HERO_LAND, "land")
def act_land(m):
    if m.input & C.INPUT_A_PRESSED:
        return set_hero_action(m, H.ACT_HERO_JUMP, 0)
    if check_attack_input(m):
        return True

    m.forward_vel = approach_f32(m.forward_vel, 0.0, H.BRAKE_DECELERATION,
                                 H.BRAKE_DECELERATION)
    m.set_forward_vel(m.forward_vel)
    perform_ground_step(m)

    m.action_timer += 1
    if m.action_timer >= H.LAND_FRAMES:
        if m.input & C.INPUT_NONZERO_ANALOG:
            return set_hero_action(m, H.ACT_HERO_WALKING, 0)
        return set_hero_action(m, H.ACT_HERO_IDLE, 0)
    return False


@action(H.ACT_HERO_SWORD, "sword")
def act_sword(m):
    """Draw or sheathe. Interruptible by anything that matters."""
    if check_common_exits(m):
        return True
    if m.input & C.INPUT_NONZERO_ANALOG:
        return set_hero_action(m, H.ACT_HERO_WALKING, 0)

    stationary_ground_step(m)
    m.action_timer += 1
    if m.action_timer >= H.SWORD_DRAW_FRAMES:
        m.sword_drawn = not m.sword_drawn
        return set_hero_action(m, H.ACT_HERO_IDLE, 0)
    return False


# -- combat -----------------------------------------------------------------


@action(H.ACT_HERO_ATTACK, "attack")
def act_attack(m):
    """One swing, with a window in which B buys the next one.

    The swing itself commits: the stick is ignored for its duration, which is
    what makes an attack feel like a decision rather than a suggestion. Only
    leaving the floor cancels it.
    """
    if m.input & C.INPUT_OFF_FLOOR:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)

    length = H.ATTACK1_FRAMES if m.combo_index == 0 else H.ATTACK2_FRAMES

    # The lunge the clip no longer carries. Applied as velocity so a wall can
    # stop it, and only over the opening frames, so the recovery stands still.
    if m.action_timer < H.ATTACK_LUNGE_FRAMES:
        m.forward_vel = H.ATTACK_LUNGE_SPEED
    else:
        m.forward_vel = approach_f32(m.forward_vel, 0.0, 2.0, 2.0)
    m.set_forward_vel(m.forward_vel)

    step = perform_ground_step(m)
    if step == C.GROUND_STEP_LEFT_GROUND:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)
    if step == C.GROUND_STEP_HIT_WALL:
        m.forward_vel = 0.0
        m.set_forward_vel(0.0)

    # Chaining is only offered on the first swing, and only mid-clip: pressing
    # B on frame 1 would let the whole combo play out in a third of the time.
    if (m.combo_index == 0
            and H.COMBO_WINDOW_START <= m.action_timer <= H.COMBO_WINDOW_END
            and m.input & C.INPUT_B_PRESSED):
        m.combo_index = 1
        return set_hero_action(m, H.ACT_HERO_ATTACK, 0)

    m.action_timer += 1
    if m.action_timer >= length:
        m.combo_index = 0
        if m.input & C.INPUT_NONZERO_ANALOG:
            return set_hero_action(m, H.ACT_HERO_WALKING, 0)
        return set_hero_action(m, H.ACT_HERO_IDLE, 0)
    return False


@action(H.ACT_HERO_SPIN_KICK, "spin_kick")
def act_spin_kick(m):
    """The running attack: he keeps his speed and carries it through the spin."""
    if m.input & C.INPUT_OFF_FLOOR:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)

    if m.action_timer == 0:
        m.forward_vel = max(m.forward_vel, H.SPIN_KICK_SPEED)
    else:
        m.forward_vel = approach_f32(m.forward_vel, 0.0, 0.8, 0.8)
    m.set_forward_vel(m.forward_vel)

    step = perform_ground_step(m)
    if step == C.GROUND_STEP_LEFT_GROUND:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)
    if step == C.GROUND_STEP_HIT_WALL:
        m.forward_vel = 0.0
        m.set_forward_vel(0.0)

    m.action_timer += 1
    if m.action_timer >= H.SPIN_KICK_FRAMES:
        if m.input & C.INPUT_NONZERO_ANALOG:
            return set_hero_action(m, H.ACT_HERO_WALKING, 0)
        return set_hero_action(m, H.ACT_HERO_IDLE, 0)
    return False


# -- airborne ---------------------------------------------------------------


def update_air_movement(m):
    """Air control: the stick steers, but weakly, and never adds speed."""
    m.forward_vel = approach_f32(m.forward_vel, 0.0, 0.35, 0.35)

    if m.input & C.INPUT_NONZERO_ANALOG:
        intended_dyaw = s16(m.intended_yaw - m.face_angle[1])
        mag = m.intended_mag / 32.0
        m.forward_vel += mag * coss(intended_dyaw) * 1.5
        sideways = mag * sins(intended_dyaw) * 10.0
    else:
        sideways = 0.0

    if m.forward_vel > H.MAX_RUN_SPEED:
        m.forward_vel -= 1.0

    m.slide_vel_x = m.forward_vel * sins(m.face_angle[1])
    m.slide_vel_z = m.forward_vel * coss(m.face_angle[1])
    m.slide_vel_x += sideways * sins(s16(m.face_angle[1] + 0x4000))
    m.slide_vel_z += sideways * coss(s16(m.face_angle[1] + 0x4000))
    m.vel[0] = m.slide_vel_x
    m.vel[2] = m.slide_vel_z


def land_from_air(m):
    """Which landing to play, and whether he keeps running through it."""
    if in_deep_water(m):
        return set_hero_action(m, H.ACT_HERO_WADING, 0)
    heavy = -m.vel[1] >= H.HEAVY_LANDING_SPEED
    return set_hero_action(m, H.ACT_HERO_LAND, 1 if heavy else 0)


@action(H.ACT_HERO_JUMP, "jump")
def act_jump(m):
    if m.action_timer == 0:
        m.vel[1] = H.JUMP_VELOCITY + m.forward_vel * H.JUMP_SPEED_BONUS
        # Read by the gravity rule in steps.py: without it, releasing the
        # button on the way up does not shorten the jump.
        m.flags |= C.MARIO_JUMPING

    update_air_movement(m)
    step = perform_air_step(m)

    if step == C.AIR_STEP_LANDED:
        m.flags &= ~C.MARIO_JUMPING
        return land_from_air(m)
    if step == C.AIR_STEP_HIT_WALL:
        m.forward_vel = 0.0

    m.action_timer += 1
    # Once he is coming back down the falling clip takes over, so a long jump
    # does not hold the take-off pose all the way to the ground.
    if m.vel[1] < 0.0:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)
    return False


@action(H.ACT_HERO_FALL, "fall")
def act_fall(m):
    update_air_movement(m)
    step = perform_air_step(m)

    if step == C.AIR_STEP_LANDED:
        m.flags &= ~C.MARIO_JUMPING
        return land_from_air(m)
    if step == C.AIR_STEP_HIT_WALL:
        m.forward_vel = 0.0

    m.action_timer += 1
    return False


# -- water ------------------------------------------------------------------


@action(H.ACT_HERO_WADING, "wading")
def act_wading(m):
    """Deep water: he floats at the surface and wades, because he cannot swim.

    Stated plainly rather than hidden: there is no swimming clip in the set, so
    rather than draw a walk cycle underwater he is held at the surface and
    slowed down. Mario, switched to with the same key, still swims properly.
    """
    if not in_deep_water(m):
        if m.input & C.INPUT_OFF_FLOOR:
            return set_hero_action(m, H.ACT_HERO_FALL, 0)
        return set_hero_action(m, H.ACT_HERO_IDLE, 0)

    if m.input & C.INPUT_NONZERO_ANALOG:
        target = min(m.intended_mag, H.MAX_WALK_SPEED) * H.WADE_SPEED_SCALE
        m.forward_vel = approach_f32(m.forward_vel, target, 0.6, 0.6)
        m.face_angle[1] = s16(
            m.intended_yaw
            - approach_s32(s16(m.intended_yaw - m.face_angle[1]), 0,
                           H.TURN_RATE, H.TURN_RATE)
        )
    else:
        m.forward_vel = approach_f32(m.forward_vel, 0.0, 0.6, 0.6)

    m.set_forward_vel(m.forward_vel)

    # Buoyancy toward the float depth, rather than gravity. Approaching it
    # instead of snapping keeps him from popping to the surface when he runs
    # off a bank into deep water.
    surface = m.water_level - H.WADE_FLOAT_DEPTH
    m.vel[1] = approach_f32(m.vel[1], 0.0, 4.0, 4.0)
    m.pos[1] = approach_f32(m.pos[1], surface, 8.0, 8.0)

    perform_ground_step(m)
    m.pos[1] = max(m.pos[1], min(surface, m.pos[1]))
    m.sync_graphics()

    m.action_timer += 1
    return False


# -- driver -----------------------------------------------------------------


def execute_action(m):
    """Run the Hero for one frame. Returns the action that ended up running."""
    m.update_inputs()
    m.sound_events.clear()

    for _ in range(8):
        handler = ACTIONS.get(m.action)
        if handler is None:
            set_hero_action(m, H.ACT_HERO_IDLE, 0)
            handler = ACTIONS[H.ACT_HERO_IDLE]
        fn, anim = handler
        m.anim_name = anim
        if not fn(m):
            break

    m.sync_graphics()
    return m.action
