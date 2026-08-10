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


class Run:
    def __init__(self, pos=(0.0, 10.0, 0.0), yaw=0.0):
        self.surfaces = flat_ground()
        self.controller = Controller()
        self.hero = HeroState(self.surfaces, self.controller)
        self.hero.spawn(*pos, yaw)
        self.camera = FollowCamera(self.surfaces, self.hero)

    def tick(self, forward=0.0, buttons=0):
        # Negative stick_y is forward; app/main.py feeds both axes mirrored.
        self.controller.set_stick(0.0, -forward)
        self.controller.set_buttons(buttons)
        self.hero.camera_yaw = self.camera.mario_yaw
        execute_action(self.hero)
        self.camera.update(TICK_DT)
        return self.hero.action

    @property
    def height(self):
        return self.hero.pos[1]

    @property
    def grounded(self):
        return self.hero.pos[1] <= 0.01


def check_walking():
    run = Run()
    speeds = []
    for _ in range(200):
        run.tick(forward=1.0)
        speeds.append(run.hero.forward_vel)
    tail = speeds[100:]
    settled = sum(tail) / len(tail)
    clip = A.action_anim(run.hero)
    ok = (H.RUN_SPEED < settled <= H.MAX_RUN_SPEED
          and run.hero.action == H.ACT_HERO_WALKING and clip == A.RUN)
    return ok, (f"settles at {settled:.1f} u/frame "
                f"(max {H.MAX_RUN_SPEED}), playing {clip!r}")


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

    "Held" means held to the last frame before the jetpack would take over,
    not held indefinitely: holding past `JETPACK_DELAY` is no longer a jump at
    all, it is flight, and measuring that here would report a number in the
    thousands and stop saying anything about the jump.
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

    held, tapped = peak(H.JETPACK_DELAY - 1), peak(0)
    return held > tapped + 20.0, f"held {held:.0f} vs tapped {tapped:.0f} units"


def check_jetpack():
    """Holding the button flies, letting go falls, and a tap does neither."""
    def fly(hold_for):
        run = Run()
        airborne, best, actions = 0, 0.0, []
        for _ in range(120):
            run.tick(buttons=C.A_BUTTON if airborne <= hold_for else 0)
            if not run.grounded:
                airborne += 1
            best = max(best, run.height)
            name = run.hero.action_name
            if not actions or actions[-1] != name:
                actions.append(name)
        return best, actions

    flying, actions = fly(120)
    fell, _ = fly(H.JETPACK_DELAY + 10)
    tapped, tap_actions = fly(0)

    problems = []
    if "jetpack" not in actions:
        problems.append(f"holding never reached the jetpack: {actions}")
    if flying < 800.0:
        problems.append(f"holding only climbed {flying:.0f} units")
    if not fell < flying / 2.0:
        problems.append(f"letting go still climbed to {fell:.0f}")
    if "jetpack" in tap_actions:
        problems.append("a tap lit the boosters")

    return not problems, ("; ".join(problems) if problems else
                          f"held climbs {flying:.0f} units, released stops at "
                          f"{fell:.0f}, a tap peaks at {tapped:.0f}")


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
    """One clip cycle per stride covered, up to the play-rate cap.

    Recomputed from the .glb rather than trusted, because the divisors in
    animations.py are measurements of these clips and a re-export can change
    them. Measuring the planted foot's travel relative to the spine gives the
    stride; the divisor that keeps it honest is stride / (30 * duration).

    This checks the divisors, not what is finally played: above
    MAX_PLAY_RATE the feet slide on purpose, because a human stride at Mario's
    top speed needs a cadence that reads as whirring. Below it -- all of
    walking, and running up to about half speed -- the contact is exact.
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
                          else "walk and run cycles match the ground covered")


CHECKS = [
    ("walking reaches a run", check_walking),
    ("stride matches ground speed", check_stride_matches_ground_speed),
    ("stopping returns to idle", check_stop),
    ("jump leaves and regains the floor", check_jump),
    ("jump height follows the button", check_jump_height_is_variable),
    ("the jetpack flies, and only when held", check_jetpack),
    ("attack chains into the second swing", check_attack_chain),
    ("spin kick out of a run", check_spin_kick),
    ("every action has a clip", check_every_action_has_a_clip),
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
