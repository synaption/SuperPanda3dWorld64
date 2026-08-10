"""Exercise the gamepad mapping against a fake pad, with no pad plugged in.

The one thing worth checking about `app/gamepad.py` is the part that is easy to
get backwards and impossible to see in a screenshot: which way the sticks
point once they reach `Controller.set_stick`, whether the deadzone eats a slow
walk, and whether the latching controls report a press once rather than every
frame it is held. None of that needs a device, or a window, so this stands a
stub in for `InputDevice` and reads the same code the game runs.

What it cannot check is that a real pad reports the axes this assumes; that is
the driver's business, and `Gamepad.poll` is written to take either of the two
things drivers do with triggers.

    python3 tools/check_gamepad.py
"""

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from panda3d.core import GamepadButton, InputDevice  # noqa: E402

from app.gamepad import STICK_DEADZONE, Gamepad  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402
from sm64py.mario.state import Controller  # noqa: E402


class FakeDevice:
    """Just enough of `InputDevice` for `Gamepad.poll` to read it."""

    name = "fake pad"
    device_class = InputDevice.DeviceClass.gamepad

    def __init__(self):
        self.axes = {}
        self.down = set()

    def poll(self):
        pass

    def find_axis(self, axis):
        return _State(self.axes.get(axis, 0.0), axis in self.axes)

    def find_button(self, handle):
        return _State(handle.name in self.down, True)

    # -- the fake's own controls --------------------------------------------

    def stick(self, x, y):
        self.axes[InputDevice.Axis.left_x] = x
        self.axes[InputDevice.Axis.left_y] = y

    def press(self, *handles):
        self.down = {handle.name for handle in handles}


class _State:
    """Stands in for both AxisState and ButtonState, which differ only in name."""

    def __init__(self, value, known):
        self.value = value
        self.pressed = bool(value)
        self.known = known


def fresh():
    pad = Gamepad()
    pad.device = FakeDevice()
    return pad, pad.device


def stick_through_controller(pad):
    """What the game does with `pad.stick`, copied from `_poll_controller`."""
    controller = Controller()
    x, y = pad.stick
    controller.set_stick(-x, -y)
    return controller


def check_stick_directions():
    """Forward on the pad has to be forward in the game, and right right.

    The axes are mirrored on the way in (see `_poll_controller`), so this is
    checked at the far end -- what the simulation would read -- rather than at
    `pad.stick`, where a sign error looks perfectly reasonable.
    """
    problems = []
    # Forward is -stick_y at the controller, right is -stick_x.
    for name, (x, y), want in (
        ("forward", (0.0, 1.0), ("y", -1)),
        ("back", (0.0, -1.0), ("y", 1)),
        ("right", (1.0, 0.0), ("x", -1)),
        ("left", (-1.0, 0.0), ("x", 1)),
    ):
        pad, device = fresh()
        device.stick(x, y)
        pad.poll()
        c = stick_through_controller(pad)
        value = c.stick_y if want[0] == "y" else c.stick_x
        if value * want[1] <= 0.0:
            problems.append(f"{name} gave stick_{want[0]} {value:+.0f}")
        if abs(c.stick_mag - 64.0) > 0.01:
            problems.append(f"{name} gave magnitude {c.stick_mag:.1f}, want 64")

    return not problems, ("; ".join(problems) if problems
                          else "forward, back, left and right all arrive intact")


def check_dpad_stands_in_for_the_stick():
    pad, device = fresh()
    device.press(GamepadButton.dpad_up())
    pad.poll()
    up = pad.stick

    # ...and gives way to it the moment the stick moves.
    device.stick(0.0, -1.0)
    pad.poll()
    both = pad.stick

    ok = up == (0.0, 1.0) and both[1] < 0.0
    return ok, f"d-pad up gives {up}, and the stick overrides it: {both}"


def check_deadzone_keeps_a_walk():
    """Past the deadzone the magnitude must rise from 0, not jump to it.

    A deadzone that only zeroes the centre leaves everything past it at its
    raw value, so the first thing the stick can ask for is a fifth of top
    speed -- and since `update_ground_speed` reads the magnitude as a fraction
    of the cap, that is a fifth of a run with no walk underneath it.
    """
    pad, device = fresh()
    problems = []

    device.stick(0.0, STICK_DEADZONE * 0.9)
    pad.poll()
    if pad.stick != (0.0, 0.0):
        problems.append(f"inside the deadzone gave {pad.stick}")

    device.stick(0.0, STICK_DEADZONE + 0.02)
    pad.poll()
    just_past = pad.stick[1]
    if not 0.0 < just_past < 0.1:
        problems.append(f"just past it gave {just_past:.3f}, want a nudge")

    device.stick(0.0, 1.0)
    pad.poll()
    if abs(pad.stick[1] - 1.0) > 1e-6:
        problems.append(f"fully pushed gave {pad.stick[1]:.3f}, want 1.0")

    return not problems, ("; ".join(problems) if problems
                          else f"0 inside {STICK_DEADZONE}, {just_past:.3f} "
                               "just past it, 1.0 at the stop")


def check_buttons():
    problems = []
    for handles, want, name in (
        ((GamepadButton.face_a(),), C.A_BUTTON, "A"),
        ((GamepadButton.face_b(),), C.B_BUTTON, "B"),
        # X is the squad button now, and must not also swing the sword.
        ((GamepadButton.face_x(),), 0, "X, which is no longer B"),
        ((GamepadButton.rtrigger(),), C.Z_TRIG, "Z from the trigger button"),
    ):
        pad, device = fresh()
        device.press(*handles)
        pad.poll()
        if pad.buttons != want:
            problems.append(f"{name} gave 0x{pad.buttons:X}, want 0x{want:X}")

    # The other half of the trigger story: drivers that report it as an axis.
    pad, device = fresh()
    device.axes[InputDevice.Axis.right_trigger] = 0.9
    pad.poll()
    if not pad.buttons & C.Z_TRIG:
        problems.append("a trigger held as an axis did not give Z")

    return not problems, ("; ".join(problems) if problems
                          else "A, B and Z arrive, from either kind of trigger")


def check_latching_controls_press_once():
    """Held is not pressed: the skates would strobe otherwise.

    They are toggles driven from a frame loop, so a control that reported
    itself pressed every frame it was held would flip them sixty times a
    second and land wherever the release happened to fall.
    """
    pad, device = fresh()
    device.press(GamepadButton.face_y())
    pad.poll()
    first = pad.pressed("skates")
    pad.poll()
    held = pad.pressed("skates")
    device.press()
    pad.poll()
    device.press(GamepadButton.face_y())
    pad.poll()
    again = pad.pressed("skates")

    ok = first and not held and again
    return ok, (f"press {first}, still held {held}, pressed again {again}")


def check_the_squad_button_reports_both_edges():
    """X gives a press and, one poll later, a release.

    The squad reads the hold rather than the button: the press starts the aim
    and the release is the command, so a falling edge that never arrived would
    leave the whistle growing forever.
    """
    pad, device = fresh()
    device.press(GamepadButton.face_x())
    pad.poll()
    down = pad.pressed("squad") and not pad.released("squad")
    pad.poll()
    holding = not pad.pressed("squad") and not pad.released("squad")
    device.press()
    pad.poll()
    up = pad.released("squad") and not pad.pressed("squad")

    ok = down and holding and up
    return ok, f"down {down}, held {holding}, up {up}"


def check_console_holds_it_neutral():
    """Everything centred while the console has the input, and no stuck press."""
    pad, device = fresh()
    device.stick(1.0, 1.0)
    device.press(GamepadButton.face_a())
    pad.poll()
    live = pad.stick != (0.0, 0.0) and pad.buttons != 0

    pad.poll(active=False)
    ok = live and pad.stick == (0.0, 0.0) and pad.buttons == 0
    return ok, f"live {live}, then {pad.stick} and 0x{pad.buttons:X}"


def check_no_pad_is_no_input():
    """The keyboard path has to survive there being no device at all."""
    pad = Gamepad()
    pad.poll()
    ok = (not pad.connected and pad.stick == (0.0, 0.0)
          and pad.camera == (0.0, 0.0) and pad.buttons == 0
          and not pad.zombie and not pad.recenter
          and not pad.pressed("skates"))
    return ok, f"unplugged reads neutral, name {pad.name!r}"


CHECKS = [
    ("the stick points where it is pushed", check_stick_directions),
    ("the d-pad stands in for the stick", check_dpad_stands_in_for_the_stick),
    ("the deadzone leaves a walk", check_deadzone_keeps_a_walk),
    ("A, B and Z map to the pad", check_buttons),
    ("latching controls press once", check_latching_controls_press_once),
    ("the squad button reports both edges",
     check_the_squad_button_reports_both_edges),
    ("the console holds the pad neutral", check_console_holds_it_neutral),
    ("no pad is no input", check_no_pad_is_no_input),
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
    raise SystemExit(main())
