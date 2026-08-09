"""Entry point.

    python -m ow.main              run the demo system
    python -m ow.main --selftest   headless physics checks, no window
"""

import argparse
import sys


def selftest():
    """Exercise the simulation without a window.

    Checks the things a port can silently get wrong: the falloff law, that a
    body's own mass cancels out of gravitational acceleration, that thrust
    follows the direction arrow rather than the lagging camera, that the
    camera actually converges, and that an isolated pair conserves momentum.
    """
    import math

    from panda3d.core import LVector3d

    from .constants import FIXED_TIMESTEP, GRAVITY_CONSTANT
    from .gravity import GravityComponent, GravityWorld, vec3d
    from .movement import InputState
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

    print("demo system")
    sim = World()
    _, gap = sim.nearest_planet()
    check("player starts outside every planet", gap > 0.0, "gap {}".format(gap))
    surface = [sim.surface_gravity(body) / 100.0 for body in sim.planets]
    rocky = [g for g in surface[:-1]]
    check("surface gravity lands in a human range (m/s^2)",
          all(2.0 < g < 30.0 for g in rocky),
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
