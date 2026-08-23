"""The demo system, read out of L_DemoLevel.umap.

Positions, scales and masses are the ones authored in the Unreal level, then
doubled for this roomier version of the system. Unreal is left-handed and Panda
is right-handed, so Y is negated on the way in; the system is otherwise
identical, including the fact that nothing starts with any orbital velocity --
the whole thing is released from rest and falls together. Give a body an
`initial_speed` if you want it to orbit instead.

Names are ours. The original level leaves the planets unnamed.
"""

from dataclasses import dataclass, field

from .constants import PLANET_COLLISION_UNIT_RADIUS, PLANET_MESH_UNIT_RADIUS

#: Applied to all body positions/radii/masses and the player start.  Mass
#: scales with radius so the 1 / r law preserves each body's surface gravity.
SYSTEM_SCALE = 2.0

#: PlayerStart, converted to Panda's handedness and scaled with the system.
_AUTHORED_PLAYER_START = (-249300.0, -33165.0, 6285.0)
PLAYER_START = tuple(value * SYSTEM_SCALE for value in _AUTHORED_PLAYER_START)


@dataclass
class Body:
    name: str
    mass: float
    position: tuple
    #: BP_Planet's Scale, applied to a unit UE sphere of radius 50.
    scale: float
    initial_speed: tuple = (0.0, 0.0, 0.0)
    color: tuple = (0.6, 0.62, 0.66, 1.0)
    emissive: bool = False

    @property
    def radius(self):
        """The surface you actually hit."""
        return self.scale * PLANET_COLLISION_UNIT_RADIUS

    @property
    def visual_radius(self):
        """The surface you see -- 1.6% outside the collider, as authored."""
        return self.scale * PLANET_MESH_UNIT_RADIUS


#: Loosely rocky/icy/gas colours so the bodies are tellable apart in flight.
_PALETTE = [
    (0.72, 0.38, 0.28, 1.0),
    (0.62, 0.60, 0.55, 1.0),
    (0.38, 0.58, 0.42, 1.0),
    (0.70, 0.66, 0.48, 1.0),
    (0.78, 0.55, 0.30, 1.0),
    (0.45, 0.52, 0.68, 1.0),
    (0.55, 0.32, 0.34, 1.0),
    (0.40, 0.42, 0.52, 1.0),
    (0.66, 0.62, 0.70, 1.0),
    (0.58, 0.56, 0.50, 1.0),
    (0.74, 0.62, 0.44, 1.0),
    (0.48, 0.46, 0.60, 1.0),
    (0.68, 0.68, 0.62, 1.0),
]


def demo_system():
    bodies = [
        Body("Ember", 3.0, (238378.0, 324927.0, 2340.0), 656.5931),
        Body("Pebble", 1.0, (493932.8, 197610.0, 2340.0), 298.9644),
        Body("Verdant", 4.0, (232120.0, 139936.9, 2340.0), 734.1511),
        Body("Mote", 1.0, (53828.4, -44457.4, 2340.0), 254.3012),
        Body("Amber", 2.0, (120869.4, -334990.9, 2340.0), 656.5931),
        Body("Hearth", 2.0, (-294000.0, 680.0, 2340.0), 452.1615),
        Body("Cinder", 3.0, (-1064.4, 55986.4, 2340.0), 734.1511),
        Body("Dusk", 3.0, (-174067.0, 87738.0, 2340.0), 656.5931),
        Body("Talon", 2.0, (-16965.0, 291745.6, 2340.0), 656.5931),
        Body("Speck", 2.0, (464839.7, -314747.6, 2340.0), 452.1615),
        Body("Giant", 4.0, (389430.6, -64421.3, 2340.0), 1039.3984),
        Body("Warden", 4.0, (8490.2, -202095.9, 2340.0), 1039.3984),
        Body("Chip", 1.0, (-221502.8, -134786.6, 2340.0), 298.9644),
        Body(
            "Sun",
            8.0,
            (150750.0, 120.0, 11130.0),
            2025.9637,
            color=(1.0, 0.86, 0.55, 1.0),
            emissive=True,
        ),
    ]
    for i, body in enumerate(bodies):
        if not body.emissive:
            body.color = _PALETTE[i % len(_PALETTE)]
    return [
        Body(
            body.name,
            body.mass * SYSTEM_SCALE,
            tuple(value * SYSTEM_SCALE for value in body.position),
            body.scale * SYSTEM_SCALE,
            initial_speed=body.initial_speed,
            color=body.color,
            emissive=body.emissive,
        )
        for body in bodies
    ]
