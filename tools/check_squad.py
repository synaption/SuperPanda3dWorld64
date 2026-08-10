"""Exercise the squad commands with no window and no level.

What is worth checking about `sm64py/squad.py` is everything the player can
only judge by eye at 60 Hz: whether the aim lands in front of him rather than
behind, whether tilting the view further up really does throw the spot further
out, whether the circle catches the allies it is drawn around and nobody else,
and whether an ally told to follow actually arrives. None of that needs Panda3D
or the castle grounds, so this stands a flat plane in for the level and a pair
of coordinates in for the camera, and runs the same code the game runs.

What it cannot check is how any of it feels, or that the reticle is drawn where
the numbers say it is -- that is the front end's half, and it wants eyes.

    python3 tools/check_squad.py
"""

import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py import objects, squad  # noqa: E402
from sm64py.surfaces import SurfaceSet  # noqa: E402

# Comfortably inside the s16 range collision samples truncate to.
PLANE = 30000


def flat_ground():
    """A floor big enough to aim across, wound so it faces upward."""
    vertices = np.array([[-PLANE, 0, -PLANE], [PLANE, 0, -PLANE],
                         [PLANE, 0, PLANE], [-PLANE, 0, PLANE]], dtype=np.int32)
    triangles = np.array([[0, 2, 1], [0, 3, 2]], dtype=np.int32)
    zeros = np.zeros(2, dtype=np.int32)
    return SurfaceSet(vertices, triangles, zeros, zeros, None)


class FakeCamera:
    """The two things `aim_point` reads off the real camera.

    Placed behind the player at (0, 0, 0) looking at him, which is the only
    arrangement `FollowCamera` ever produces: the higher it sits for a given
    distance back, the steeper the view down and the nearer the ground the ray
    through him meets.
    """

    def __init__(self, height, distance=1200.0):
        self.pos = [0.0, height, -distance]
        self.focus = [0.0, 60.0, 0.0]


class FakeLeader:
    """A stand-in for whoever is being played, as the squad reads him."""

    def __init__(self, x=0.0, y=0.0, z=0.0, yaw=0):
        self.pos = [x, y, z]
        self.face_angle = [0, yaw, 0]


def a_squad(ally_positions, leader=None):
    """A world with allies at the given spots, and a squad over it."""
    ground = flat_ground()
    world = objects.ObjectSet(ground)
    allies = [world.spawn(objects.Mario, x, 0.0, z) for x, z in ally_positions]
    return world, allies, squad.Squad(world, objects.Mario), \
        leader if leader is not None else FakeLeader()


def run(world, group, leader, ticks):
    """Step the simulation the way app/main.py's tick loop does."""
    for _ in range(ticks):
        group.update(leader)
        world.update(leader)


def run_until(world, group, leader, done, limit):
    """Step until something is true, and report how long it took."""
    for ticks in range(limit):
        if done():
            return ticks
        group.update(leader)
        world.update(leader)
    return limit


# -- aiming -----------------------------------------------------------------


def check_aim_lands_in_front():
    """The spot is ahead of the player, on the floor, inside the range."""
    ground = flat_ground()
    point = squad.aim_point(ground, FakeCamera(500.0), [0.0, 0.0, 0.0])
    distance = math.hypot(point[0], point[2])
    ok = (point[2] > 0.0                      # the camera is at -Z, so ahead is +Z
          and abs(point[0]) < 1.0             # dead ahead, not off to a side
          and abs(point[1]) < 1.0             # on the plane
          and squad.AIM_MIN_RANGE <= distance <= squad.AIM_MAX_RANGE)
    return ok, f"{distance:.0f} units ahead, y {point[1]:.1f}"


def check_tilting_up_throws_it_further():
    """A shallower view reaches further out; a steeper one pulls in."""
    ground = flat_ground()
    distances = []
    for height in (700.0, 500.0, 300.0, 200.0):
        point = squad.aim_point(ground, FakeCamera(height), [0.0, 0.0, 0.0])
        distances.append(math.hypot(point[0], point[2]))
    ok = all(a < b for a, b in zip(distances, distances[1:]))
    return ok, "  ".join(f"{d:.0f}" for d in distances)


def check_looking_down_stops_at_his_feet():
    """The camera at its highest aims just past him, not behind him.

    `FollowCamera` will pitch up far enough that the ray through his back
    reaches the ground within a few dozen units of it, which is nowhere to send
    anybody; AIM_MIN_RANGE is what turns that into a spot in front of him.
    """
    ground = flat_ground()
    point = squad.aim_point(ground, FakeCamera(1600.0), [0.0, 0.0, 0.0])
    distance = math.hypot(point[0], point[2])
    ok = abs(distance - squad.AIM_MIN_RANGE) < 1.0 and point[2] > 0.0
    return ok, f"{distance:.0f} units, floor {squad.AIM_MIN_RANGE:.0f}"


def check_the_range_is_capped():
    """A view at the horizon lands at the cap rather than at infinity."""
    ground = flat_ground()
    # Level with the focus: the ray never descends, so it never meets ground.
    camera = FakeCamera(60.0)
    point = squad.aim_point(ground, camera, [0.0, 0.0, 0.0])
    distance = math.hypot(point[0], point[2])
    ok = abs(distance - squad.AIM_MAX_RANGE) < 1.0 and abs(point[1]) < 1.0
    return ok, f"{distance:.0f} units, cap {squad.AIM_MAX_RANGE:.0f}"


def check_aim_follows_the_camera_around():
    """Swing the camera and the spot swings with it, staying ahead."""
    ground = flat_ground()
    camera = FakeCamera(500.0)
    # The same camera, moved a quarter turn around the player.
    camera.pos = [-1200.0, 500.0, 0.0]
    point = squad.aim_point(ground, camera, [0.0, 0.0, 0.0])
    ok = point[0] > 0.0 and abs(point[2]) < 1.0
    return ok, f"({point[0]:.0f}, {point[2]:.0f})"


def check_the_arc_connects_its_ends():
    """The lob leaves the hand, lands on the spot, and goes over the top."""
    start = (0.0, 110.0, 0.0)
    end = (0.0, 0.0, 1500.0)
    points = squad.throw_arc(start, end)
    apex = max(p[1] for p in points)
    chord = max(start[1], end[1])
    ok = (max(abs(points[0][i] - start[i]) for i in range(3)) < 0.01
          and max(abs(points[-1][i] - end[i]) for i in range(3)) < 0.01
          and apex > chord + 100.0)
    return ok, f"peaks {apex:.0f} above ground, ends {points[-1][1]:.1f}"


def check_the_circle_grows_and_stops():
    """It opens at the minimum, reaches the maximum, and stays there."""
    opening = squad.circle_radius(0.0)
    grown = squad.circle_radius(squad.CIRCLE_GROW_SECONDS)
    over = squad.circle_radius(squad.CIRCLE_GROW_SECONDS * 4.0)
    ok = (abs(opening - squad.CIRCLE_MIN_RADIUS) < 0.01
          and abs(grown - squad.CIRCLE_MAX_RADIUS) < 0.01
          and abs(over - squad.CIRCLE_MAX_RADIUS) < 0.01)
    return ok, f"{opening:.0f} -> {grown:.0f}, held {over:.0f}"


# -- the squad --------------------------------------------------------------


def check_the_whistle_takes_only_the_circle():
    """Allies inside join; the one outside carries on wandering."""
    world, allies, group, leader = a_squad(
        [(0.0, 2000.0), (300.0, 2000.0), (0.0, 4000.0)])
    joined = group.recruit((0.0, 0.0, 2000.0), 500.0)
    ok = (joined == 2 and len(group.members) == 2
          and allies[2] not in group.members)
    return ok, f"{joined} of {len(allies)} joined"

def check_a_whistled_ally_follows():
    """Called from across the field, he ends up behind the leader."""
    world, allies, group, leader = a_squad([(0.0, 2000.0)])
    group.recruit((0.0, 0.0, 2000.0), 500.0)
    run(world, group, leader, 200)
    ally = allies[0]
    distance = math.hypot(ally.pos[0] - leader.pos[0],
                          ally.pos[2] - leader.pos[2])
    # The leader faces +Z at yaw 0, so his squad gathers at -Z.
    ok = distance < squad.FOLLOW_DISTANCE + 200.0 and ally.pos[2] < 0.0
    return ok, f"{distance:.0f} units away, behind by {-ally.pos[2]:.0f}"


def check_the_squad_keeps_up():
    """The leader walks off and the group is still with him at the end."""
    world, allies, group, leader = a_squad([(0.0, 300.0), (200.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    run(world, group, leader, 60)
    for _ in range(200):
        # Twenty units a tick is a run; the Hero's cap is higher, but this is
        # what an ally has to be able to answer to be worth following anyone.
        leader.pos[2] += 20.0
        group.update(leader)
        world.update(leader)
    far = max(math.hypot(a.pos[0] - leader.pos[0], a.pos[2] - leader.pos[2])
              for a in allies)
    ok = far < 800.0
    return ok, f"furthest {far:.0f} units behind"


def check_sending_spreads_them_out():
    """Sent to one spot, they stand around it rather than inside each other."""
    world, allies, group, leader = a_squad(
        [(0.0, 300.0), (200.0, 300.0), (-200.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    sent = group.send((0.0, 0.0, 3000.0))
    ticks = run_until(world, group, leader,
                      lambda: group.holding == sent, limit=600)

    at_the_spot = [math.hypot(a.pos[0], a.pos[2] - 3000.0) for a in allies]
    gaps = [math.hypot(a.pos[0] - b.pos[0], a.pos[2] - b.pos[2])
            for i, a in enumerate(allies) for b in allies[i + 1:]]
    ok = (sent == 3 and not group.members and ticks < 600
          and max(at_the_spot) < 700.0 and min(gaps) > 60.0)
    return ok, (f"{sent} sent, there in {ticks} ticks, furthest "
                f"{max(at_the_spot):.0f} from the spot, closest pair "
                f"{min(gaps):.0f} apart")


def check_arriving_posts_them_there():
    """They hold the spot rather than wandering off it again.

    Which is the difference between sending a squad somewhere and shooing it:
    a Mario with no goal goes back to strolling about, and after ten seconds of
    that he is a thousand units from where he was put.
    """
    world, allies, group, leader = a_squad([(0.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    group.send((0.0, 0.0, 2000.0))
    run_until(world, group, leader, lambda: group.holding == 1, limit=600)
    arrival = list(allies[0].pos)
    run(world, group, leader, 300)
    drift = math.hypot(allies[0].pos[0] - arrival[0],
                       allies[0].pos[2] - arrival[2])
    ok = (group.holding == 1 and allies[0].goal is not None
          and drift < squad.SEND_ARRIVE)
    return ok, f"drifted {drift:.0f} units in ten seconds of standing there"


def check_a_whistle_overrides_an_order():
    """Called back mid-march, he turns round rather than finishing the trip."""
    world, allies, group, leader = a_squad([(0.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    group.send((0.0, 0.0, 6000.0))
    run(world, group, leader, 30)
    joined = group.recruit((allies[0].pos[0], 0.0, allies[0].pos[2]), 400.0)
    ok = joined == 1 and not group.sent and allies[0] in group.members
    return ok, f"{joined} called back, {len(group.sent)} still marching"


def check_orders_shorten_the_leash():
    """An ally in formation answers what is near, not what is far.

    The wandering Mario hunts anything inside HUNT_RANGE, which is most of the
    field; one under orders that did the same would leave the moment a pipe
    fired on the other side of the level.
    """
    world, allies, group, leader = a_squad([(0.0, 300.0)])
    ally = allies[0]
    far = (squad.FOLLOW_DISTANCE
           + (objects.Mario.HUNT_RANGE + objects.Mario.SQUAD_HUNT_RANGE) / 2.0)
    world.spawn(objects.Goomba, 0.0, 0.0, far)

    free_target = ally._acquire_target()
    group.recruit((0.0, 0.0, 300.0), 500.0)
    group.update(leader)
    held_target = ally._acquire_target()
    ok = free_target is not None and held_target is None
    return ok, f"a goomba {far:.0f} out: loose yes, in formation no"


def check_the_leash_is_held_from_the_spot():
    """Something beside him but far from his post is not his business.

    The leash is anchored to the goal rather than to his own feet, or a squad
    whistled up from across the field would fight its way home one enemy at a
    time instead of coming when it was called.
    """
    world, allies, group, leader = a_squad([(0.0, 4000.0)])
    ally = allies[0]
    world.spawn(objects.Goomba, 0.0, 0.0, 4000.0 + objects.Mario.STRIKE_RANGE)
    group.recruit((0.0, 0.0, 4000.0), 500.0)
    group.update(leader)
    ok = ally._acquire_target() is None
    return ok, "a goomba at arm's length, 4000 units from where he is wanted"


def check_disbanding_lets_everyone_go():
    """Swapping character hands them all back."""
    world, allies, group, leader = a_squad([(0.0, 300.0), (200.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    group.send((0.0, 0.0, 3000.0))
    group.disband()
    ok = (not group.members and not group.sent
          and all(a.goal is None for a in allies))
    return ok, "no members, no orders, no goals"


def check_the_fallen_are_dropped():
    """A defeated ally leaves the squad instead of being marched about."""
    world, allies, group, leader = a_squad([(0.0, 300.0), (200.0, 300.0)])
    group.recruit((0.0, 0.0, 300.0), 500.0)
    allies[0].defeat()
    group.update(leader)
    ok = len(group.members) == 1 and allies[0] not in group.members
    return ok, f"{len(group.members)} left following"


CHECKS = [
    ("the aim lands in front of him", check_aim_lands_in_front),
    ("tilting up throws it further", check_tilting_up_throws_it_further),
    ("looking down stops at his feet", check_looking_down_stops_at_his_feet),
    ("the range is capped", check_the_range_is_capped),
    ("the aim follows the camera round", check_aim_follows_the_camera_around),
    ("the arc connects its ends", check_the_arc_connects_its_ends),
    ("the circle grows and stops", check_the_circle_grows_and_stops),
    ("the whistle takes only the circle", check_the_whistle_takes_only_the_circle),
    ("a whistled ally follows", check_a_whistled_ally_follows),
    ("the squad keeps up", check_the_squad_keeps_up),
    ("sending spreads them out", check_sending_spreads_them_out),
    ("arriving posts them there", check_arriving_posts_them_there),
    ("a whistle overrides an order", check_a_whistle_overrides_an_order),
    ("orders shorten the leash", check_orders_shorten_the_leash),
    ("the leash is held from the spot", check_the_leash_is_held_from_the_spot),
    ("disbanding lets everyone go", check_disbanding_lets_everyone_go),
    ("the fallen are dropped", check_the_fallen_are_dropped),
]


def main():
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
    raise SystemExit(main())
