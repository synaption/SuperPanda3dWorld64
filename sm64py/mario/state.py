"""Mario's per-frame state, controller sampling, and geometry queries."""

import math

from ..math_util import atan2s, coss, s16, sins
from . import constants as C

# Stand-in water height for areas with no water at all. Far below any real
# collision, so "is Mario underwater" tests are simply false there.
NO_WATER = -11000.0


class Controller:
    """One frame of controller input, in N64 units.

    The stick is reported as a pair in roughly [-64, 64] plus a magnitude,
    because gameplay code squares the magnitude separately from the direction.
    """

    __slots__ = ("stick_x", "stick_y", "stick_mag", "button_down",
                 "button_pressed", "_prev_down", "zombie", "skating",
                 "thrust", "thrust_pressed")

    def __init__(self):
        self.stick_x = 0.0
        self.stick_y = 0.0
        self.stick_mag = 0.0
        self.button_down = 0
        self.button_pressed = 0
        self._prev_down = 0
        # Not an N64 button. The controller had no room for one, and the
        # zombie shamble changes nothing the action code can see -- it swaps
        # which clip standing and walking draw and stops there -- so it rides
        # here rather than being folded into the button mask, where every
        # action test would have to learn to ignore it.
        self.zombie = False

        # Skates on. Unlike the zombie this one does reach the action code --
        # it is what ACT_SKATING stays in, and what puts ice underfoot.
        self.skating = False

        # The jetpack's own control, on the Hero's left trigger. Also not an
        # N64 button, and it rides here for the same reason the skates do:
        # folding it into the button mask would mean picking a bit the original
        # already means something by, and Mario shares this controller.
        #
        # The press is kept beside the hold because both are asked for -- the
        # hold is the thrust, and the press is what lights the boosters again
        # after they have been let go of in mid-air.
        self.thrust = False
        self.thrust_pressed = False

    def set_thrust(self, down):
        """Feed the jetpack control. Once a tick, as `set_buttons` is."""
        self.thrust_pressed = bool(down) and not self.thrust
        self.thrust = bool(down)

    def set_stick(self, x, y):
        """Feed a normalised [-1, 1] stick position, with a deadzone."""
        mag = math.hypot(x, y)
        if mag < 0.1:
            self.stick_x = self.stick_y = self.stick_mag = 0.0
            return

        # Clamp to the circular gate the real stick is limited by.
        if mag > 1.0:
            x, y, mag = x / mag, y / mag, 1.0

        self.stick_x = x * 64.0
        self.stick_y = y * 64.0
        self.stick_mag = mag * 64.0

    def set_buttons(self, down):
        self.button_pressed = down & ~self._prev_down
        self._prev_down = self.button_down = down


class MarioState:
    """Everything the action state machine reads and writes each frame."""

    def __init__(self, surfaces, controller=None):
        self.surfaces = surfaces
        self.controller = controller or Controller()

        self.input = 0
        self.flags = C.MARIO_NORMAL_CAP
        self.particle_flags = 0
        self.action = C.ACT_IDLE
        self.prev_action = C.ACT_UNINITIALIZED
        self.action_state = 0
        self.action_timer = 0
        self.action_arg = 0

        self.intended_mag = 0.0
        self.intended_yaw = 0
        self.invinc_timer = 0
        self.framesSinceA = 0xFF
        self.framesSinceB = 0xFF
        self.wall_kick_timer = 0
        self.double_jump_timer = 0

        self.face_angle = [0, 0, 0]
        self.angle_vel = [0, 0, 0]
        self.slide_yaw = 0
        self.twirl_yaw = 0

        self.pos = [0.0, 0.0, 0.0]
        self.vel = [0.0, 0.0, 0.0]
        self.forward_vel = 0.0
        self.slide_vel_x = 0.0
        self.slide_vel_z = 0.0

        self.wall = None
        self.ceil = None
        self.floor = None
        self.ceil_height = 0.0
        self.floor_height = 0.0
        self.floor_angle = 0
        self.water_level = NO_WATER

        # How hard the current swim stroke pulls. Chaining strokes builds it
        # up; letting go drops it back to the minimum. The engine keeps this
        # in a file-level static, which works because there is only ever one
        # Mario -- here it lives on him.
        self.swim_strength = C.MIN_SWIM_STRENGTH


        # Sound IDs the actions raised this frame. Actions never play anything
        # themselves, so the simulation stays independent of whether the front
        # end has audio at all. Drained once per frame by the front end.
        self.sound_events = []

        # Set by an action that wants its clip restarted from the top even
        # though the clip itself has not changed, which is how a held stroke
        # reads as one continuous cycle. Cleared once the front end acts on it.
        self.anim_reset = False
        self.peak_height = 0.0
        self.quicksand_depth = 0.0
        self.squish_timer = 0
        self.health = 0x880
        self.hurt_counter = 0
        self.num_lives = 4

        # Graphical position lags the physics position by one frame; the
        # engine falls back to it when Mario ends up out of bounds.
        self.gfx_pos = [0.0, 0.0, 0.0]
        self.gfx_angle = [0, 0, 0]

        # Set by the camera each frame; the analog stick is relative to it.
        self.camera_yaw = 0

        self.anim_name = "idle"
        self.anim_frame = 0

        # Which of tiptoe / walk / run the walking action last drew, so the
        # choice can hold its ground when his speed sawtooths across the
        # threshold between two of them. See animations._walking.
        self.gait_anim = None

    # -- spawning -----------------------------------------------------------

    def spawn(self, x, y, z, yaw_degrees=0.0):
        self.pos = [float(x), float(y), float(z)]
        self.face_angle = [0, s16(round(yaw_degrees * 65536.0 / 360.0)), 0]
        self.vel = [0.0, 0.0, 0.0]
        self.forward_vel = 0.0

        # Search from well above the requested point.  A floor query only
        # reports surfaces at or below the sample, so spawning even slightly
        # under the ground would otherwise find nothing and drop Mario out of
        # the world.
        self.floor_height, self.floor = self.surfaces.find_floor(x, y + 1000.0, z)
        if self.floor is None:
            self.floor_height, self.floor = self.surfaces.find_floor(x, y, z)
        if self.floor is not None:
            self.pos[1] = max(self.pos[1], self.floor_height)

        self.gfx_pos = list(self.pos)
        self.action = C.ACT_IDLE
        self.peak_height = self.pos[1]

    # -- floor classification ----------------------------------------------

    def get_floor_class(self):
        # Skating puts ice under him wherever he happens to be, which is the
        # whole of what the skates do -- everything below reads the floor class
        # rather than the surface type, so overriding it here is what makes
        # momentum, friction, steering and slope acceleration all become the
        # very-slippery ones the game already has. The decomp plays the same
        # trick in the other direction for crawling, a few lines down.
        if self.action == C.ACT_SKATING:
            return C.SURFACE_CLASS_VERY_SLIPPERY

        floor_class = C.SURFACE_CLASS_DEFAULT
        if self.floor is not None:
            ftype = self.floor.type
            if ftype in C.NOT_SLIPPERY_TYPES:
                floor_class = C.SURFACE_CLASS_NOT_SLIPPERY
            elif ftype in C.SLIPPERY_TYPES:
                floor_class = C.SURFACE_CLASS_SLIPPERY
            elif ftype in C.VERY_SLIPPERY_TYPES:
                floor_class = C.SURFACE_CLASS_VERY_SLIPPERY

        # Crawling keeps Mario planted on slopes he would otherwise slide down.
        if (self.action == C.ACT_CRAWLING and self.floor is not None
                and self.floor.normal[1] > 0.5
                and floor_class == C.SURFACE_CLASS_DEFAULT):
            floor_class = C.SURFACE_CLASS_NOT_SLIPPERY

        return floor_class

    def floor_is_slippery(self):
        if self.floor is None:
            return False
        limits = {
            C.SURFACE_CLASS_VERY_SLIPPERY: 0.9848077,   # cos(10 deg)
            C.SURFACE_CLASS_SLIPPERY: 0.9396926,        # cos(20 deg)
            C.SURFACE_CLASS_NOT_SLIPPERY: 0.0,
        }
        limit = limits.get(self.get_floor_class(), 0.7880108)  # cos(38 deg)
        return self.floor.normal[1] <= limit

    def floor_is_slope(self):
        if self.floor is None:
            return False
        limits = {
            C.SURFACE_CLASS_VERY_SLIPPERY: 0.9961947,   # cos(5 deg)
            C.SURFACE_CLASS_SLIPPERY: 0.9848077,        # cos(10 deg)
            C.SURFACE_CLASS_NOT_SLIPPERY: 0.9396926,    # cos(20 deg)
        }
        limit = limits.get(self.get_floor_class(), 0.9659258)  # cos(15 deg)
        return self.floor.normal[1] <= limit

    def floor_is_steep(self):
        if self.floor is None or self.facing_downhill(False):
            return False
        limits = {
            C.SURFACE_CLASS_VERY_SLIPPERY: 0.9659258,   # cos(15 deg)
            C.SURFACE_CLASS_SLIPPERY: 0.9396926,        # cos(20 deg)
        }
        limit = limits.get(self.get_floor_class(), 0.8660254)  # cos(30 deg)
        return self.floor.normal[1] <= limit

    def facing_downhill(self, turn_yaw):
        """Whether Mario's facing angle points down the slope he is on."""
        face_angle_yaw = self.face_angle[1]

        # A standing Mario is treated as facing the way the stick points.
        if turn_yaw and self.forward_vel < 0.0:
            face_angle_yaw += 0x8000

        face_angle_yaw = s16(self.floor_angle - face_angle_yaw)
        return -0x4000 < face_angle_yaw < 0x4000

    def get_slope_steepness(self):
        if self.floor is None:
            return 0.0
        nx, _, nz = self.floor.normal
        return math.sqrt(nx * nx + nz * nz)

    # -- input --------------------------------------------------------------

    def update_inputs(self):
        self.particle_flags = 0
        self.input = 0

        self._update_button_inputs()
        self._update_joystick_inputs()
        self.update_geometry_inputs()

        if not (self.input & (C.INPUT_NONZERO_ANALOG | C.INPUT_A_PRESSED)):
            self.input |= C.INPUT_UNKNOWN_5

        if self.wall_kick_timer > 0:
            self.wall_kick_timer -= 1
        if self.double_jump_timer > 0:
            self.double_jump_timer -= 1

    def _update_button_inputs(self):
        ctrl = self.controller

        if ctrl.button_pressed & C.A_BUTTON:
            self.input |= C.INPUT_A_PRESSED
        if ctrl.button_down & C.A_BUTTON:
            self.input |= C.INPUT_A_DOWN
        if ctrl.button_pressed & C.B_BUTTON:
            self.input |= C.INPUT_B_PRESSED
        if ctrl.button_down & C.Z_TRIG:
            self.input |= C.INPUT_Z_DOWN
        if ctrl.button_pressed & C.Z_TRIG:
            self.input |= C.INPUT_Z_PRESSED

        # Saturating counters, used for jump and dive timing windows.
        if self.input & C.INPUT_A_PRESSED:
            self.framesSinceA = 0
        elif self.framesSinceA < 0xFF:
            self.framesSinceA += 1

        if self.input & C.INPUT_B_PRESSED:
            self.framesSinceB = 0
        elif self.framesSinceB < 0xFF:
            self.framesSinceB += 1

    def _update_joystick_inputs(self):
        ctrl = self.controller
        # Squaring the magnitude gives the stick its non-linear response.
        mag = ((ctrl.stick_mag / 64.0) ** 2) * 64.0

        if self.squish_timer == 0:
            self.intended_mag = mag / 2.0
        else:
            self.intended_mag = mag / 8.0

        if self.intended_mag > 0.0:
            self.intended_yaw = s16(
                atan2s(-ctrl.stick_y, ctrl.stick_x) + self.camera_yaw
            )
            self.input |= C.INPUT_NONZERO_ANALOG
        else:
            self.intended_yaw = self.face_angle[1]

    def update_geometry_inputs(self):
        from .steps import find_wall_collisions

        # Two passes at different heights so Mario is pushed clear of walls
        # around both his torso and his feet.
        find_wall_collisions(self, self.pos, 60.0, 50.0)
        find_wall_collisions(self, self.pos, 30.0, 24.0)

        self.floor_height, self.floor = self.surfaces.find_floor(*self.pos)

        # Out of bounds: retry from the graphical position, which was not
        # advanced this frame.
        if self.floor is None:
            self.pos = list(self.gfx_pos)
            self.floor_height, self.floor = self.surfaces.find_floor(*self.pos)

        self.ceil_height, self.ceil = self.find_ceil(self.pos, self.floor_height)

        # Resolved before the tests below, two of which read it. Areas with no
        # water leave this far underground so those tests stay false.
        level = self.surfaces.find_water_level(self.pos[0], self.pos[2])
        self.water_level = NO_WATER if level is None else level

        if self.floor is not None:
            self.floor_angle = atan2s(self.floor.normal[2], self.floor.normal[0])

            if self.pos[1] > self.water_level - 40 and self.floor_is_slippery():
                self.input |= C.INPUT_ABOVE_SLIDE

            if self.pos[1] > self.floor_height + 100.0:
                self.input |= C.INPUT_OFF_FLOOR

            if self.pos[1] < self.water_level - 10:
                self.input |= C.INPUT_IN_WATER

    def find_ceil(self, pos, floor_height):
        """Ceiling above `pos`, searched from just above the floor.

        Starting the search at the floor rather than at Mario keeps a ceiling
        he has already passed under from blocking him.
        """
        x, y, z = pos
        if floor_height + 80.0 <= y:
            return self.surfaces.find_ceil(x, y, z)
        return self.surfaces.find_ceil(x, floor_height + 80.0, z)

    # -- helpers used by actions -------------------------------------------

    def set_forward_vel(self, forward_vel):
        self.forward_vel = forward_vel
        self.slide_vel_x = forward_vel * sins(self.face_angle[1])
        self.slide_vel_z = forward_vel * coss(self.face_angle[1])
        self.vel[0] = self.slide_vel_x
        self.vel[2] = self.slide_vel_z

    def set_y_vel_based_on_fspeed(self, initial_vel_y, multiplier):
        """Jump height scales with running speed."""
        self.vel[1] = initial_vel_y + self.forward_vel * multiplier
        if self.squish_timer != 0 or self.quicksand_depth > 1.0:
            self.vel[1] *= 0.5

    # -- how an enemy touch lands -------------------------------------------
    #
    # Reached from sm64py/objects.py, which resolves the player against every
    # enemy and needs to put him into a reaction. It used to name Mario's
    # actions directly; it cannot any more, because the Hero is playable and
    # his machine has never heard of ACT_BACKWARD_AIR_KB. Naming the *reaction*
    # rather than the action leaves each character to answer in its own
    # vocabulary, and leaves Mario's answer exactly what it was.

    def bounce_off_enemy(self, velocity):
        from .actions import set_mario_action
        # Action first, velocity second: entering an airborne action sets
        # vel[1] itself, so assigning the bounce before the transition just
        # gets overwritten.
        set_mario_action(self, C.ACT_JUMP, 0)
        self.vel[1] = velocity

    def take_enemy_hit(self, away_yaw, speed, velocity):
        from .actions import set_mario_action
        self.face_angle[1] = away_yaw
        self.set_forward_vel(-speed)
        self.vel[1] = velocity
        set_mario_action(self, C.ACT_BACKWARD_AIR_KB, 0)

    def sync_graphics(self):
        self.gfx_pos = list(self.pos)
        # On land only the yaw is drawn: Mario stays upright however steep the
        # slope. Swimming aims his whole body along the direction he is
        # heading, so pitch and roll are drawn too. The pitch is negated
        # because a positive face pitch means swimming upward.
        if self.action & C.ACT_FLAG_SWIMMING:
            self.gfx_angle = [-self.face_angle[0], self.face_angle[1],
                              self.face_angle[2]]
        else:
            self.gfx_angle = [0, self.face_angle[1], 0]
