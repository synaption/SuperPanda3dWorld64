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

from .. import audio
from ..math_util import approach_f32, approach_s32, coss, s16, sins
from ..mario import constants as C
# Two helpers rather than two actions: `update_sliding` is SM64's ice and
# `bounce_off_wall` is what a slide does when it meets one, and both are
# character-agnostic in the same way the step functions below are. They live in
# the decomp's actions.c rather than in a physics file, which is the only reason
# this import reaches into Mario's module at all.
from ..mario.actions import bounce_off_wall, update_sliding
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
    H.ACT_HERO_JUMP: (audio.SOUND_HERO_JUMP,),
    H.ACT_HERO_LAND: (C.SOUND_ACTION_TERRAIN_LANDING,),
    H.ACT_HERO_ATTACK: (audio.SOUND_HERO_ATTACK_1,),
    H.ACT_HERO_SPIN_KICK: (audio.SOUND_HERO_SPIN_KICK,),
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


def keep_the_stride(m):
    """Complete a transition without restarting the clip. Returns True.

    `set_hero_action` restarts the animation on every change, which is what
    nineteen of the twenty transitions in here want and what the run and the
    skate do not: both draw the same run cycle, so resetting it drops him back
    to the top of his stride on the frame the trigger moves. That is a visible
    hitch on a control the player holds and lets go of constantly, and there is
    nothing to restart -- it is the same clip on both sides.

    Harmless on the transitions where it is not needed, since a clip that is
    not the one already playing starts from its own first frame regardless.
    """
    m.anim_reset = False
    return True


def launch_jetpack(m, lift=True):
    """Out of the skate and into the air, with no jump under it.

    No jump under it in the sense that matters: `ACT_HERO_JUMP` is never
    entered, so there is no take-off arc governed by the button, no jump
    physics and no landing waiting at the end of one. What he draws on the way
    up is the flight's own pose, which is `jump up` because there is no flying
    clip among his twenty.

    `lift` is the difference between choosing to go up and simply running out
    of ground. A launch gets the booster kick; skating off the edge of
    something keeps whatever vertical velocity he had, so a ledge drops him the
    way a ledge should and the jets only stop him falling further.
    """
    set_hero_action(m, H.ACT_HERO_JETPACK, 0)
    if lift:
        m.vel[1] = max(m.vel[1], H.JETPACK_LAUNCH_SPEED)
    return True


def check_common_exits(m):
    """Transitions every grounded action shares."""
    # The trigger before A, and the order is the control scheme: with the
    # trigger down he is on his skates, and A out of the skate is a take-off
    # rather than a jump. Reading A first would mean the two pressed together
    # jumped, and a jump is exactly the thing the launch is not.
    if m.controller.thrust:
        set_hero_action(m, H.ACT_HERO_SKATING, 0)
        return keep_the_stride(m)
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
    # The landing swallows most input for its eight frames, but not the
    # boosters: a trigger pressed as he touches down should put him on his
    # skates, not wait for the pose to finish first.
    if m.controller.thrust:
        return set_hero_action(m, H.ACT_HERO_SKATING, 0)
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


# -- the skates -------------------------------------------------------------


@action(H.ACT_HERO_SKATING, "skating")
def act_skating(m):
    """The boosters at ground level: he skims instead of lifting off.

    Holding the trigger on the ground is its own thing rather than a slower
    take-off, and that is what fixed the worst bug the jetpack had. Thrust used
    to lift him the moment it was pressed, at 8 units a frame -- which loses to
    any slope he can run up at 38 units a frame forward, so the air step landed
    him on the frame it started, the landing pose played, the trigger was still
    down, and he lit the boosters again. Running uphill on the trigger was a
    landing animation on a loop and no flight at all.

    Now there is no take-off to fail. Ground under a burning jetpack is a
    skate, whether he started there or flew into it, and A is what leaves.
    """
    if not m.controller.thrust:
        # Hand back whatever speed he had rather than dropping it, so stepping
        # off the jets at pace carries into a run -- and carries the stride
        # with it, since at that speed it is the same cycle on both sides.
        set_hero_action(
            m, H.ACT_HERO_WALKING if m.forward_vel > 0.0 else H.ACT_HERO_IDLE,
            0)
        return keep_the_stride(m)
    if m.input & C.INPUT_A_PRESSED:
        return launch_jetpack(m)
    if in_deep_water(m):
        return set_hero_action(m, H.ACT_HERO_WADING, 0)
    if m.input & C.INPUT_OFF_FLOOR:
        return launch_jetpack(m, lift=False)

    # Pushing or coasting. Nothing else differs between the two, and the pose
    # does not either -- he is riding thrust, not striding.
    pushing = m.intended_mag > 0.0

    if pushing and m.forward_vel < H.SKATE_TOP_SPEED:
        push = H.SKATE_PUSH * (m.intended_mag / H.MAX_STICK_MAG)
        m.slide_vel_x += push * sins(m.intended_yaw)
        m.slide_vel_z += push * coss(m.intended_yaw)

    # The jets holding the hill, applied against the pull `update_sliding` is
    # about to add and in the same terms, so the two are one subtraction. See
    # SKATE_GRIP for why a slide's answer to a slope is the wrong one here.
    grip = H.SKATE_GRIP * m.get_slope_steepness()
    m.slide_vel_x -= grip * sins(m.floor_angle)
    m.slide_vel_z -= grip * coss(m.floor_angle)

    update_sliding(m, 0.0 if pushing else H.SKATE_STOP_SPEED)
    step = perform_ground_step(m)

    if step == C.GROUND_STEP_LEFT_GROUND:
        return launch_jetpack(m, lift=False)
    if step == C.GROUND_STEP_HIT_WALL:
        # Off the boards and back down the rink, the way a slide bounces.
        # Stopping dead against a wall is the one thing that would not read as
        # a character carrying this much momentum.
        bounce_off_wall(m)

    m.action_timer += 1
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
        m.sound_events.append(audio.SOUND_HERO_ATTACK_2)
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


def update_jetpack_movement(m):
    """Steer under thrust the way he steers on the ground.

    The weak air control above is a jump's: he keeps the heading he took off
    with and the stick only nudges him off it. Under power there is nothing
    holding him to that heading, so the boosters get the running controls
    outright -- the same turn toward the stick at `TURN_RATE`, the same
    acceleration and the same top speed -- which is what lets him fly a circle
    around the camera instead of drifting sideways with his back to it.
    """
    if m.input & C.INPUT_NONZERO_ANALOG:
        update_ground_speed(m)
    else:
        # Coast to a stop rather than keeping the speed he flew in with, the
        # same as letting go of the stick on the ground does.
        m.forward_vel = approach_f32(m.forward_vel, 0.0, H.DECELERATION,
                                     H.DECELERATION)
        m.set_forward_vel(m.forward_vel)


def land_from_air(m):
    """Which landing to play, and whether he keeps running through it."""
    if in_deep_water(m):
        return set_hero_action(m, H.ACT_HERO_WADING, 0)
    heavy = -m.vel[1] >= H.HEAVY_LANDING_SPEED
    return set_hero_action(m, H.ACT_HERO_LAND, 1 if heavy else 0)


def check_jetpack_hold(m):
    """The trigger, in the air, lights the boosters.

    The hold rather than the press, and it cannot arrive stale: the only way to
    hold the trigger on the ground is to be skating, and the only way out of a
    skate on A is the launch, so a jump with the trigger already down is not a
    state this machine can be in. A is not asked about at all any more -- it
    used to become the boosters once it had been held past a few frames, which
    meant a jump you were slow off the button on turned into a flight.
    """
    if not m.controller.thrust:
        return False
    return set_hero_action(m, H.ACT_HERO_JETPACK, 0)


@action(H.ACT_HERO_JUMP, "jump")
def act_jump(m):
    if m.action_timer == 0:
        m.vel[1] = H.JUMP_VELOCITY + m.forward_vel * H.JUMP_SPEED_BONUS
        # Read by the gravity rule in steps.py: without it, releasing the
        # button on the way up does not shorten the jump.
        m.flags |= C.MARIO_JUMPING

    if check_jetpack_hold(m):
        return True

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
    # The hold is safe to read here now that the trigger is the only control
    # that flies him: the flight only ever falls out of `act_jetpack` because
    # the trigger came *up*, so a held one on this frame is a new one. When A
    # answered here as well it had to be the press, since falling with A still
    # down is the ordinary end of a jump.
    if check_jetpack_hold(m):
        return True

    update_air_movement(m)
    step = perform_air_step(m)

    if step == C.AIR_STEP_LANDED:
        m.flags &= ~C.MARIO_JUMPING
        return land_from_air(m)
    if step == C.AIR_STEP_HIT_WALL:
        m.forward_vel = 0.0

    m.action_timer += 1
    return False


@action(H.ACT_HERO_JETPACK, "jetpack")
def act_jetpack(m):
    """Thrust up for as long as it is asked for, flown with the running controls.

    The thrust is written as an approach toward a rise speed, run before the
    air step rather than after it, which is what makes the number behave: the
    step moves him by the velocity it is given and only then hands 4 units of
    it back to gravity, so the approach re-covers that loss every frame and he
    settles at exactly `JETPACK_RISE_SPEED` instead of somewhere under it.
    """
    if m.action_timer == 0:
        # He is no longer in a jump whose height the button governs, and
        # leaving the flag set would have steps.py quarter his velocity the
        # frame he lets go rather than simply letting him fall.
        m.flags &= ~C.MARIO_JUMPING

    if not m.controller.thrust:
        return set_hero_action(m, H.ACT_HERO_FALL, 0)

    m.vel[1] = approach_f32(m.vel[1], H.JETPACK_RISE_SPEED,
                            H.JETPACK_THRUST, H.JETPACK_THRUST)

    update_jetpack_movement(m)
    step = perform_air_step(m)

    if step == C.AIR_STEP_LANDED:
        # Ground under a burning jetpack is a skate, never a landing -- and the
        # trigger is necessarily still down, since the check at the top of this
        # action is the only way to reach here. Hugging a rising slope on the
        # way up puts him on his skates and he carries on up the hill; before
        # this it put him in the landing pose, from which the same trigger lit
        # the boosters again, and that loop was the whole of the bug.
        return set_hero_action(m, H.ACT_HERO_SKATING, 0)
    if step == C.AIR_STEP_HIT_WALL:
        # Through the setter rather than the field alone, as the grounded
        # actions do it: it clears the horizontal velocity the wall stopped as
        # well as the speed behind it, so nothing else reads a frame of motion
        # into a wall before the next one recomputes it.
        m.set_forward_vel(0.0)

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
