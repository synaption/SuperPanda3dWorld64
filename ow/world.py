"""Simulation state: the bodies, the player, and the fixed-step tick.

Deliberately free of any scene-graph or window dependency so it can be stepped
headlessly (see `python -m ow.main --selftest`). The renderer reads positions
and orientations back out of here each frame.
"""

from panda3d.core import LVector3d

from .constants import (
    FIXED_TIMESTEP,
    MAX_STEPS_PER_FRAME,
    PLAYER_COLLISION_RADIUS,
    PLAYER_MASS,
    CharacterVariables,
)
from .gravity import GravityComponent, GravityWorld
from .level import PLAYER_START, demo_system
from .movement import InputState, SpaceMovementComponent


class World:
    def __init__(self, bodies=None, variables=None, planets_attract_each_other=False):
        self.gravity = GravityWorld(planets_attract_each_other)
        self.definitions = bodies if bodies is not None else demo_system()

        self.planets = []
        for definition in self.definitions:
            body = GravityComponent(
                definition.name,
                definition.mass,
                position=definition.position,
                initial_speed=definition.initial_speed,
                radius=definition.radius,
                is_planet=True,
            )
            self.planets.append(self.gravity.add(body))

        self.player = self.gravity.add(
            GravityComponent(
                "Player",
                PLAYER_MASS,
                position=PLAYER_START,
                radius=PLAYER_COLLISION_RADIUS,
                is_planet=False,
            )
        )
        self.movement = SpaceMovementComponent(
            self.player, variables or CharacterVariables()
        )
        self.input = InputState()

        self._accumulator = 0.0
        self.elapsed = 0.0

    # -- ticking -----------------------------------------------------------

    def step(self, dt):
        """Advance one fixed step: input forces first, then integration."""
        self.movement.update(self.input, dt)
        # Mouse motion is a displacement already spent, not a held state, so
        # it must not be re-applied if several fixed steps run in one frame.
        self.input.look_impulse = (0.0, 0.0)
        self.gravity.step(dt)
        self.elapsed += dt

    def advance(self, frame_dt):
        """Consume a variable frame time in whole fixed steps.

        Returns the number of steps run. Leftover time carries to next frame;
        if the frame was very long the surplus is dropped rather than letting
        the sim spiral.
        """
        self._accumulator += frame_dt
        steps = 0
        while self._accumulator >= FIXED_TIMESTEP and steps < MAX_STEPS_PER_FRAME:
            self.step(FIXED_TIMESTEP)
            self._accumulator -= FIXED_TIMESTEP
            steps += 1
        if self._accumulator > FIXED_TIMESTEP * MAX_STEPS_PER_FRAME:
            self._accumulator = 0.0
        return steps

    # -- queries for the HUD ----------------------------------------------

    @property
    def player_speed(self):
        return self.player.speed.length()

    def nearest_planet(self):
        return self.gravity.nearest_body(self.player.position, exclude=self.player)

    def surface_gravity(self, body):
        """Acceleration a body this size would impose at its own surface."""
        if body.radius <= 0.0:
            return 0.0
        return body.gravity_constant * body.mass_self / body.radius

    def total_momentum(self):
        total = LVector3d(0, 0, 0)
        for body in self.gravity.bodies:
            total += body.speed * body.mass_self
        return total
