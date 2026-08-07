"""Mario: state, physics stepping, and the action state machine."""

from .actions import ACTIONS, execute_action, set_mario_action
from .state import Controller, MarioState

# Imported for its side effect: the submerged actions register themselves into
# ACTIONS on import, and nothing else refers to the module by name.
from . import water  # noqa: F401

__all__ = [
    "ACTIONS",
    "Controller",
    "MarioState",
    "execute_action",
    "set_mario_action",
]
