"""The debug console: what the game is doing, what it printed, and its numbers.

Dropped down with the backquote key, `~`. It shows three things at once: the
same readout F1 draws, the game's own standard output -- a few hundred lines of
it, walked back with the scroll wheel -- and a command line. The game stops
while it is open, which is the host's doing rather than this module's: `visible`
is what the update loop checks, and `on_toggle` is what tells it to freeze the
clips. Typing the name of a tunable puts a slider
for that number on screen, and the slider *stays* once the console is
dismissed -- which is the point of the whole exercise, since a number worth
tuning is a number you want to move while you are running around rather than
while you are staring at a console.

Standard output is captured by wrapping `sys.stdout` and `sys.stderr`, so
everything the game prints lands here as well as in the terminal. Panda3D's own
notify output is written from C++ and never passes through Python, so it is not
captured; it still goes to the terminal as it always did.

Nothing here knows what a tunable *means*. The game registers them (see
`app/main.py`), giving each a name, a range, and something to read and write --
which for the movement constants is the module attribute the action code reads
every frame, so a drag takes effect on the very next tick.
"""

import sys
import threading
from collections import deque

from direct.gui.DirectGui import DGG, DirectFrame, DirectSlider
from direct.gui.OnscreenText import OnscreenText
from direct.showbase.DirectObject import DirectObject
from panda3d.core import TextNode

# -- the panel, in aspect2d units -------------------------------------------
#
# y runs from -1 to 1, but x runs from -aspect to +aspect, and the aspect is
# whatever shape the window happens to be -- so every horizontal position here
# is a margin from an edge rather than a coordinate, applied in `_layout` and
# reapplied whenever the window is resized.

PANEL_TOP = 1.05
PANEL_BOTTOM = -0.22
MARGIN = 0.04

READOUT_TOP = 0.94
READOUT_SCALE = 0.040

DIVIDER_Y = 0.46

LOG_TOP = 0.40
LOG_SCALE = 0.034
LOG_SPACING = 0.045
LOG_LINES = 12

INPUT_Y = -0.16
INPUT_SCALE = 0.042

# Lines per notch of the wheel.
SCROLL_LINES = 3

# The slider tray, which is drawn whether or not the console is open, and so
# lives below the panel rather than inside it. Positioned from the right edge:
# the rows are built at x <= 0 and the tray itself is moved to the edge.
TRAY_WIDTH = 0.95
TRAY_TOP = -0.34
ROW_HEIGHT = 0.155

# How often the panel is rebuilt while it is open. Assigning to an
# OnscreenText only marks it dirty; the glyphs are regenerated in the following
# cull traversal, and this panel is a lot of glyphs. The same reasoning, and
# the same interval, as the F1 readout.
REFRESH_INTERVAL = 0.1
CURSOR_BLINK = 0.53

COLOURS = {
    "out": (0.86, 0.88, 0.92, 1.0),
    "err": (1.00, 0.48, 0.42, 1.0),
    "cmd": (0.55, 0.85, 1.00, 1.0),
    "info": (0.98, 0.85, 0.45, 1.0),
}


class OutputLog:
    """Every line the game prints, kept so the console can show it back.

    Written to from whichever thread did the printing -- Panda3D's loader
    threads print too -- so the partial-line bookkeeping is done under a lock.
    Lines arrive as arbitrary chunks of text, not as lines, which is what the
    buffering below is for: `print` alone reaches `write` twice, once for the
    text and once for the newline.
    """

    def __init__(self, limit=400):
        self._lines = deque(maxlen=limit)
        self._partial = {}
        self._lock = threading.Lock()
        # Bumped on every change, so a view can tell "nothing was printed" from
        # "the same thing was printed again" without comparing the text.
        self.revision = 0

    def feed(self, text, tag="out"):
        """Take a chunk of stream output and split it into whole lines."""
        with self._lock:
            pieces = (self._partial.get(tag, "") + text).split("\n")
            self._partial[tag] = pieces.pop()
            for line in pieces:
                self._lines.append((tag, line.rstrip("\r")))
            self.revision += 1

    def echo(self, text, tag="info"):
        """Put a line in directly -- the console's own replies come this way."""
        with self._lock:
            for line in str(text).split("\n"):
                self._lines.append((tag, line))
            self.revision += 1

    def _snapshot(self):
        with self._lock:
            lines = list(self._lines)
            for tag, partial in self._partial.items():
                if partial:
                    lines.append((tag, partial))
        return lines

    def tail(self, count):
        """The last `count` lines, with any unterminated one last."""
        return self._snapshot()[-count:]

    def window(self, count, offset=0):
        """`count` lines ending `offset` lines above the newest.

        Returns the lines, how many there are in total, and the offset it
        actually used -- clamped to what exists, so the caller can hand its
        scroll position straight back in and let this be the one place that
        knows how far back the log goes.
        """
        lines = self._snapshot()
        total = len(lines)
        offset = max(0, min(offset, max(0, total - count)))
        end = total - offset
        return lines[max(0, end - count):end], total, offset

    def clear(self):
        with self._lock:
            self._lines.clear()
            self.revision += 1


class _Tee:
    """A stream that writes through to the real one and keeps a copy.

    Deliberately a wrapper rather than a replacement: the terminal is still the
    better place to read a traceback from, and a crash takes the console's
    window with it while the terminal survives.
    """

    def __init__(self, stream, log, tag):
        self._stream = stream
        self._log = log
        self._tag = tag

    def write(self, text):
        written = self._stream.write(text)
        self._log.feed(text, self._tag)
        return written

    def flush(self):
        self._stream.flush()

    def isatty(self):
        return self._stream.isatty()

    def fileno(self):
        return self._stream.fileno()

    def writable(self):
        return True

    @property
    def encoding(self):
        return getattr(self._stream, "encoding", "utf-8")


_CAPTURED = None


def capture_output(limit=400):
    """Start copying stdout and stderr into a log, and return it.

    Call this before the game builds anything, so the console opens with the
    startup output -- what was spawned, what the audio device turned out to be,
    which assets were missing -- already in it. One log for the process, so
    calling it again from wherever the console is finally built hands back the
    log that has been filling up since, rather than a fresh empty one.
    """
    global _CAPTURED
    if _CAPTURED is None:
        _CAPTURED = OutputLog(limit)
    if not isinstance(sys.stdout, _Tee):
        sys.stdout = _Tee(sys.stdout, _CAPTURED, "out")
    if not isinstance(sys.stderr, _Tee):
        sys.stderr = _Tee(sys.stderr, _CAPTURED, "err")
    return _CAPTURED


# -- tunables ---------------------------------------------------------------


class Tunable:
    """One number the console can show, set, and put on a slider.

    The value is not stored here. It is read and written through the getter and
    setter it was built with, so what the slider shows is what the game is
    actually using, even for a number something else has changed since.
    """

    def __init__(self, name, getter, setter, low, high, doc="", integer=False):
        self.name = name
        self._get = getter
        self._set = setter
        self.low = low
        self.high = high
        self.doc = doc
        self.integer = integer
        # What it was tuned to before anybody started dragging, so `reset` can
        # put it back rather than guessing at a sensible value.
        self.default = getter()

    @property
    def value(self):
        return self._get()

    @value.setter
    def value(self, value):
        value = max(self.low, min(self.high, float(value)))
        self._set(int(round(value)) if self.integer else value)

    def reset(self):
        self._set(self.default)

    def format(self, value=None):
        value = self.value if value is None else value
        return f"{int(round(value))}" if self.integer else f"{value:.2f}"


class Tunables:
    """The names the console will answer to, in the order they were added."""

    def __init__(self):
        self._items = {}

    def add(self, name, target, attrs, low, high, doc="", integer=False):
        """Expose one or more attributes of a module or object under one name.

        Several attributes because some numbers only make sense moved
        together: the Hero's top speed is a walking cap and a running cap that
        must not drift apart, and a slider per cap would be a slider for
        something nobody wants to tune.
        """
        if isinstance(attrs, str):
            attrs = (attrs,)

        def getter():
            return getattr(target, attrs[0])

        def setter(value):
            for attr in attrs:
                setattr(target, attr, value)

        tunable = Tunable(name, getter, setter, low, high, doc, integer)
        self._items[name] = tunable
        return tunable

    def __contains__(self, name):
        return name in self._items

    def __getitem__(self, name):
        return self._items[name]

    def __iter__(self):
        return iter(self._items.values())

    def names(self):
        return list(self._items)

    def matching(self, prefix):
        return [n for n in self._items if n.startswith(prefix)]


# -- the console ------------------------------------------------------------


class Console(DirectObject):
    """The panel, the command line, and the sliders it puts on screen.

    Owns its own key bindings, including the backquote that opens it. While it
    is open the game must not act on the keyboard -- otherwise typing
    `run_speed` walks the character across the field and puts the skates on --
    so `visible` is published for the game loop to check, and it is what the
    loop pauses on. `on_toggle` fires on every change for the rest: dropping
    held keys, holding the animation clips still, hiding the readout the panel
    is now drawing itself.
    """

    def __init__(self, base, tunables, log=None, readout=None, on_toggle=None):
        self._base = base
        self.tunables = tunables
        self.log = log if log is not None else capture_output()
        # Called for the readout at the top of the panel; the game owns the
        # text, since it is the same text F1 draws.
        self._readout = readout
        self._on_toggle = on_toggle

        self.visible = False
        self._input = ""
        self._history = []
        self._history_index = 0
        self._refresh_timer = 0.0
        self._blink_timer = 0.0
        self._cursor = True
        # Lines the wheel has scrolled back from the newest, and what the log
        # looked like when it was last drawn.
        self._scroll = 0
        self._drawn = None
        self._total = 0
        self._rows = {}          # tunable name -> slider row
        self._row_order = []
        self._syncing = False    # a slider we are writing to, not the mouse

        font = base.loader.load_font("cmtt12.egg")
        self._font = font if font is not None and font.is_valid() else None

        self._build_panel()
        self._tray = base.aspect2d.attach_new_node("console-sliders")
        self._layout()
        self.accept("aspectRatioChanged", self._layout)

        # ShowBase throws an event per key by name, which is all a game needs,
        # but says nothing about what a key *typed*: shift, the keyboard layout
        # and the difference between `4` and `$` all live in the keystroke
        # event, and nothing turns that on by default.
        # A window that was never opened has no throwers at all, which is how
        # the headless checks reach this.
        for thrower in base.buttonThrowers or ():
            thrower.node().set_keystroke_event("keystroke")

        # All three, because the key printed with a tilde on it arrives under
        # whichever name the platform gives it: the bare button, the button
        # with shift held, or the character it produces with shift held.
        self.accept("`", self.toggle)
        self.accept("shift-`", self.toggle)
        self.accept("asciitilde", self.toggle)

        self._root.hide()

    # -- construction --------------------------------------------------------

    def _text(self, y, scale, colour, parent, align=TextNode.A_left):
        text = OnscreenText(
            text="", pos=(0.0, y), scale=scale, fg=colour, align=align,
            font=self._font, parent=parent, mayChange=True,
        )
        # Remembered with the height it was placed at and the edge it belongs
        # to, so a resize can put it back without disturbing the layout.
        self._panel_text.append((text, y, align))
        return text

    def _build_panel(self):
        self._root = self._base.aspect2d.attach_new_node("console")
        self._panel_text = []

        # Wide enough to cover the screen at any aspect ratio the window can
        # take; nothing is positioned against its edges, so oversizing is free.
        DirectFrame(
            parent=self._root,
            frameColor=(0.03, 0.04, 0.07, 0.86),
            frameSize=(-4.0, 4.0, PANEL_BOTTOM, PANEL_TOP),
        )

        # Created after the background and so drawn over it: aspect2d renders
        # in scene graph order.
        self._readout_text = self._text(
            READOUT_TOP, READOUT_SCALE, (0.88, 0.94, 1.0, 1.0), self._root,
        )

        DirectFrame(
            parent=self._root,
            frameColor=(0.35, 0.45, 0.60, 0.8),
            frameSize=(-4.0, 4.0, DIVIDER_Y, DIVIDER_Y + 0.004),
        )

        # One text node per line rather than one node holding twelve lines, so
        # stderr can be red while stdout is not.
        self._log_text = [
            self._text(LOG_TOP - i * LOG_SPACING, LOG_SCALE,
                       COLOURS["out"], self._root)
            for i in range(LOG_LINES)
        ]

        self._input_text = self._text(
            INPUT_Y, INPUT_SCALE, (1.0, 1.0, 1.0, 1.0), self._root,
        )

        # How far back the wheel has scrolled, against the right edge of the
        # divider. Empty when the newest line is on screen, which is most of
        # the time, and the one thing that says the log has more to show.
        self._scroll_text = self._text(
            DIVIDER_Y + 0.016, 0.030, (0.60, 0.72, 0.88, 1.0), self._root,
            align=TextNode.A_right,
        )

    def _layout(self):
        """Put everything back against the window's edges.

        Called again on every resize, since aspect2d's x range is the window's
        aspect ratio: text laid out for a 4:3 window floats half a screen from
        the edge of a 16:9 one.
        """
        aspect = self._base.get_aspect_ratio()
        for text, y, align in self._panel_text:
            # setTextPos rather than moving the node: OnscreenText keeps its
            # own idea of where the text sits, and setX is on its way out.
            edge = aspect - MARGIN if align == TextNode.A_right else -aspect + MARGIN
            text.setTextPos(edge, y)
        self._tray.set_x(aspect - MARGIN)

    # -- opening and closing -------------------------------------------------

    def toggle(self):
        self.hide() if self.visible else self.show()

    def show(self):
        if self.visible:
            return
        self.visible = True
        self._root.show()
        self._history_index = len(self._history)
        # Only while open, so the arrow keys stay the game's the rest of the
        # time. The keystroke event carries the actual character, shift and
        # keyboard layout already applied, which is why the typing is done
        # from it rather than from the key names.
        self.accept("keystroke", self._type)
        self.accept("backspace", self._backspace)
        self.accept("backspace-repeat", self._backspace)
        self.accept("enter", self._submit)
        self.accept("tab", self._complete)
        self.accept("arrow_up", self._recall, [-1])
        self.accept("arrow_down", self._recall, [1])
        self.accept("wheel_up", self._scroll_by, [SCROLL_LINES])
        self.accept("wheel_down", self._scroll_by, [-SCROLL_LINES])
        # Reopening starts at the newest line; the log has usually moved on.
        self._scroll = 0
        self._force_refresh()
        self._notify()

    def hide(self):
        if not self.visible:
            return
        self.visible = False
        self._root.hide()
        for event in ("keystroke", "backspace", "backspace-repeat", "enter",
                      "tab", "arrow_up", "arrow_down", "wheel_up",
                      "wheel_down"):
            self.ignore(event)
        self._notify()

    def _notify(self):
        if self._on_toggle is not None:
            self._on_toggle(self.visible)

    def wants_mouse(self):
        """Is the pointer busy with the console rather than with the game?

        The game swings the camera on a mouse drag, which would otherwise
        happen underneath every slider drag as well.
        """
        if self.visible:
            return True
        if not self._row_order:
            return False
        mouse = self._base.mouseWatcherNode
        if mouse is None or not mouse.has_mouse():
            return False
        aspect = self._base.get_aspect_ratio()
        x = mouse.get_mouse_x() * aspect
        y = mouse.get_mouse_y()
        right = aspect - MARGIN
        bottom = TRAY_TOP - len(self._row_order) * ROW_HEIGHT
        return (right - TRAY_WIDTH - 0.06 < x < right + 0.06
                and bottom < y < TRAY_TOP + 0.10)

    # -- typing --------------------------------------------------------------

    def _type(self, key):
        """One typed character.

        Printable characters only. Backspace, enter and tab arrive here as
        control codes *and* as named key events, and acting on both would
        delete two characters for one press, so the named events are the ones
        that do the work -- they are the same on every keyboard, which the
        control codes are not.
        """
        if isinstance(key, int):
            key = chr(key)
        # The key that opened the console also arrives as a character, and it
        # is not one anybody needs to type in here.
        if key in ("`", "~") or key < " " or key == "\x7f":
            return
        self._input += key
        self._refresh_input()

    def _backspace(self):
        self._input = self._input[:-1]
        self._refresh_input()

    def _scroll_by(self, lines):
        """Wheel the log back and forward. `window` does the clamping."""
        self._scroll = max(0, self._scroll + lines)
        self._refresh_log()

    def _recall(self, direction):
        """Walk the command history, ending back at a blank line."""
        if not self._history:
            return
        self._history_index = max(
            0, min(len(self._history), self._history_index + direction))
        if self._history_index == len(self._history):
            self._input = ""
        else:
            self._input = self._history[self._history_index]
        self._refresh_input()

    def _complete(self):
        """Finish a tunable name, or list the candidates if it is ambiguous."""
        head, _, prefix = self._input.rpartition(" ")
        matches = self.tunables.matching(prefix)
        if not matches:
            return
        if len(matches) == 1:
            completed = matches[0]
        else:
            # Extend as far as they all agree, then show what is left.
            completed = prefix
            for i in range(len(prefix), len(min(matches, key=len))):
                if len({m[i] for m in matches}) > 1:
                    break
                completed += matches[0][i]
            self.log.echo("  ".join(matches))
        self._input = f"{head} {completed}" if head else completed
        self._force_refresh()

    # -- commands ------------------------------------------------------------

    def _submit(self):
        line = self._input.strip()
        self._input = ""
        # Whatever it prints should be visible, wherever the wheel had got to.
        self._scroll = 0
        if line:
            self.log.echo(f"> {line}", "cmd")
            self._history.append(line)
            self._run(line)
        self._history_index = len(self._history)
        self._force_refresh()

    def _run(self, line):
        words = line.split()
        command, args = words[0].lower(), words[1:]

        if command in ("help", "?"):
            self._help()
        elif command in ("vars", "list"):
            self._list()
        elif command == "clear":
            self.log.clear()
        elif command in ("close", "hide"):
            self._close(args)
        elif command == "reset":
            self._reset(args)
        elif command in ("set", "slider", "var"):
            # The prefixes are optional sugar; `set run_speed 20` and
            # `run_speed 20` are the same command.
            if args:
                self._tunable_command(args[0], args[1:])
            else:
                self.log.echo(f"{command}: needs a name", "err")
        elif command in self.tunables:
            self._tunable_command(command, args)
        else:
            near = self.tunables.matching(command)
            hint = f" -- did you mean {', '.join(near)}?" if near else ""
            self.log.echo(f"unknown: {words[0]}{hint}  (try `help`)", "err")

    def _tunable_command(self, name, args):
        """A bare name shows a slider; a name and a number sets it."""
        if name not in self.tunables:
            self.log.echo(f"no such variable: {name}  (try `vars`)", "err")
            return
        tunable = self.tunables[name]

        if args:
            try:
                requested = float(args[0])
            except ValueError:
                self.log.echo(f"{name}: {args[0]!r} is not a number", "err")
                return
            previous = tunable.value
            tunable.value = requested
            self.log.echo(
                f"{name} = {tunable.format()}  (was {tunable.format(previous)})")
            if not tunable.low <= requested <= tunable.high:
                self.log.echo(
                    f"  clamped to its range {tunable.format(tunable.low)} .. "
                    f"{tunable.format(tunable.high)}")
            self._sync_row(name)
            return

        if name in self._rows:
            self.log.echo(f"{name} = {tunable.format()}  (slider already up)")
            return
        self._add_row(tunable)
        self.log.echo(f"{name} = {tunable.format()}  -- slider added"
                      + (f", {tunable.doc}" if tunable.doc else ""))

    def _help(self):
        self.log.echo(
            "commands:\n"
            "  <name>            put a slider for that variable on screen\n"
            "  <name> <value>    set it outright\n"
            "  vars              every variable, with its value and range\n"
            "  close <name|all>  take a slider away\n"
            "  reset <name|all>  back to the value it started at\n"
            "  clear             empty the log\n"
            "sliders stay up when the console is closed, so you can drag one\n"
            "while playing -- the game is paused for as long as this is open.\n"
            "Tab completes a name, up and down recall commands, the wheel\n"
            "scrolls back through everything the game has printed."
        )

    def _list(self):
        for tunable in self.tunables:
            shown = "  [slider]" if tunable.name in self._rows else ""
            self.log.echo(
                f"  {tunable.name:<18} {tunable.format():>8}"
                f"   [{tunable.format(tunable.low)} .. "
                f"{tunable.format(tunable.high)}]{shown}"
                + (f"   {tunable.doc}" if tunable.doc else "")
            )

    def _close(self, args):
        if args and args[0] == "all":
            for name in list(self._row_order):
                self._remove_row(name)
            self.log.echo("all sliders closed")
        elif args and args[0] in self._rows:
            self._remove_row(args[0])
            self.log.echo(f"{args[0]} slider closed")
        else:
            self.log.echo("close: needs the name of a slider, or `all`", "err")

    def _reset(self, args):
        targets = (list(self.tunables) if args and args[0] == "all"
                   else [self.tunables[a] for a in args if a in self.tunables])
        if not targets:
            self.log.echo("reset: needs a variable name, or `all`", "err")
            return
        for tunable in targets:
            tunable.reset()
            self._sync_row(tunable.name)
            self.log.echo(f"{tunable.name} = {tunable.format()}  (default)")

    # -- sliders -------------------------------------------------------------

    def _add_row(self, tunable):
        """A caption and a slider, hung from the right edge of the screen."""
        name = tunable.name
        holder = self._tray.attach_new_node(f"slider-{name}")

        # Something to read the caption against: the ground in this level is
        # bright sand, and pale text on it is not text.
        DirectFrame(
            parent=holder,
            frameColor=(0.03, 0.04, 0.07, 0.55),
            frameSize=(-TRAY_WIDTH - 0.03, 0.03, -0.055, 0.10),
        )
        label = OnscreenText(
            text="", pos=(-TRAY_WIDTH, 0.045), scale=0.038,
            fg=(1.0, 0.96, 0.80, 1.0), align=TextNode.A_left,
            font=self._font, parent=holder, mayChange=True,
        )
        # Right-aligned and on its own, so a long name and a wide value cannot
        # collide in the middle.
        value = OnscreenText(
            text="", pos=(0.0, 0.045), scale=0.038,
            fg=(1.0, 1.0, 1.0, 1.0), align=TextNode.A_right,
            font=self._font, parent=holder, mayChange=True,
        )
        slider = DirectSlider(
            parent=holder,
            pos=(-TRAY_WIDTH / 2.0, 0, 0),
            scale=TRAY_WIDTH / 2.0,
            range=(tunable.low, tunable.high),
            value=tunable.value,
            pageSize=(tunable.high - tunable.low) / 20.0,
            frameColor=(0.16, 0.19, 0.26, 0.95),
            frameSize=(-1.0, 1.0, -0.045, 0.045),
            thumb_frameColor=(0.55, 0.78, 1.0, 1.0),
            thumb_relief=DGG.FLAT,
            thumb_frameSize=(-0.035, 0.035, -0.075, 0.075),
            command=self._dragged,
            extraArgs=[name],
        )
        self._rows[name] = {"holder": holder, "label": label, "value": value,
                            "slider": slider, "shown": None}
        self._row_order.append(name)
        self._layout_rows()
        self._sync_row(name)

    def _remove_row(self, name):
        row = self._rows.pop(name, None)
        if row is None:
            return
        row["slider"].destroy()
        row["label"].destroy()
        row["value"].destroy()
        row["holder"].remove_node()
        self._row_order.remove(name)
        self._layout_rows()

    def _layout_rows(self):
        for i, name in enumerate(self._row_order):
            self._rows[name]["holder"].set_pos(0, 0, TRAY_TOP - i * ROW_HEIGHT)

    def _dragged(self, name):
        # Fires for our own writes as well as for the mouse; without the guard
        # a programmatic set would be rounded through the slider and written
        # back, so typing an exact value would not stay exact.
        if self._syncing:
            return
        tunable = self.tunables[name]
        tunable.value = self._rows[name]["slider"]["value"]
        self._refresh_label(name)

    def _sync_row(self, name):
        """Put the slider where the value now is, after something else set it."""
        row = self._rows.get(name)
        if row is None:
            return
        self._syncing = True
        try:
            row["slider"]["value"] = self.tunables[name].value
        finally:
            self._syncing = False
        self._refresh_label(name)

    def _refresh_label(self, name):
        """Redraw a slider's caption, but only when its number has changed.

        Text is regenerated in the cull traversal that follows the assignment,
        so a label reassigned every frame costs a frame's worth of glyphs for
        nothing 59 times out of 60.
        """
        row = self._rows[name]
        text = self.tunables[name].format()
        if text != row["shown"]:
            row["shown"] = text
            row["label"].setText(name)
            row["value"].setText(text)

    # -- the frame -----------------------------------------------------------

    def update(self, dt):
        """Called once a frame, open or not."""
        if not self.visible:
            # A slider can still be dragged with the panel down, and DirectGui
            # runs its own mouse handling, so the caption still has to follow.
            for name in self._row_order:
                self._refresh_label(name)
            return

        self._blink_timer -= dt
        if self._blink_timer <= 0.0:
            self._blink_timer = CURSOR_BLINK
            self._cursor = not self._cursor
            self._refresh_input()

        self._refresh_timer -= dt
        if self._refresh_timer <= 0.0:
            self._refresh_timer = REFRESH_INTERVAL
            self._refresh_readout()
            self._refresh_log()

    def _force_refresh(self):
        """Redraw now rather than at the next tick, so typing feels immediate."""
        self._cursor = True
        self._blink_timer = CURSOR_BLINK
        self._refresh_input()
        self._refresh_log()
        self._refresh_readout()

    def _refresh_input(self):
        self._input_text.setText(f"> {self._input}" + ("_" if self._cursor else ""))

    def _refresh_readout(self):
        if self._readout is not None:
            self._readout_text.setText(self._readout())

    def _refresh_log(self):
        lines, total, self._scroll = self.log.window(LOG_LINES, self._scroll)

        # Scrolled back, and the game printed something meanwhile: hold the
        # lines being read still rather than letting new output slide them
        # down. Once the log is full and dropping its oldest line the count
        # stops rising, and the view drifts after all -- by then it is showing
        # something four hundred lines old.
        if self._scroll > 0 and total > self._total:
            self._scroll += total - self._total
            lines, total, self._scroll = self.log.window(LOG_LINES, self._scroll)
        self._total = total

        drawn = (self.log.revision, self._scroll)
        if drawn == self._drawn:
            return
        self._drawn = drawn

        # Bottom-aligned: the newest line always sits just above the prompt,
        # wherever the log is short enough not to fill the panel.
        padding = [None] * (LOG_LINES - len(lines))
        for text, entry in zip(self._log_text, padding + lines):
            if entry is None:
                text.setText("")
                continue
            tag, line = entry
            text.setText(line[:150])
            text.setFg(COLOURS.get(tag, COLOURS["out"]))

        self._scroll_text.setText(
            f"{self._scroll} lines back of {total}   (wheel down for the end)"
            if self._scroll else ""
        )
