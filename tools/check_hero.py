"""Exercise the Hero's action machine headlessly, the way check_movement.py does.

Same synthetic flat floor and the same reasons for it -- terrain cannot confuse
a reading taken on a plane, and the two ways of getting that plane wrong
(winding and s16 range) both fail quietly.

What is being checked is not "does it run" but the handful of things that would
be invisible in a screenshot and wrong in play: that he reaches a settled
speed, that a jump leaves the floor and comes back to it, that the attack chain
advances and terminates instead of looping forever, and that every action the
machine can reach has a clip to draw.

    python3 tools/check_hero.py
"""

import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py.camera import FollowCamera  # noqa: E402
from sm64py.hero import HeroState, execute_action  # noqa: E402
from sm64py.hero import animations as A  # noqa: E402
from sm64py.hero import constants as H  # noqa: E402
from sm64py.mario import Controller  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402
from sm64py.surfaces import SurfaceSet  # noqa: E402

TICK_DT = 1.0 / 30.0
PLANE = 30000

CLIPS = os.path.join(ROOT, "assets", "hero", "hero_clips.json")


def flat_ground():
    vertices = np.array([[-PLANE, 0, -PLANE], [PLANE, 0, -PLANE],
                         [PLANE, 0, PLANE], [-PLANE, 0, PLANE]], dtype=np.int32)
    triangles = np.array([[0, 2, 1], [0, 3, 2]], dtype=np.int32)
    zeros = np.zeros(2, dtype=np.int32)
    return SurfaceSet(vertices, triangles, zeros, zeros, None)


# A 25 degree hill, which is gentler than the one up to the castle door and
# steep enough for what it is here to test: at a 38 unit run it lifts the floor
# 16 units a frame, and the boosters climb 8 on the frame they light. Anything
# that tries to take off by out-climbing the ground loses on a hill like this.
HILL_RISE, HILL_RUN = 3000, 6400


def ground_with_a_hill():
    """Flat until z = 0, then a hill climbing away from it."""
    vertices = np.array([
        [-PLANE, 0, -PLANE], [PLANE, 0, -PLANE], [PLANE, 0, 0], [-PLANE, 0, 0],
        [PLANE, HILL_RISE, HILL_RUN], [-PLANE, HILL_RISE, HILL_RUN],
    ], dtype=np.int32)
    triangles = np.array([[0, 2, 1], [0, 3, 2], [3, 4, 2], [3, 5, 4]],
                         dtype=np.int32)
    zeros = np.zeros(4, dtype=np.int32)
    return SurfaceSet(vertices, triangles, zeros, zeros, None)


class Run:
    def __init__(self, pos=(0.0, 10.0, 0.0), yaw=0.0, surfaces=None):
        self.surfaces = surfaces if surfaces is not None else flat_ground()
        self.controller = Controller()
        self.hero = HeroState(self.surfaces, self.controller)
        self.hero.spawn(*pos, yaw)
        self.camera = FollowCamera(self.surfaces, self.hero)

    def tick(self, forward=0.0, buttons=0, thrust=False):
        # Negative stick_y is forward; app/main.py feeds both axes mirrored.
        self.controller.set_stick(0.0, -forward)
        self.controller.set_buttons(buttons)
        self.controller.set_thrust(thrust)
        self.hero.camera_yaw = self.camera.mario_yaw
        execute_action(self.hero)
        self.camera.update(TICK_DT)
        return self.hero.action

    @property
    def height(self):
        return self.hero.pos[1]

    @property
    def grounded(self):
        # Against the floor rather than against zero, so the same reading works
        # on the hill as on the plane.
        return self.hero.pos[1] - self.hero.floor_height <= 0.01


def check_walking():
    run = Run()
    speeds = []
    for _ in range(200):
        run.tick(forward=1.0)
        speeds.append(run.hero.forward_vel)
    tail = speeds[100:]
    settled = sum(tail) / len(tail)
    clip, _loop, rate = A.resolve(run.hero)
    ok = (H.RUN_SPEED < settled <= H.MAX_RUN_SPEED
          and run.hero.action == H.ACT_HERO_WALKING and clip == A.RUN
          and rate == 1.0)
    return ok, (f"settles at {settled:.1f} u/frame "
                f"(max {H.MAX_RUN_SPEED}), playing {clip!r} at {rate:.1f}x")


def check_stop():
    run = Run()
    for _ in range(60):
        run.tick(forward=1.0)
    for _ in range(120):
        run.tick(forward=0.0)
    ok = run.hero.action == H.ACT_HERO_IDLE and abs(run.hero.forward_vel) < 0.01
    return ok, f"releasing the stick returns to {run.hero.action_name!r}"


def check_jump():
    run = Run()
    peak = 0.0
    seen = set()
    launched = False
    for _ in range(150):
        buttons = C.A_BUTTON if not launched else 0
        act = run.tick(buttons=buttons)
        seen.add(act)
        if not run.grounded:
            launched = True
        peak = max(peak, run.height)
    ok = (peak > 100.0 and run.grounded
          and H.ACT_HERO_JUMP in seen and H.ACT_HERO_FALL in seen
          and H.ACT_HERO_LAND in seen and run.hero.action == H.ACT_HERO_IDLE)
    return ok, (f"peak {peak:.0f} units, jump -> fall -> land -> "
                f"{run.hero.action_name}")


def check_jump_height_is_variable():
    """Releasing the button early must give a lower jump than holding it.

    Held all the way now, rather than held to the last frame before the
    boosters would have taken over: A is a jump and nothing else, so there is
    no longer a length of hold that turns it into a flight and stops this from
    measuring a jump at all.
    """
    def peak(frames_held):
        run = Run()
        airborne = 0
        best = 0.0
        for _ in range(150):
            buttons = C.A_BUTTON if airborne <= frames_held else 0
            run.tick(buttons=buttons)
            if not run.grounded:
                airborne += 1
            best = max(best, run.height)
        return best

    held, tapped = peak(150), peak(0)
    return held > tapped + 20.0, f"held {held:.0f} vs tapped {tapped:.0f} units"


def check_the_trigger_skates_him():
    """On the ground the boosters skate rather than lift.

    Both halves matter. He must not leave the floor -- a trigger that took off
    on its own is the behaviour this replaced -- and he must end up faster than
    his own legs, because a skate that is slower than running is a downgrade
    nobody would press the button for.
    """
    run = Run()
    for _ in range(4):
        run.tick()                       # settle on the floor
    started = run.hero.action_name

    actions, lift = [], 0.0
    for _ in range(120):
        run.tick(forward=1.0, thrust=True)
        lift = max(lift, run.height)
        name = run.hero.action_name
        if not actions or actions[-1] != name:
            actions.append(name)

    clip, _loop, rate = A.resolve(run.hero)
    problems = []
    if clip != A.RUN:
        problems.append(f"it drew {clip!r} rather than the run")
    if rate < 1.0:
        problems.append(f"the run played at {rate:.2f}, slower than a run")
    if actions[:1] != ["skating"]:
        problems.append(f"{started} went to {actions[:1]}, not to the skates")
    if set(actions) != {"skating"}:
        problems.append(f"it did not stay on them: {actions}")
    if lift > 1.0:
        problems.append(f"it lifted him {lift:.0f} units off the floor")
    if run.hero.forward_vel <= H.MAX_RUN_SPEED:
        problems.append(f"only reached {run.hero.forward_vel:.0f} u/frame, "
                        f"no better than his {H.MAX_RUN_SPEED} run")

    return not problems, ("; ".join(problems) if problems
                          else f"{started} -> skating at "
                               f"{run.hero.forward_vel:.0f} u/frame, "
                               f"{lift:.1f} units of lift, "
                               f"{clip!r} at {rate:.2f}")


def check_the_stride_survives_the_trigger():
    """Going onto the skates at a run, and off them, must not reset the stride.

    Both sides draw the same cycle, so a restart is not a change of animation,
    it is the same one dropped back to the top of its stride -- a visible hitch
    on a control that is held and let go of constantly.
    """
    run = Run()
    for _ in range(60):
        run.tick(forward=1.0)            # up to a run
    before = A.action_anim(run.hero)

    clips, resets = set(), 0
    run.hero.anim_reset = False
    for frame in range(60):
        # Read and cleared the way `Game._update_animation` reads and clears
        # it, so what is counted is restarts asked for rather than one flag
        # left standing since an earlier transition.
        run.tick(forward=1.0, thrust=10 <= frame < 40)
        resets += run.hero.anim_reset
        run.hero.anim_reset = False
        clips.add(A.action_anim(run.hero))

    problems = []
    if clips != {A.RUN}:
        problems.append(f"the clip changed: {sorted(clips)}")
    if resets:
        problems.append(f"the stride was reset {resets} times")
    if run.hero.action_name != "walking":
        problems.append(f"ended in {run.hero.action_name}, not back in a run")

    return not problems, ("; ".join(problems) if problems else
                          f"{before!r} runs unbroken through the trigger going "
                          "down and coming back up")


def check_a_takes_off_from_the_skates():
    """A out of a skate is the take-off, and there is no jump in front of it.

    `ACT_HERO_JUMP` never being entered is the whole of it, and it is not a
    detail of bookkeeping: the jump carries its own take-off arc, its own
    button-governed height and a landing at the end of it, and none of those
    belong in front of a flight. What he draws going up is the flight's pose
    rather than the jump's, which is the same clip -- there being no flying
    clip in the set -- and reached from a different action.
    """
    run = Run()
    run.tick(thrust=True)
    skate_pose = A.action_anim(run.hero)

    actions = []
    for frame in range(120):
        run.tick(thrust=True, buttons=C.A_BUTTON if frame == 4 else 0)
        name = run.hero.action_name
        if not actions or actions[-1] != name:
            actions.append(name)

    problems = []
    if actions != ["skating", "jetpack"]:
        problems.append(f"took off through {actions}, not skating -> jetpack")
    if skate_pose != A.RUN:
        problems.append(f"the skate drew {skate_pose!r}, not the run")
    if run.height < 800.0:
        problems.append(f"only climbed {run.height:.0f} units")

    return not problems, ("; ".join(problems) if problems else
                          f"{skate_pose!r} -> {A.action_anim(run.hero)!r}, "
                          f"skating -> jetpack, {run.height:.0f} units up")


def check_jetpack():
    """The trigger flies, letting go falls, and A alone is only ever a jump."""
    def fly(hold_for, thrust=True):
        run = Run()
        airborne, best, actions = 0, 0.0, []
        for frame in range(120):
            run.tick(thrust=thrust and frame <= hold_for,
                     buttons=C.A_BUTTON if frame == 1 else 0)
            if not run.grounded:
                airborne += 1
            best = max(best, run.height)
            name = run.hero.action_name
            if not actions or actions[-1] != name:
                actions.append(name)
        return best, actions

    flying, actions = fly(120)
    fell, _ = fly(20)
    jumped, jump_actions = fly(0, thrust=False)

    problems = []
    if "jetpack" not in actions:
        problems.append(f"holding never reached the jetpack: {actions}")
    if flying < 800.0:
        problems.append(f"holding only climbed {flying:.0f} units")
    if not fell < flying / 2.0:
        problems.append(f"letting go still climbed to {fell:.0f}")
    if "jetpack" in jump_actions:
        problems.append(f"A alone lit the boosters: {jump_actions}")

    return not problems, ("; ".join(problems) if problems else
                          f"held climbs {flying:.0f} units, released stops at "
                          f"{fell:.0f}, A alone jumps {jumped:.0f}")


def check_a_hill_does_not_loop_the_landing():
    """Thrusting up a slope must not bounce between the boosters and a landing.

    The bug this stands against: the boosters used to lift him at 8 units a
    frame, which is less than a hill lifts the floor under a 38 unit run, so
    the air step landed him on the frame it started. That played the landing
    pose, the trigger was still down, and the landing handed him straight back
    to the boosters -- a landing animation on a loop and no flight at all.

    The floor is not the enemy now: touching it under thrust is a skate.
    """
    run = Run(pos=(0.0, 10.0, -1500.0), surfaces=ground_with_a_hill())
    for _ in range(40):
        run.tick(forward=1.0)            # get up to speed, onto the hill

    actions, landings = [], 0
    started = run.height
    for _ in range(120):
        run.tick(forward=1.0, thrust=True)
        name = run.hero.action_name
        if not actions or actions[-1] != name:
            actions.append(name)
            landings += name == "land"

    problems = []
    if landings:
        problems.append(f"the landing played {landings} times: {actions}")
    if run.height - started < 500.0:
        problems.append(f"he only got {run.height - started:.0f} units "
                        "up the hill")

    return not problems, ("; ".join(problems) if problems else
                          f"{' -> '.join(actions)} up the hill, "
                          f"{run.height - started:.0f} units gained")


def check_attack_chain():
    """B swings, B again inside the window buys the second swing, then it ends."""
    run = Run()
    run.tick(buttons=C.B_BUTTON)
    first = run.hero.action
    for _ in range(H.COMBO_WINDOW_START + 1):
        run.tick()
    run.tick(buttons=C.B_BUTTON)
    chained = run.hero.combo_index
    clip = A.action_anim(run.hero)
    for _ in range(200):
        run.tick()
    ok = (first == H.ACT_HERO_ATTACK and chained == 1 and clip == A.ATTACK2
          and run.hero.action == H.ACT_HERO_IDLE)
    return ok, (f"attack -> {clip!r} -> {run.hero.action_name} "
                f"(combo_index {chained})")


def check_spin_kick():
    """At speed, B spins instead of swinging."""
    run = Run()
    for _ in range(60):
        run.tick(forward=1.0)
    run.tick(forward=1.0, buttons=C.B_BUTTON)
    ok = run.hero.action == H.ACT_HERO_SPIN_KICK
    return ok, f"running B gives {run.hero.action_name!r}"


def check_every_action_has_a_clip():
    """No action may resolve to a clip the .glb does not contain."""
    import json
    with open(CLIPS, "r", encoding="utf-8") as fh:
        available = set(json.load(fh))

    run = Run()
    missing = []
    for act in H.ACTION_NAMES:
        run.hero.action = act
        run.hero.action_timer = 0
        run.hero.action_arg = 0
        for arg in (0, 1):
            run.hero.action_arg = arg
            clip, _, _ = A.resolve(run.hero)
            if clip not in available:
                missing.append((H.ACTION_NAMES[act], clip))
    # The idle fidget only appears after a long wait, so it needs asking for.
    run.hero.action = H.ACT_HERO_IDLE
    run.hero.action_timer = A.IDLE_VAR_AFTER + 1
    clip, _, _ = A.resolve(run.hero)
    if clip not in available:
        missing.append(("idle (fidget)", clip))

    return not missing, (f"{len(H.ACTION_NAMES)} actions resolve into "
                         f"{len(available)} clips"
                         + (f"; missing {missing}" if missing else ""))


def check_stride_matches_ground_speed():
    """One walk cycle per stride covered, up to the play-rate cap.

    Recomputed from the .glb rather than trusted, because the divisors in
    animations.py are measurements of these clips and a re-export can change
    them. Measuring the planted foot's travel relative to the spine gives the
    stride; the divisor that keeps it honest is stride / (30 * duration).

    This checks the walk divisor against the authored clip. The run deliberately
    stays at its authored 1.0 playback rate instead of matching ground speed.
    """
    sys.path.insert(0, HERE)
    import rig                                          # noqa: PLC0415

    gltf = rig.Gltf(os.path.join(ROOT, "assets", "hero", "hero.glb"))
    index = {n.get("name"): i for i, n in enumerate(gltf.json["nodes"])}

    from app.main import HERO_SCALE                     # noqa: PLC0415

    problems = []
    for clip, divisor in A.SPEED_SCALED.items():
        tracks = gltf.tracks(clip)
        frames = max(int(round(rig.clip_length(tracks) * 30.0)), 1)
        travel = []
        for foot in ("DEF-foot.L", "DEF-foot.R"):
            offsets = []
            for f in range(frames):
                world = gltf.world(rig.pose_at(gltf, tracks, f / 30.0))
                offsets.append(world[index[foot]][2, 3]
                               - world[index["DEF-spine"]][2, 3])
            travel.append(max(offsets) - min(offsets))
        stride = 2.0 * (sum(travel) / len(travel)) * HERO_SCALE
        wanted = stride / (30.0 * (frames / 30.0))
        if abs(wanted - divisor) > 0.35:
            problems.append(f"{clip!r} wants {wanted:.2f}, has {divisor:.2f}")

    return not problems, ("; ".join(problems) if problems
                          else "walk cycle matches the ground covered")


def check_the_aim_pivot_carries_the_right_half():
    """AIM_TORSO must hold every joint above the hips and none below.

    The one thing about the restructure that a screenshot cannot show and that
    nothing else would catch: a re-export growing a joint, or tools/aim_rig.py
    being given a name it does not recognise, leaves a bone in the half of the
    body it does not belong to -- an arm that stops following the aim, or a
    thigh that starts. Both look like an animation bug and neither is.
    """
    sys.path.insert(0, HERE)
    import aim_rig                                       # noqa: PLC0415
    import rig                                           # noqa: PLC0415

    gltf = rig.Gltf(os.path.join(ROOT, "assets", "hero", "hero.glb"))
    for name in (aim_rig.PIVOT, aim_rig.SOCKET):
        if name not in gltf.index:
            return False, (f"no {name} joint; run "
                           "python3 tools/aim_rig.py assets/hero/hero.glb")

    pivot = gltf.index[aim_rig.PIVOT]
    below = set()

    def collect(node):
        below.add(gltf.nodes[node].get("name"))
        for child in gltf.nodes[node].get("children", []):
            collect(child)

    collect(pivot)

    problems = []
    for name in ("DEF-spine.006", "Head", "DEF-hand.L", "DEF-hand.R",
                 "fingers.r", aim_rig.SOCKET):
        if name not in below:
            problems.append(f"{name} does not turn with the aim")
    for name in ("DEF-thigh.L", "DEF-thigh.R", "DEF-foot.L", "DEF-toe.R",
                 "Belt", "Sash.00"):
        if name in below:
            problems.append(f"{name} turns with the aim and should not")
    if gltf.index[aim_rig.SOCKET] not in gltf.json["skins"][0]["joints"]:
        problems.append(f"{aim_rig.SOCKET} is not a skin joint, so Panda3D "
                        "will not expose it")

    return not problems, ("; ".join(problems) if problems else
                          f"{len(below) - 1} joints ride the pivot, the legs "
                          "do not")


def check_the_torso_stops_at_its_limit():
    """The twist clamps, and the feet are asked for exactly the excess."""
    from sm64py.aim import AimController                 # noqa: PLC0415
    from sm64py.math_util import degrees_to_s16, s16_to_degrees  # noqa: PLC0415

    class Pivot:
        """Stands in for the joint. It is what the controller writes to, and
        having one is what tells it there is a torso to twist at all."""

        def set_hpr(self, *hpr):
            self.hpr = hpr

    aim = AimController(Pivot())
    aim.set_tracking(1.0)
    limit = aim.profile.yaw_limit

    problems = []
    # Aim 100 degrees off his facing, from three different facings: the answer
    # is local, so it must not depend on which way he is standing.
    for facing in (0.0, 90.0, -140.0):
        yaw = math.radians(facing + 100.0)
        aim.set_aim_direction((math.sin(yaw), 0.0, math.cos(yaw)),
                              degrees_to_s16(facing))
        if abs(aim.target_yaw - 100.0) > 0.5:
            problems.append(f"facing {facing:.0f}, a 100 degree aim reads as "
                            f"{aim.target_yaw:.1f}")
        if abs(aim.local_yaw - limit) > 1e-6:
            problems.append(f"the torso twists to {aim.local_yaw:.1f}, past "
                            f"its {limit:.0f} degree limit")

    # Standing, the feet come round for the excess; running, the torso is left
    # to cover more of it before they are asked.
    standing = s16_to_degrees(aim.body_turn(1.0 / 30.0, moving=False))
    running = s16_to_degrees(aim.body_turn(1.0 / 30.0, moving=True))
    if not standing > running > 0.0:
        problems.append(f"a 100 degree aim turns him {standing:.1f} standing "
                        f"and {running:.1f} running; both should be positive "
                        "and standing should be the larger")

    # Inside the torso's reach his feet stay where they are.
    yaw = math.radians(aim.profile.yaw_limit - 5.0)
    aim.set_aim_direction((math.sin(yaw), 0.0, math.cos(yaw)), 0)
    if aim.body_turn(1.0 / 30.0, moving=True) != 0:
        problems.append("his feet turn for an aim the torso could have covered")

    # And a character with no pivot -- Mario -- turns for all of it.
    bare = AimController(None)
    bare.set_tracking(1.0)
    bare.set_aim_direction((math.sin(yaw), 0.0, math.cos(yaw)), 0)
    if bare.body_turn(1.0 / 30.0, moving=True) <= 0:
        problems.append("a skeleton with no pivot does not turn on its feet")

    # And an attack lets go of the aim as it commits.
    from sm64py.aim import melee_tracking                # noqa: PLC0415
    curve = [melee_tracking(t) for t in (0.0, 0.3, 0.6, 0.9)]
    if not all(a >= b for a, b in zip(curve, curve[1:])) or curve[-1] != 0.0:
        problems.append(f"melee tracking does not commit: {curve}")

    return not problems, ("; ".join(problems) if problems else
                          f"clamps at {limit:.0f} degrees, then turns his feet")


CHECKS = [
    ("walking reaches a run", check_walking),
    ("stride matches ground speed", check_stride_matches_ground_speed),
    ("stopping returns to idle", check_stop),
    ("jump leaves and regains the floor", check_jump),
    ("jump height follows the button", check_jump_height_is_variable),
    ("the jetpack flies, and only when held", check_jetpack),
    ("the trigger skates him", check_the_trigger_skates_him),
    ("the stride survives the trigger", check_the_stride_survives_the_trigger),
    ("A takes off from the skates", check_a_takes_off_from_the_skates),
    ("a hill does not loop the landing", check_a_hill_does_not_loop_the_landing),
    ("attack chains into the second swing", check_attack_chain),
    ("spin kick out of a run", check_spin_kick),
    ("every action has a clip", check_every_action_has_a_clip),
    ("the aim pivot carries the right half", check_the_aim_pivot_carries_the_right_half),
    ("the torso stops at its limit", check_the_torso_stops_at_its_limit),
]


def main():
    A.load_clip_metadata(CLIPS)
    failures = 0
    for name, check in CHECKS:
        try:
            ok, detail = check()
        except Exception as exc:                       # noqa: BLE001
            ok, detail = False, f"raised {type(exc).__name__}: {exc}"
        print(f"  [{'ok' if ok else 'FAIL'}] {name}: {detail}")
        failures += not ok
    print(f"{len(CHECKS) - failures}/{len(CHECKS)} checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
