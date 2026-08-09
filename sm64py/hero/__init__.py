"""The Hero: his own action state machine, on Mario's physics.

The state object extends Mario's so the decomp's quarter-step collision moves
him; everything above that -- which actions exist, what they do, and what they
draw -- is his own.
"""

from .actions import ACTIONS, execute_action, set_hero_action
from .state import HeroState

__all__ = [
    "ACTIONS",
    "HeroState",
    "execute_action",
    "set_hero_action",
]
