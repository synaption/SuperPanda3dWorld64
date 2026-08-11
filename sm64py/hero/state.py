"""What the Hero's action machine reads and writes each frame.

`HeroState` extends `MarioState` rather than restating it. The division is
deliberate: everything the base class holds is *physical* state -- where the
character is, what floor and ceiling and water he is in, what the stick and
buttons did this frame -- and none of it is about being Mario. The step
functions in `sm64py/mario/steps.py` read that surface directly, so inheriting
it is what lets the Hero move through the level with the original's quarter
steps, wall pushback and ledge detection instead of a second, worse collision
routine written to sit beside it.

What is *not* inherited is the part that makes Mario Mario: the action state
machine. `sm64py/hero/actions.py` replaces it wholesale, and none of the
decomp's ~60 actions run for the Hero.
"""

from ..mario.constants import ACT_UNINITIALIZED
from ..mario.state import MarioState
from . import constants as H


class HeroState(MarioState):
    """The Hero, standing on the same floors Mario does."""

    voice_profile = "hero"
    motion_scale = 1.0

    # He skates on the jetpack rather than on a pair of skates, but the ice is
    # the same ice: `MarioState.get_floor_class` reads this to know which
    # action puts it underfoot.
    skating_action = H.ACT_HERO_SKATING

    def __init__(self, surfaces, controller=None):
        super().__init__(surfaces, controller)

        self.action = H.ACT_HERO_IDLE
        self.prev_action = ACT_UNINITIALIZED

        # Frames left in the window where pressing B chains the second swing.
        # Counted down by the attack action rather than by the state, so a
        # cancelled attack drops it without anything having to remember to.
        self.combo_window = 0
        # Which swing is next: 0 for the first, 1 for the second.
        self.combo_index = 0

        # The sword is a toggle, and the draw clip plays in reverse to sheathe
        # it. Nothing else in the moveset reads this yet -- the attacks look
        # the same either way -- so it is presentation for now, and the place
        # a drawn-only moveset would hang off later.
        self.sword_drawn = False

        self.anim_name = "idle"

    def spawn(self, x, y, z, yaw_degrees=0.0):
        super().spawn(x, y, z, yaw_degrees)
        self.action = H.ACT_HERO_IDLE
        self.prev_action = ACT_UNINITIALIZED
        self.combo_window = 0
        self.combo_index = 0

    def bounce_off_enemy(self, velocity):
        from .actions import set_hero_action
        set_hero_action(self, H.ACT_HERO_JUMP, 0)
        # Set after the transition, and after the jump action would set its own
        # take-off velocity, so the bounce is what actually carries him up.
        self.action_timer = 1
        self.vel[1] = velocity

    def take_enemy_hit(self, away_yaw, speed, velocity):
        """Knocked back -- as a fall, because there is no knockback clip.

        Mario has a clip of being thrown onto his back and this character does
        not, so he takes the same push and the same loss of control, and draws
        it with the falling pose instead of an invented one.
        """
        from .actions import set_hero_action
        self.face_angle[1] = away_yaw
        self.set_forward_vel(-speed)
        set_hero_action(self, H.ACT_HERO_FALL, 0)
        self.vel[1] = velocity

    def sync_graphics(self):
        """Draw him upright, whatever the floor is doing.

        The base class tilts Mario's whole body while he is swimming, which is
        the one case the Hero does not have: he wades rather than swims, and a
        wade is drawn standing up.
        """
        self.gfx_pos = list(self.pos)
        self.gfx_angle = [0, self.face_angle[1], 0]

    @property
    def action_name(self):
        return H.ACTION_NAMES.get(self.action, hex(self.action))
