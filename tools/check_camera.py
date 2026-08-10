"""Exercise the shooter camera with no window and no level.

Most of what a camera is judged on is felt rather than measured, and none of
that is in here. What *is* in here is the half that has a number attached, and
it is the half that goes wrong quietly:

  * that a look input lands on the frame it arrives and in full, which is the
    single thing the old follow camera got wrong and the reason this one exists;
  * that the same hand movement turns the view by the same angle whatever the
    frame rate is, for both the mouse and the stick, since a camera tuned at
    60 fps that oversteers at 144 is a camera tuned for one machine;
  * that everything which *is* smoothed settles rather than ringing, and lands
    in the same place from either direction;
  * that the boom comes in the instant something is in the way and goes back
    out slowly, which is the asymmetry the whole occlusion behaviour rests on;
  * that the aim ray really is the middle of the screen -- that the direction
    the view was built from and the direction gameplay is told to aim along are
    the same direction, to the last decimal.

A flat plane and a single wall stand in for the level; a stub with a position,
a facing and a speed stands in for the character.

    python3 tools/check_camera.py
"""

import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from sm64py import camera as cam  # noqa: E402
from sm64py.camera import FollowCamera  # noqa: E402
from sm64py.math_util import s16, s16_to_degrees  # noqa: E402
from sm64py.surfaces import SurfaceSet  # noqa: E402

PLANE = 30000


def flat_ground(extra_vertices=(), extra_triangles=()):
    """A floor, wound so it faces upward, with anything else asked for on it."""
    vertices = [[-PLANE, 0, -PLANE], [PLANE, 0, -PLANE],
                [PLANE, 0, PLANE], [-PLANE, 0, PLANE]]
    triangles = [[0, 2, 1], [0, 3, 2]]
    vertices.extend(extra_vertices)
    triangles.extend(extra_triangles)
    count = len(triangles)
    return SurfaceSet(np.array(vertices, dtype=np.int32),
                      np.array(triangles, dtype=np.int32),
                      np.zeros(count, dtype=np.int32),
                      np.zeros(count, dtype=np.int32), None)


WALL_Z = 450


def ground_with_a_wall():
    """The same floor with a wall across +Z, well inside the boom's length.

    Two triangles, tall enough that the camera cannot go over them and wide
    enough that it cannot go round.
    """
    return flat_ground(
        extra_vertices=([-4000, 0, WALL_Z], [4000, 0, WALL_Z],
                        [4000, 3000, WALL_Z], [-4000, 3000, WALL_Z]),
        extra_triangles=([4, 5, 6], [4, 6, 7]),
    )


class Stub:
    """What the camera reads off the character, and nothing else."""

    def __init__(self, x=0.0, y=0.0, z=0.0, yaw=0, speed=0.0):
        self.pos = [x, y, z]
        self.face_angle = [0, s16(yaw), 0]
        self.forward_vel = speed
        self.floor_height = 0.0


def run(camera, seconds, dt=1.0 / 60.0, before=None):
    """Step the camera for a while, optionally doing something each frame."""
    for step in range(int(round(seconds / dt))):
        if before is not None:
            before(step * dt)
        camera.update(dt)
    return camera


def settled(camera, hero=None, seconds=2.0):
    """Let everything that eases finish easing."""
    return run(camera, seconds)


def a_camera(surfaces=None, **kwargs):
    hero = Stub(**kwargs)
    return FollowCamera(surfaces if surfaces is not None else flat_ground(),
                        hero), hero


# -- the one that matters ----------------------------------------------------


def check_look_lands_this_frame():
    """A look input is in the view immediately, not eventually.

    The old camera eased toward a target yaw at eight per second, which put
    two thirds of a turn in on the first frame at 60 fps and the rest over the
    following quarter second. This is the check that fails if anything ever
    quietly puts a spring back between the hand and the angle.
    """
    camera, hero = a_camera()
    settled(camera)
    before = camera.yaw
    camera.look(30.0, 0.0)
    turned = s16_to_degrees(cam._wrap_angle(before - camera.yaw))
    # And it must still be there after the frame is stepped, rather than being
    # something `update` eases away from.
    camera.update(1.0 / 60.0)
    kept = s16_to_degrees(cam._wrap_angle(before - camera.yaw))
    ok = abs(turned - 30.0) < 0.01 and abs(kept - 30.0) < 0.01
    return ok, f"asked 30 deg, got {turned:.2f} on the spot, {kept:.2f} after the frame"


def check_look_survives_the_frame_rate():
    """The same mouse movement turns the same angle at 30 and at 240 fps.

    The mouse reports movement rather than a held position, so its delta must
    *not* be scaled by dt -- and the smoothing that spreads it over a couple of
    frames must not lose or duplicate any of it either.
    """
    turned = []
    for dt in (1.0 / 30.0, 1.0 / 240.0):
        camera, hero = a_camera()
        settled(camera)
        before = camera.yaw
        for _ in range(12):
            camera.look_mouse(50.0, 0.0)
            camera.update(dt)
        run(camera, 0.5, dt)          # let the smoothing pay out the rest
        turned.append(s16_to_degrees(cam._wrap_angle(before - camera.yaw)))
    ok = abs(turned[0] - turned[1]) < 0.05
    return ok, f"{turned[0]:.2f} deg at 30 fps, {turned[1]:.2f} at 240"


def check_stick_survives_the_frame_rate():
    """And the same stick push held the same *time* turns the same angle.

    The opposite case to the mouse: a stick reports a position that is held, so
    it is the one that has to be scaled by dt.
    """
    turned = []
    for dt in (1.0 / 30.0, 1.0 / 240.0):
        camera, hero = a_camera()
        settled(camera)
        before = camera.yaw
        run(camera, 1.0, dt, before=lambda t: camera.look_stick(1.0, 0.0, dt))
        turned.append(s16_to_degrees(cam._wrap_angle(before - camera.yaw)))
    ok = abs(turned[0] - turned[1]) / max(abs(turned[0]), 1.0) < 0.02
    return ok, f"{turned[0]:.1f} deg at 30 fps, {turned[1]:.1f} at 240"


def check_the_stick_ramps():
    """Held at the rim, the stick turns faster than it starts out doing.

    A stick that turns at one rate can either flick round quickly or track
    something slowly, and a shooter needs both out of the same thumb.
    """
    camera, hero = a_camera()
    settled(camera)
    dt = 1.0 / 60.0

    def turned_over(start, end):
        first = camera.yaw
        for _ in range(int((end - start) / dt)):
            camera.look_stick(1.0, 0.0, dt)
            camera.update(dt)
        return abs(s16_to_degrees(cam._wrap_angle(first - camera.yaw)))

    early = turned_over(0.0, 0.2)
    late = turned_over(0.2, 0.4)      # by now the ramp is up
    ok = late > early * 1.15
    return ok, (f"{early:.1f} deg in the first fifth of a second, "
                f"{late:.1f} in the third")


def check_the_curve_leaves_a_slow_turn():
    """Half a stick is much less than half the turn rate.

    The whole point of the response curve: the middle of the stick's travel is
    where a player tracks something, and it has to be usably slow there.
    """
    camera, hero = a_camera()
    dt = 1.0 / 60.0
    rates = []
    for push in (0.5, 1.0):
        camera, hero = a_camera()
        settled(camera)
        before = camera.yaw
        # One frame only, so the ramp has not moved and only the curve is
        # being measured.
        camera.look_stick(push, 0.0, dt)
        rates.append(abs(s16_to_degrees(cam._wrap_angle(before - camera.yaw))))
    share = rates[0] / rates[1]
    ok = 0.2 < share < 0.35
    return ok, f"half a push gives {share * 100:.0f}% of the rate, not 50%"


# -- following ---------------------------------------------------------------


def check_the_camera_settles_behind_him():
    """Left alone, the camera ends up one boom behind him at his own height."""
    camera, hero = a_camera(yaw=0)
    settled(camera)
    # He faces +Z, so the camera sits at -Z of him.
    back = camera.pos[2] - hero.pos[2]
    flat = math.hypot(camera.pos[0] - hero.pos[0], camera.pos[2] - hero.pos[2])
    ok = back < 0 and abs(flat - cam.HIP_DISTANCE * math.cos(camera.pitch)) < 60.0
    return ok, f"{-back:.0f} units behind him, {flat:.0f} out in the plane"


def check_following_does_not_overshoot():
    """He walks; the camera follows and never passes the distance it holds.

    A spring that overshoots reads as the camera bouncing off him, and it is
    the first thing a badly chosen damping shows up as.
    """
    camera, hero = a_camera()
    settled(camera)
    dt = 1.0 / 60.0
    worst = 0.0

    def walk(_t):
        hero.pos[2] += 38.0 * dt * 30.0 / 30.0   # a run, in units per second
        hero.forward_vel = 38.0

    for _ in range(120):
        walk(0.0)
        camera.update(dt)
        gap = hero.pos[2] - camera.pos[2]
        worst = max(worst, gap)

    # He is walking away from a camera that is behind him, so the gap grows to
    # a steady trail and must not oscillate around it.
    tail = []
    for _ in range(60):
        walk(0.0)
        camera.update(dt)
        tail.append(hero.pos[2] - camera.pos[2])
    swing = max(tail) - min(tail)
    ok = swing < 8.0
    return ok, f"trail steady to within {swing:.1f} units over the last second"


def check_small_steps_do_not_move_the_camera():
    """A kerb-sized change in his height leaves the view alone.

    The dead band. Without it, every step up the castle path pumps the horizon.
    """
    camera, hero = a_camera()
    settled(camera)
    height = camera.pos[1]
    hero.pos[1] = hero.floor_height = 40.0       # a step, not a storey
    run(camera, 1.0)
    moved = abs(camera.pos[1] - height)
    ok = moved < 8.0
    return ok, f"a 40-unit step moved the camera {moved:.1f} units"


def check_big_climbs_are_followed():
    """A jetpack climb does not leave him off the bottom of the screen."""
    camera, hero = a_camera()
    settled(camera)
    dt = 1.0 / 60.0
    for _ in range(240):
        hero.pos[1] += 30.0
        camera.update(dt)
    lag = (hero.pos[1] + camera.height) - camera._pivot[1]
    ok = lag <= cam.MAX_VERTICAL_LAG + 1.0
    return ok, (f"climbed {hero.pos[1]:.0f} units, camera {lag:.0f} behind "
                f"(leash {cam.MAX_VERTICAL_LAG:.0f})")


# -- the boom ----------------------------------------------------------------


def check_the_boom_comes_in_at_once():
    """Swinging the camera into a wall shortens the boom on that same frame.

    Not over the following tenth of a second: those would be frames spent
    inside the wall, which is the one place a camera may never be. The view is
    turned rather than the character moved because a turn is instantaneous --
    the camera is asked for a position beyond the wall between one frame and
    the next, with nothing easing into it.
    """
    camera, hero = a_camera(ground_with_a_wall(), yaw=0)
    settled(camera)             # facing +Z: the camera is at -Z, in the open
    camera.look(180.0, 0.0)     # now it wants to be at +Z, past the wall
    camera.update(1.0 / 60.0)
    ok = camera.pos[2] < WALL_Z
    return ok, (f"wall at z={WALL_Z}, camera at z={camera.pos[2]:.0f} "
                f"one frame after swinging into it")


def check_the_boom_goes_out_slowly():
    """And it grows back over a third of a second rather than snapping."""
    camera, hero = a_camera(ground_with_a_wall(), yaw=0x8000)
    settled(camera)
    short = camera._boom
    # Turn away from the wall: the boom now wants its full length back.
    camera.look(180.0, 0.0)
    camera.update(1.0 / 60.0)
    after_one_frame = camera._boom
    run(camera, 1.0)
    recovered = camera._boom
    share = (after_one_frame - short) / max(recovered - short, 1e-6)
    ok = share < 0.25 and recovered > short * 1.5
    return ok, (f"{short:.0f} at the wall, {after_one_frame:.0f} one frame later, "
                f"{recovered:.0f} a second later")


def check_the_camera_stays_above_the_ground():
    """Look down as far as it goes and the camera is still out of the floor."""
    camera, hero = a_camera()
    settled(camera)
    camera.look(0.0, -90.0)          # all the way down, clamped by the limit
    run(camera, 1.0)
    ok = camera.pos[1] > 0.0
    return ok, f"floor at 0, camera at {camera.pos[1]:.0f}"


# -- the sights --------------------------------------------------------------


def check_the_sights_pull_in_and_let_go():
    """Aiming closes the distance and narrows the view, and releasing undoes it."""
    camera, hero = a_camera()
    settled(camera)
    wide, far = camera.fov, camera._boom

    camera.set_aim(1.0)
    run(camera, 0.6)
    near, close = camera.fov, camera._boom

    camera.set_aim(0.0)
    run(camera, 1.0)
    back, out = camera.fov, camera._boom

    ok = (near < wide - 5.0 and close < far * 0.75
          and abs(back - wide) < 0.5 and abs(out - far) < 20.0)
    return ok, (f"fov {wide:.0f} -> {near:.0f} -> {back:.0f}, "
                f"boom {far:.0f} -> {close:.0f} -> {out:.0f}")


def check_half_an_amount_is_half_the_aim():
    """Asked for half the sights, the rig goes half way in.

    What `set_aim` taking an amount rather than a flag is for. Nothing is bound
    to an analog control at the moment, so this is the only thing holding the
    blend to being a blend rather than a switch with a ramp on it.
    """
    camera, hero = a_camera()
    settled(camera)
    camera.set_aim(0.5)
    run(camera, 0.6)
    half = camera.fov
    camera.set_aim(1.0)
    run(camera, 0.6)
    full = camera.fov
    share = (cam.HIP_FOV - half) / max(cam.HIP_FOV - full, 1e-6)
    ok = 0.35 < share < 0.65
    return ok, f"half the amount moved the view {share * 100:.0f}% of the way in"


def check_aiming_slows_the_hand():
    """The same mouse movement covers less angle down the sights."""
    turned = []
    for aim in (0.0, 1.0):
        camera, hero = a_camera()
        camera.set_aim(aim)
        run(camera, 1.0)
        before = camera.yaw
        camera.look_mouse(200.0, 0.0)
        run(camera, 0.3)
        turned.append(abs(s16_to_degrees(cam._wrap_angle(before - camera.yaw))))
    share = turned[1] / turned[0]
    ok = abs(share - cam.AIM_SENSITIVITY) < 0.02
    return ok, f"{turned[0]:.1f} deg at the hip, {turned[1]:.1f} down the sights"


# -- the ray -----------------------------------------------------------------


def check_the_ray_is_the_view():
    """The aim ray points exactly where the camera was pointed.

    Rebuilt here from the yaw and the pitch by hand rather than taken from the
    camera, so this fails if the two ever drift apart -- which is what happens
    the moment anything starts placing the camera by looking at a point instead
    of along an angle.
    """
    camera, hero = a_camera()
    settled(camera)
    camera.look(37.0, -12.0)
    run(camera, 0.2)

    origin, direction = camera.aim_ray()
    yaw = math.radians(s16_to_degrees(camera.mario_yaw))
    pitch = camera.pitch
    wanted = (math.sin(yaw) * math.cos(pitch), -math.sin(pitch),
              math.cos(yaw) * math.cos(pitch))
    error = max(abs(direction[i] - wanted[i]) for i in range(3))
    at_camera = max(abs(origin[i] - camera.pos[i]) for i in range(3))
    ok = error < 1e-3 and at_camera < 1e-9
    return ok, f"direction off by {error:.2e}, origin off by {at_camera:.2e}"


def check_the_stick_is_measured_against_the_view():
    """Pushing the stick forward sends him away from the camera.

    `mario_yaw` is what the movement code turns the stick by, and getting it
    backwards sends him at the camera -- which is the one bug in a third-person
    camera that makes a game unplayable rather than merely unpleasant.
    """
    camera, hero = a_camera()
    settled(camera)
    camera.look(70.0, 0.0)
    run(camera, 0.2)

    yaw = math.radians(s16_to_degrees(camera.mario_yaw))
    ahead = (hero.pos[0] + math.sin(yaw) * 1000.0,
             hero.pos[2] + math.cos(yaw) * 1000.0)
    before = math.hypot(camera.pos[0] - hero.pos[0], camera.pos[2] - hero.pos[2])
    after = math.hypot(camera.pos[0] - ahead[0], camera.pos[2] - ahead[1])
    ok = after > before
    return ok, (f"a step along the stick's forward leaves the camera "
                f"{after - before:+.0f} units further off")


# -- nothing points the view but the player -----------------------------------


def check_nothing_re_points_the_view():
    """Left alone, the view stays exactly where it was put.

    Not approximately, and not for a while: no drift back behind him, no
    framing assist, no correction. He runs a hundred units a second under a
    camera that does not answer for it.
    """
    camera, hero = a_camera(yaw=0, speed=38.0)
    settled(camera)
    camera.look(80.0, -20.0)
    yaw, pitch = camera.yaw, camera.pitch
    run(camera, 6.0, before=lambda t: hero.pos.__setitem__(2, t * 1000.0))
    moved = abs(s16_to_degrees(cam._wrap_angle(yaw - camera.yaw)))
    tilted = abs(pitch - camera.pitch)
    ok = moved < 1e-6 and tilted < 1e-9
    return ok, (f"six seconds and a thousand units a second later: "
                f"{moved:.1e} deg of yaw, {tilted:.1e} rad of pitch")


def check_the_boom_length_does_not_move_the_aim():
    """Whatever the boom does, the crosshair keeps pointing at the same spot.

    This is the invariant the occlusion behaviour is built on and the reason
    ground is treated as something to shorten the boom for rather than
    something to lift the camera over: a camera anywhere along its own view ray
    sees the same point at the middle of the screen, so pulling in for a wall
    or a hillside costs the aim nothing, while lifting over one walks the aim
    across the world as the terrain changes.

    The boom is driven here by moving the tunable rather than by finding a wall
    for every length, but it is the same code doing the placing. An earlier
    version measured the boom from the pivot rather than from the shoulder,
    which tilted the line the camera came in along by the shoulder offset and
    moved the aim point by about eleven units over this range -- small, and
    the sort of small that is felt rather than seen, since it happens exactly
    when a wall slides in behind you.
    """
    camera, hero = a_camera()
    settled(camera)
    camera.look(0.0, -8.0)          # a shallow angle: it magnifies any error
    run(camera, 0.5)

    hits = []
    for distance in (cam.HIP_DISTANCE, 620.0, 350.0, cam.MIN_DISTANCE):
        camera.distance = distance
        run(camera, 0.8)
        hits.append(_ground_hit(*camera.aim_ray()))

    worst = max(math.dist(hits[0], hit) for hit in hits)
    spread = abs(camera._boom - cam.HIP_DISTANCE)
    ok = worst < 1.0 and spread > 400.0
    return ok, (f"the boom moved {spread:.0f} units; the aim point moved "
                f"{worst:.4f}")


def check_the_stick_sets_the_boom():
    """Holding the left stick in and pushing the right one dollies the camera.

    Three things, and the third is the one worth having a check for. Pushing
    forward brings it in and pulling back stands it off; the travel is bounded
    at both ends, so a thumb left on the stick cannot put the camera inside his
    head or a mile behind him; and the two lengths are set independently, since
    the framing at the hip and the framing down the sights are different
    decisions and one dragging the other about would mean setting either twice.
    """
    camera, hero = a_camera()
    settled(camera)

    started = camera.distance
    for _ in range(30):
        camera.dolly(-1.0, 1.0 / 60.0)      # forward on the stick: in
    closer = camera.distance
    for _ in range(600):
        camera.dolly(1.0, 1.0 / 60.0)       # and back: out, to the stop
    far = camera.distance
    for _ in range(1200):
        camera.dolly(-1.0, 1.0 / 60.0)
    near = camera.distance

    # Down the sights it is the other length that moves.
    camera.distance = cam.HIP_DISTANCE
    camera.set_aim(1.0)
    run(camera, 1.0)
    aim_started, hip_started = camera.aim_distance, camera.distance
    for _ in range(30):
        camera.dolly(1.0, 1.0 / 60.0)

    problems = []
    if not closer < started:
        problems.append(f"forward did not come in: {started:.0f} -> {closer:.0f}")
    if abs(far - cam.BOOM_MAX) > 0.5 or abs(near - cam.BOOM_MIN) > 0.5:
        problems.append(f"the stops are at {near:.0f} and {far:.0f}, not "
                        f"{cam.BOOM_MIN:.0f} and {cam.BOOM_MAX:.0f}")
    if camera.aim_distance <= aim_started:
        problems.append("the sights' length did not move down the sights")
    if camera.distance != hip_started:
        problems.append("the hip length moved down the sights as well")

    return not problems, ("; ".join(problems) if problems else
                          f"{started:.0f} -> {closer:.0f} in, out to the stop "
                          f"at {far:.0f}, in to {near:.0f}; the sights move "
                          f"on their own ({aim_started:.0f} -> "
                          f"{camera.aim_distance:.0f})")


def _ground_hit(origin, direction):
    """Where a ray meets the y = 0 plane. The plane every check stands on."""
    if direction[1] >= -1e-6:
        return (origin[0], 0.0, origin[2])
    t = -origin[1] / direction[1]
    return (origin[0] + direction[0] * t, 0.0, origin[2] + direction[2] * t)


def check_recentring_arrives():
    """R puts the camera on his back, and gets all the way there."""
    camera, hero = a_camera(yaw=0)
    settled(camera)
    camera.look(150.0, -20.0)
    camera.update(1.0 / 60.0)

    # One press: held for a frame, then released, and it finishes on its own.
    camera.update(1.0 / 60.0, recenter=True)
    run(camera, 1.5)
    off = abs(s16_to_degrees(cam._wrap_angle(
        camera.yaw - s16(hero.face_angle[1] + 0x8000))))
    ok = off < 1.0 and abs(camera.pitch - cam.DEFAULT_PITCH) < 0.02
    return ok, f"{off:.2f} deg off his back, pitch {camera.pitch:.3f}"


CHECKS = [
    ("a look lands this frame", check_look_lands_this_frame),
    ("the mouse ignores the frame rate", check_look_survives_the_frame_rate),
    ("the stick ignores the frame rate", check_stick_survives_the_frame_rate),
    ("the stick ramps into a turn", check_the_stick_ramps),
    ("the curve leaves a slow turn", check_the_curve_leaves_a_slow_turn),
    ("the camera settles behind him", check_the_camera_settles_behind_him),
    ("following does not overshoot", check_following_does_not_overshoot),
    ("small steps do not move it", check_small_steps_do_not_move_the_camera),
    ("big climbs are followed", check_big_climbs_are_followed),
    ("the boom comes in at once", check_the_boom_comes_in_at_once),
    ("the boom goes out slowly", check_the_boom_goes_out_slowly),
    ("the camera stays above ground", check_the_camera_stays_above_the_ground),
    ("the sights pull in and let go", check_the_sights_pull_in_and_let_go),
    ("half an amount is half the aim", check_half_an_amount_is_half_the_aim),
    ("aiming slows the hand", check_aiming_slows_the_hand),
    ("the ray is the view", check_the_ray_is_the_view),
    ("the stick is measured against the view",
     check_the_stick_is_measured_against_the_view),
    ("nothing re-points the view", check_nothing_re_points_the_view),
    ("the boom does not move the aim",
     check_the_boom_length_does_not_move_the_aim),
    ("the stick sets the boom", check_the_stick_sets_the_boom),
    ("recentring arrives", check_recentring_arrives),
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
    sys.exit(main())
