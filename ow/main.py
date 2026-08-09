"""Entry point.

    python -m ow.main              run the demo system
    python -m ow.main --selftest   headless physics checks, no window

Running the file directly -- `python ow/main.py` -- works too; the block below
puts the project root on the path so the package-relative imports resolve.
"""

import argparse
import os
import sys

if __name__ == "__main__" and __package__ in (None, ""):
    # Executed as a plain script, so there is no parent package for `from .x`
    # to resolve against. Adopt one.
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    __package__ = "ow"
    import ow  # noqa: F401  (binds the name the relative imports need)


def selftest():
    """Exercise the simulation without a window.

    Checks the things a port can silently get wrong: the falloff law, that a
    body's own mass cancels out of gravitational acceleration, that thrust
    follows the direction arrow rather than the lagging camera, that the
    camera actually converges, and that an isolated pair conserves momentum.
    """
    import math

    from panda3d.core import LVector3d

    from .constants import (
        FIXED_TIMESTEP,
        GRAVITY_ALL,
        GRAVITY_CONSTANT,
        GROUND_TOLERANCE,
        WALK_SPEED,
    )
    from .gravity import GravityComponent, GravityWorld, vec3d
    from .movement import InputState, vec3f
    from .world import World

    failures = []

    def check(name, condition, detail=""):
        print("  {:4}  {}{}".format("ok" if condition else "FAIL", name,
                                    "" if condition else "  <- " + detail))
        if not condition:
            failures.append(name)

    print("gravity")
    a = GravityComponent("a", 2.0, position=(0, 0, 0))
    b = GravityComponent("b", 5.0, position=(1000.0, 0, 0))
    force = a.gravitational_force_toward(b)
    expected = GRAVITY_CONSTANT * 2.0 * 5.0 / 1000.0
    check("Fg = G*m*M/r (linear falloff, per the original's comment)",
          abs(force.length() - expected) < 1e-6,
          "{} != {}".format(force.length(), expected))
    check("force points at the other body", force.getX() > 0 and abs(force.getY()) < 1e-9)

    # Halving the distance should double the force, not quadruple it.
    b.position = LVector3d(500.0, 0, 0)
    half = a.gravitational_force_toward(b).length()
    check("halving r doubles Fg (not 4x)", abs(half / expected - 2.0) < 1e-9,
          "ratio {}".format(half / expected))

    print("attracted mass cancels")
    accelerations = []
    for mass in (1.0, 7.5):
        probe = GravityComponent("probe", mass, position=(0, 0, 0))
        planet = GravityComponent("planet", 4.0, position=(20000.0, 0, 0), is_planet=True)
        probe.accumulate_gravity([probe, planet])
        probe.integrate(FIXED_TIMESTEP)
        accelerations.append(probe.acceleration.length())
    check("a = G*M/r regardless of the falling body's mass",
          abs(accelerations[0] - accelerations[1]) < 1e-9 * accelerations[0],
          "{} vs {}".format(*accelerations))

    print("momentum")
    world = GravityWorld()
    world.add(GravityComponent("x", 3.0, position=(-5000.0, 0, 0)))
    world.add(GravityComponent("y", 4.0, position=(5000.0, 0, 0)))
    for _ in range(600):
        world.step(FIXED_TIMESTEP)
    momentum = LVector3d(0, 0, 0)
    for body in world.bodies:
        momentum += body.speed * body.mass_self
    scale = sum(abs(b.speed.length() * b.mass_self) for b in world.bodies)
    check("an isolated pair conserves momentum", momentum.length() < 1e-9 * max(scale, 1.0),
          "|p| = {} against scale {}".format(momentum.length(), scale))

    print("movement")
    sim = World()
    movement = sim.movement
    start = LVector3d(sim.player.position)

    # Thrust must follow the arrow immediately, while the camera lags.
    state = InputState()
    state.look_impulse = (90.0, 0.0)
    sim.input = state
    sim.step(FIXED_TIMESTEP)
    arrow_after = movement.arrow_quat.getForward()
    camera_after = movement.camera_quat.getForward()
    check("look turns the direction arrow at once",
          abs(arrow_after.getX()) > 0.5,
          "forward {}".format(arrow_after))
    check("camera lags behind the arrow",
          (arrow_after - camera_after).length() > 0.5,
          "arrow {} camera {}".format(arrow_after, camera_after))

    for _ in range(600):
        movement.update_camera(FIXED_TIMESTEP)
    check("camera converges on the arrow",
          (movement.arrow_quat.getForward() - movement.camera_quat.getForward()).length() < 1e-3)

    print("jetpack")
    sim = World()
    sim.input.clear()
    sim.input.move = (0.0, 1.0)
    forward = vec3d(sim.movement.forward)
    before = LVector3d(sim.player.speed)
    sim.step(FIXED_TIMESTEP)
    gained = sim.player.speed - before
    along = gained.dot(forward)
    check("forward thrust accelerates along the arrow's forward axis",
          along > 0.0, "delta-v along forward = {}".format(along))
    expected_dv = sim.movement.variables.move_acceleration / sim.player.mass_self * FIXED_TIMESTEP
    check("thrust magnitude matches MoveAcceleration/mass",
          abs(along - expected_dv) < expected_dv * 0.35,
          "{:.3f} vs ~{:.3f} (gravity also acting)".format(along, expected_dv))

    print("brake")
    sim = World()
    sim.input.clear()
    sim.player.speed = LVector3d(1200.0, 0.0, 0.0)
    sim.input.brake = True
    for _ in range(240):
        sim.step(FIXED_TIMESTEP)
    check("braking bleeds off speed", sim.player.speed.length() < 1200.0,
          "speed {}".format(sim.player.speed.length()))

    # Braking from a low speed must stop, not reverse.
    sim = World(bodies=[])
    sim.input.clear()
    sim.player.speed = LVector3d(5.0, 0.0, 0.0)
    sim.input.brake = True
    sim.step(FIXED_TIMESTEP)
    check("braking cannot reverse the velocity",
          sim.player.speed.getX() > -1e-6 and sim.player.speed.length() < 1e-6,
          "speed {}".format(sim.player.speed))

    print("planets attract each other (bPlanetsAttractEachOther = False)")
    sim = World()
    check("the level default leaves planets inert",
          not sim.gravity.planets_attract_each_other)
    for _ in range(60 * 60):  # 60 s
        sim.step(FIXED_TIMESTEP)
    drift = max((b.position - vec3d(d.position)).length()
                for b, d in zip(sim.planets, sim.definitions))
    check("planets do not move, even with the player nearby",
          drift == 0.0, "max drift {} cm".format(drift))

    # ...and the toggle really does turn full n-body back on.
    sim = World(planets_attract_each_other=True)
    for _ in range(600):
        sim.step(FIXED_TIMESTEP)
    moved = max((b.position - vec3d(d.position)).length()
                for b, d in zip(sim.planets, sim.definitions))
    check("the toggle restores mutual attraction", moved > 0.0,
          "max drift {} cm".format(moved))

    print("collision (K2_AddActorWorldOffset with bSweep = true)")
    sim = World()
    target = sim.planets[5]
    contact = target.radius + sim.player.radius
    up = LVector3d(0.0, 0.0, 1.0)
    sim.player.position = target.position + up * (contact + 5000.0)
    sim.player.speed = LVector3d(0, 0, 0)
    closest = None
    for _ in range(60 * 20):
        sim.step(FIXED_TIMESTEP)
        d = (sim.player.position - target.position).length()
        closest = d if closest is None else min(closest, d)
    check("falling onto a planet stops at its surface",
          closest >= contact - 1e-6,
          "reached {:.1f} cm, surface at {:.1f}".format(closest, contact))
    check("and actually reaches it rather than hanging above",
          closest < contact + 50.0, "closest {:.1f} cm".format(closest))

    # Sweeping, not point-sampling: a fast body must not skip through.
    sim = World()
    target = sim.planets[5]
    contact = target.radius + sim.player.radius
    offset = LVector3d(0.0, 0.0, 1.0) * (contact + 1000.0)
    sim.player.position = target.position + offset
    sim.player.speed = -offset / offset.length() * 5.0e5   # 5 km per step
    sim.step(FIXED_TIMESTEP)
    check("a fast body does not tunnel through a planet",
          (sim.player.position - target.position).length() >= contact - 1e-6,
          "ended {:.1f} cm from centre".format(
              (sim.player.position - target.position).length()))

    print("nearest-body gravity")
    sim = World()
    target = sim.planets[5]
    up = LVector3d(0.0, 0.0, 1.0)
    sim.player.position = target.position + up * (target.radius + sim.player.radius)
    sim.player.accumulate_gravity(sim.gravity.bodies, False)
    field = sim.player.gravity_force / sim.player.mass_self
    off_axis = (field - up * field.dot(up)).length()
    check("gravity points straight down at the surface you are on",
          off_axis < 1e-6, "{:.3f} cm/s^2 sideways".format(off_axis))
    # The original summed every body; that is what made surfaces unwalkable.
    sim2 = World(gravity_mode=GRAVITY_ALL)
    sim2.player.position = LVector3d(sim.player.position)
    sim2.player.accumulate_gravity(sim2.gravity.bodies, False)
    field2 = sim2.player.gravity_force / sim2.player.mass_self
    check("summing every body does not, which is why walking needed a change",
          (field2 - up * field2.dot(up)).length() > 100.0,
          "{:.1f} cm/s^2 sideways".format((field2 - up * field2.dot(up)).length()))

    print("walking")
    sim = World()
    target = sim.planets[5]
    contact = target.radius + sim.player.radius
    up = (sim.player.position - target.position)
    up /= up.length()
    sim.player.position = target.position + up * (contact + 2000.0)
    sim.player.speed = LVector3d(0, 0, 0)
    for _ in range(int(6.0 / FIXED_TIMESTEP)):
        sim.step(FIXED_TIMESTEP)
    check("you land and end up standing", sim.movement.grounded)
    check("and come to rest rather than sliding",
          sim.player.speed.length() < 30.0,
          "{:.1f} cm/s".format(sim.player.speed.length()))

    sim.input.clear()
    sim.input.move = (0.0, 1.0)
    altitudes, airborne = [], 0
    for _ in range(int(25.0 / FIXED_TIMESTEP)):
        sim.step(FIXED_TIMESTEP)
        altitudes.append(
            (sim.player.position - target.position).length() - contact)
        if not sim.movement.grounded:
            airborne += 1
    check("walking holds the surface -- facing is re-flattened as it curves",
          max(altitudes) < GROUND_TOLERANCE and airborne == 0,
          "peak {:.2f} cm, airborne on {} steps".format(max(altitudes), airborne))
    check("and reaches walking speed",
          abs(sim.player.speed.length() - WALK_SPEED) < WALK_SPEED * 0.05,
          "{:.1f} vs {:.1f} cm/s".format(sim.player.speed.length(), WALK_SPEED))
    # Feet on the ground means two things: you face along the surface, and
    # your head tilts only by however far you are looking up or down. (Not
    # "up == normal" -- looking down 37 degrees tilts your up by 37 degrees.)
    normal = vec3f((sim.player.position - target.position).normalized())
    check("facing stays in the surface's tangent plane",
          abs(sim.movement.walk_forward.dot(normal)) < 1e-2,
          "{:.2e} out of plane".format(sim.movement.walk_forward.dot(normal)))
    check("head tilts only by the view pitch",
          abs(sim.movement.arrow_quat.getUp().dot(normal)
              - math.cos(math.radians(sim.movement.ground_pitch))) < 0.02,
          "up.n {:.4f} vs cos(pitch) {:.4f}".format(
              sim.movement.arrow_quat.getUp().dot(normal),
              math.cos(math.radians(sim.movement.ground_pitch))))

    sim.input.clear()
    for _ in range(int(2.0 / FIXED_TIMESTEP)):
        sim.step(FIXED_TIMESTEP)
    check("letting go stops you", sim.player.speed.length() < 30.0,
          "{:.1f} cm/s".format(sim.player.speed.length()))

    sim.input.clear()
    sim.input.up_down = 1.0
    sim.step(FIXED_TIMESTEP)
    sim.input.clear()
    peak, landed = 0.0, False
    for _ in range(int(5.0 / FIXED_TIMESTEP)):
        sim.step(FIXED_TIMESTEP)
        peak = max(peak, (sim.player.position - target.position).length() - contact)
        landed = landed or sim.movement.grounded
    check("jumping leaves the ground and comes back down",
          peak > 50.0 and landed, "peak {:.0f} cm".format(peak))

    print("demo system")
    sim = World()
    _, gap = sim.nearest_planet()
    check("player starts outside every planet", gap > 0.0, "gap {}".format(gap))
    surface = [sim.surface_gravity(body) / 100.0 for body in sim.planets]
    rocky = [g for g in surface[:-1]]
    check("surface gravity lands in a human range (m/s^2)",
          all(5.0 < g < 40.0 for g in rocky),
          "min {:.2f} max {:.2f}".format(min(rocky), max(rocky)))
    print("       surface gravity: " + ", ".join(
        "{} {:.1f}".format(b.name, g) for b, g in zip(sim.planets, surface)))

    sim = World()
    for _ in range(1800):  # 30 s
        sim.advance(FIXED_TIMESTEP)
    finite = all(
        math.isfinite(c) for body in sim.gravity.bodies for c in body.position
    )
    check("30 s of simulation stays finite", finite)
    check("player start recorded", (sim.player.position - start).length() >= 0.0)

    print()
    if failures:
        print("{} check(s) failed: {}".format(len(failures), ", ".join(failures)))
        return 1
    print("all checks passed")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description="Outer Wilds player controller, Panda3D port")
    parser.add_argument("--selftest", action="store_true",
                        help="run headless physics checks and exit")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    from .app import OuterWildsApp, configure

    configure()
    OuterWildsApp().run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
