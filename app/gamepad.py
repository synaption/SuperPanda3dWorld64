"""Gamepad input, polled once a frame and handed to the game as a snapshot.

Panda3D can throw button events for a device attached to the data graph, but
none of the game's input works that way: `Game._poll_controller` reads a set of
held keys every 30 Hz tick, and the analog stick has no event to be thrown for
at all. So this polls instead, and the one thing it keeps state for is the pair
of latching controls -- the skates and the character swap -- where the press
matters rather than the hold, and where the tick loop cannot do the edge
detection itself because a single frame may run two ticks or none.

The snapshot is taken once per rendered frame, which is deliberate: reading the
device again between the two ticks of a slow frame would be reading the same
values anyway, and `Controller.set_buttons` would see one press across two
ticks and correctly report `A_PRESSED` on the first.

Nothing here is required. With no pad plugged in every field reads as neutral,
the keyboard is untouched, and a pad connected mid-game is picked up by the
`connect-device` event ShowBase throws when it notices one.
"""

from direct.showbase.DirectObject import DirectObject
from panda3d.core import GamepadButton, InputDevice

from sm64py.mario import constants as C

# How far a stick has to leave centre before it counts. Sticks rest a little
# off zero and the rest wears with use, and the game's own 0.1 deadzone in
# `Controller.set_stick` is there to catch a keyboard's diagonals rather than a
# worn stick. Past the edge the magnitude is rescaled from 0 rather than
# stepping straight to the deadzone value, so a slow walk is still available.
STICK_DEADZONE = 0.18
# Tighter than it was on the look stick, not wider. A quarter of the travel is
# a lot to give away on the control that aims, and the response curve in
# `FollowCamera.look_stick` already makes the first part of what is left move
# the view slowly: the deadzone only has to cover the rest the stick has, not
# stand in for a curve.
CAMERA_DEADZONE = 0.12

# Analog triggers report as an axis rather than a button on most drivers, and
# as both on some. Half pressed is pressed.
TRIGGER_THRESHOLD = 0.5

# The left trigger is the jetpack, and it is read as held rather than as an
# amount: the boosters have one thrust and the action code takes a button.
# Lower than the threshold above because this one is a control you fly with --
# a finger resting on the trigger should not be a finger flying, but touching
# it should be immediate.
THRUST_THRESHOLD = 0.3


def _apply_deadzone(x, y, deadzone):
    """Rescale a stick past its deadzone, keeping the direction it points.

    Taken on the magnitude rather than per axis, which is what keeps the gate
    circular: a per-axis deadzone squares off the centre and makes a gentle
    diagonal impossible to hold.
    """
    mag = (x * x + y * y) ** 0.5
    if mag <= deadzone:
        return 0.0, 0.0
    scale = min((mag - deadzone) / (1.0 - deadzone), 1.0) / mag
    return x * scale, y * scale


class Gamepad(DirectObject):
    """The first gamepad the system reports, if there is one.

    One pad, not several: this is a single-player game with one `Controller`
    behind it, and a second pad would have nothing to drive.
    """

    def __init__(self, devices=None):
        self.device = None
        # Held controls, as the game reads them.
        self.stick = (0.0, 0.0)
        self.camera = (0.0, 0.0)
        self.buttons = 0
        self.zombie = False
        self.recenter = False
        # The jetpack, off the left trigger.
        self.thrust = False
        # Edges, cleared by the next poll: see `pressed` and `released`.
        self._pressed = set()
        self._released = set()
        self._held = set()

        self._devices = devices
        if devices is not None:
            for device in devices.get_devices(InputDevice.DeviceClass.gamepad):
                self._adopt(device)
                break

        # Hot plug. ShowBase asks the device manager for new arrivals every
        # frame and throws these, so a pad plugged in mid-game is picked up
        # without the player having to restart to be noticed.
        self.accept("connect-device", self._connected)
        self.accept("disconnect-device", self._disconnected)

    # -- the device ---------------------------------------------------------

    @property
    def connected(self):
        return self.device is not None

    @property
    def name(self):
        return self.device.name if self.device is not None else "none"

    def _adopt(self, device):
        self.device = device
        print(f"Gamepad: {device.name}")

    def _connected(self, device):
        if self.device is None and device.device_class == InputDevice.DeviceClass.gamepad:
            self._adopt(device)

    def _disconnected(self, device):
        if device is not self.device:
            return
        self.device = None
        self._neutral()
        print("Gamepad: disconnected")
        # A pad unplugged while a second one is attached should fall through to
        # that one rather than leaving the player with nothing.
        if self._devices is not None:
            for other in self._devices.get_devices(InputDevice.DeviceClass.gamepad):
                self._adopt(other)
                break

    def _neutral(self):
        """Everything centred and released.

        Called when the pad goes away and when the console takes the input, so
        a direction held at that moment does not stay held forever: there is no
        release to arrive later the way there is for a key.
        """
        self.stick = (0.0, 0.0)
        self.camera = (0.0, 0.0)
        self.buttons = 0
        self.zombie = False
        self.recenter = False
        self.thrust = False
        self._pressed.clear()
        self._released.clear()
        self._held.clear()

    # -- reading it ---------------------------------------------------------

    def _axis(self, axis):
        state = self.device.find_axis(axis)
        return state.value if state.known else 0.0

    def _button(self, handle):
        return self.device.find_button(handle).pressed

    def _any_button(self, *handles):
        return any(self._button(handle) for handle in handles)

    def poll(self, active=True):
        """Read the pad. Call once per rendered frame, before anything reads it.

        `active` is false while the console has the input, and holds everything
        neutral without dropping the device.
        """
        if self.device is None:
            return
        if not active:
            self._neutral()
            return

        # A no-op for drivers that keep the device up to date themselves, and
        # the only thing that reads new events on the ones that do not.
        self.device.poll()

        x = self._axis(InputDevice.Axis.left_x)
        y = self._axis(InputDevice.Axis.left_y)
        # The d-pad stands in for the stick when the stick is centred, at full
        # deflection: it is the same control the arrow keys are, and it is what
        # a player reaches for to line up a jump.
        if abs(x) + abs(y) == 0.0:
            x = float(self._button(GamepadButton.dpad_right())) - \
                float(self._button(GamepadButton.dpad_left()))
            y = float(self._button(GamepadButton.dpad_up())) - \
                float(self._button(GamepadButton.dpad_down()))
            self.stick = (x, y)
        else:
            self.stick = _apply_deadzone(x, y, STICK_DEADZONE)

        self.camera = _apply_deadzone(
            self._axis(InputDevice.Axis.right_x),
            self._axis(InputDevice.Axis.right_y),
            CAMERA_DEADZONE,
        )

        buttons = 0
        if self._any_button(GamepadButton.face_a()):
            buttons |= C.A_BUTTON
        # B alone. X used to double as attack, and now carries the squad
        # commands instead -- one button cannot both swing a sword and hold a
        # whistle open, and the squad is the one that needs the hold.
        if self._any_button(GamepadButton.face_b()):
            buttons |= C.B_BUTTON
        if self._triggers_down():
            buttons |= C.Z_TRIG
        self.buttons = buttons

        self.zombie = self._button(GamepadButton.lshoulder())
        self.recenter = self._button(GamepadButton.rshoulder())
        self.thrust = self._thrust_down()

        held = set()
        if self._button(GamepadButton.face_y()):
            held.add("skates")
        if self._button(GamepadButton.start()):
            held.add("swap")
        if self._button(GamepadButton.face_x()):
            held.add("squad")
        # Clicking the right stick is the zoom, and it latches: a stick you
        # have to hold in is a stick you cannot aim with.
        if self._button(GamepadButton.rstick()):
            held.add("zoom")
        self._pressed = held - self._held
        self._released = self._held - held
        self._held = held

    def _triggers_down(self):
        """Z, from whichever of the two things the driver calls a trigger.

        Analog triggers come through as axes on evdev and as buttons on XInput,
        and a pad that reports both would otherwise work on one machine and not
        the other.

        The right one only. Z used to come off either, and the left one is the
        jetpack now -- which cannot also crouch.
        """
        if self._button(GamepadButton.rtrigger()):
            return True
        return self._axis(InputDevice.Axis.right_trigger) > TRIGGER_THRESHOLD

    def _thrust_down(self):
        """The left trigger, from whichever way the driver reports it."""
        if self._button(GamepadButton.ltrigger()):
            return True
        return self._axis(InputDevice.Axis.left_trigger) > THRUST_THRESHOLD

    def pressed(self, name):
        """True on the frame `name` went down, for the controls that latch."""
        return name in self._pressed

    def released(self, name):
        """True on the frame `name` came up.

        The squad button is the one control where the release is a command in
        its own right -- how long it was held is what tells a whistle from an
        order -- so the falling edge is published alongside the rising one.
        Going neutral does not produce one: a button held as the console opens
        was not let go of, and the game cancels the aim itself.
        """
        return name in self._released
