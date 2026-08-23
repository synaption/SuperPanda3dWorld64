"""Panda3D front end: scene, input, HUD.

All the behaviour lives in world/gravity/movement; this module only draws what
they produce and feeds them input.
"""

from collections import deque
from pathlib import Path

from direct.showbase.ShowBase import ShowBase
from direct.gui.OnscreenText import OnscreenText
from panda3d.core import (
    AmbientLight,
    ClockObject,
    Filename,
    NodePath,
    PointLight,
    TextNode,
    Vec4,
    WindowProperties,
    loadPrcFileData,
)

from .constants import GRAVITY_ALL, GRAVITY_NEAREST, CharacterVariables
from .geometry import make_sphere, make_starfield
from .world import World

#: Degrees of look per pixel of mouse movement.
MOUSE_SENSITIVITY = 0.12

# The scene spans ~1e6 cm, so the near plane wants to be as far out as it can
# be -- but no further than the player's own collision radius (32 cm), or the
# ground clips away the moment you land on something and stand on it.
NEAR_PLANE = 10.0
FAR_PLANE = 1.6e6
#: Stars draw depth-test-off in the background bin, so this only has to be
#: inside the far plane; it is not a real distance.
STARFIELD_RADIUS = 8.0e5
PERFORMANCE_WINDOW_SECONDS = 10.0
FRAME_RATE_CAP = 120.0

# planet_gen authors its mesh in metres around a 300 m sea-level radius.  The
# OW port uses centimetres, but the per-body scale below is a ratio, so no unit
# conversion is needed.
PLANET_GEN_RADIUS = 300.0
PLANET_MODEL = (
    Path(__file__).resolve().parent.parent
    / "planet_gen" / "out" / "planet_lod1.glb"
)


def configure():
    loadPrcFileData(
        "",
        "\n".join(
            [
                "window-title Outer Wilds Player Controller -- Panda3D",
                "win-size 1280 720",
                "framebuffer-multisample 1",
                "multisamples 4",
                "sync-video 0",
                "clock-mode limited",
                "clock-frame-rate 120",
                "text-minfilter linear",
            ]
        ),
    )


class OuterWildsApp(ShowBase):
    def __init__(self, variables=None):
        ShowBase.__init__(self)
        # Explicitly configure the global clock as well as the PRC defaults:
        # the clock exists before configure() runs in some embedding contexts.
        self.clock.setMode(ClockObject.MLimited)
        self.clock.setFrameRate(FRAME_RATE_CAP)
        self.disableMouse()
        self.setBackgroundColor(0.015, 0.017, 0.03, 1.0)

        self.world = World(variables=variables or CharacterVariables())

        self.camLens.setNear(NEAR_PLANE)
        self.camLens.setFar(FAR_PLANE)
        self.camLens.setFov(75)

        self._build_scene()
        self._build_hud()
        self._bind_input()

        self.mouse_captured = False
        self._previous_pointer = None
        self._capture_mouse(True)

        self.show_hud = True
        # Store real render-frame durations, rather than the capped duration
        # handed to the simulation, so the HUD can accurately report hitches.
        self._frame_times = deque()
        self._performance_clock = 0.0
        self._last_frame_time = 0.0
        self.taskMgr.add(self._update, "ow-update")

    # -- scene -------------------------------------------------------------

    def _build_scene(self):
        self.planet_nodes = []
        # Loader accepts native paths on POSIX, but a Windows Python launched
        # from a WSL share sees this as a UNC path (\\wsl.localhost\...).
        # Panda's VFS represents that path as /hosts/wsl.localhost/...; doing
        # the conversion explicitly keeps Loader from treating the backslashes
        # as part of a model-path filename.
        planet_filename = Filename.fromOsSpecific(str(PLANET_MODEL))
        terrain = self.loader.loadModel(planet_filename)
        if terrain.isEmpty():
            raise RuntimeError(
                "planet_gen mesh is missing or unreadable: {}".format(PLANET_MODEL)
            )

        for index, (definition, body) in enumerate(
            zip(self.world.definitions, self.world.planets)
        ):
            if definition.emissive:
                # The generated asset is rocky terrain.  Keep the system's
                # light source round and emissive rather than making it look
                # like another shaded planet.
                node = NodePath(
                    make_sphere(
                        definition.visual_radius, 48, 24, definition.color
                    )
                )
                node.reparentTo(self.render)
            else:
                node = terrain.copyTo(self.render)
                node.setScale(definition.visual_radius / PLANET_GEN_RADIUS)
                # Every body shares the authored terrain, but a stable rotation
                # keeps their silhouettes and landmarks from lining up.
                node.setHpr(index * 137.5 % 360.0, index * 47.0 % 360.0, 0.0)
                node.setColorScale(*definition.color)
            node.setPos(*body.position)
            if definition.emissive:
                node.setLightOff()
                node.setColorScale(1.6, 1.4, 1.0, 1.0)
            self.planet_nodes.append(node)

        terrain.removeNode()

        # Stars ride with the camera's position but not its rotation, so they
        # read as infinitely far away.
        self.starfield = NodePath(make_starfield(2200, STARFIELD_RADIUS))
        self.starfield.reparentTo(self.render)
        self.starfield.setLightOff()
        self.starfield.setBin("background", 0)
        self.starfield.setDepthWrite(False)
        self.starfield.setDepthTest(False)
        self.starfield.setRenderModeThickness(1.6)

        sun_definition = self.world.definitions[-1]
        sun_light = PointLight("sun")
        sun_light.setColor(Vec4(1.0, 0.95, 0.85, 1.0))
        # Constant attenuation: the demo system is small enough that a falloff
        # would leave the outer planets unlit.
        sun_light.setAttenuation((1.0, 0.0, 0.0))
        self.sun_light_np = self.render.attachNewNode(sun_light)
        self.sun_light_np.setPos(*sun_definition.position)
        self.render.setLight(self.sun_light_np)

        ambient = AmbientLight("ambient")
        ambient.setColor(Vec4(0.10, 0.11, 0.15, 1.0))
        self.render.setLight(self.render.attachNewNode(ambient))

    def _build_hud(self):
        self.hud = OnscreenText(
            text="",
            pos=(-1.72, 0.92),
            scale=0.042,
            fg=(0.85, 0.92, 1.0, 1.0),
            align=TextNode.ALeft,
            mayChange=True,
            font=self.loader.loadFont("cmtt12.egg"),
        )
        self.reticle = OnscreenText(
            text="+",
            pos=(0, -0.012),
            scale=0.06,
            fg=(1.0, 1.0, 1.0, 0.55),
            align=TextNode.ACenter,
            mayChange=False,
        )

    # -- input -------------------------------------------------------------

    def _bind_input(self):
        self.keys = {}
        bindings = {
            "w": "forward", "s": "back", "a": "left", "d": "right",
            "arrow_up": "forward", "arrow_down": "back",
            "arrow_left": "left", "arrow_right": "right",
            "space": "up", "lcontrol": "down", "control": "down",
            "lshift": "brake", "shift": "brake",
            "q": "roll",
        }
        for key, action in bindings.items():
            self.keys[action] = False
            self.accept(key, self._set_key, [action, True])
            self.accept(key + "-up", self._set_key, [action, False])

        self.accept("escape", self._on_escape)
        self.accept("f1", self._toggle_hud)
        self.accept("f2", self._toggle_zero_g)
        self.accept("f3", self._toggle_planet_attraction)
        self.accept("f4", self._toggle_gravity_mode)
        self.accept("r", self._recentre_camera)
        self.accept("mouse1", self._capture_mouse, [True])

    def _set_key(self, action, value):
        self.keys[action] = value

    def _on_escape(self):
        if self.mouse_captured:
            self._capture_mouse(False)
        else:
            self.userExit()

    def _toggle_hud(self):
        self.show_hud = not self.show_hud
        (self.hud.show if self.show_hud else self.hud.hide)()

    def _toggle_zero_g(self):
        enabled = not self.world.player.is_zero_g
        self.world.gravity.set_zero_g(enabled)

    def _toggle_planet_attraction(self):
        self.world.gravity.planets_attract_each_other = (
            not self.world.gravity.planets_attract_each_other
        )

    def _toggle_gravity_mode(self):
        self.world.gravity_mode = (
            GRAVITY_ALL if self.world.gravity_mode == GRAVITY_NEAREST else GRAVITY_NEAREST
        )

    def _recentre_camera(self):
        self.world.movement.snap_camera()

    def _capture_mouse(self, captured):
        # Offscreen buffers have no cursor or pointer to capture; the sim still
        # runs, so degrade rather than fail.
        if not hasattr(self.win, "requestProperties"):
            self.mouse_captured = False
            return
        props = WindowProperties()
        props.setCursorHidden(captured)
        props.setMouseMode(
            WindowProperties.M_relative if captured else WindowProperties.M_absolute
        )
        self.win.requestProperties(props)
        self.mouse_captured = captured
        self._previous_pointer = None

    def _read_mouse_delta(self):
        """Pointer delta in pixels, for either mouse mode.

        M_relative is requested but not honoured everywhere; when it isn't,
        fall back to reading an absolute position and recentring the pointer.
        """
        if not self.mouse_captured or not self.win.hasPointer(0):
            return 0.0, 0.0
        pointer = self.win.getPointer(0)
        if not pointer.getInWindow() and not self._is_relative_mode():
            return 0.0, 0.0
        x, y = pointer.getX(), pointer.getY()

        if self._is_relative_mode():
            if self._previous_pointer is None:
                self._previous_pointer = (x, y)
                return 0.0, 0.0
            dx = x - self._previous_pointer[0]
            dy = y - self._previous_pointer[1]
            self._previous_pointer = (x, y)
            return dx, dy

        cx = self.win.getXSize() // 2
        cy = self.win.getYSize() // 2
        dx, dy = x - cx, y - cy
        if self.win.movePointer(0, cx, cy):
            return dx, dy
        return 0.0, 0.0

    def _is_relative_mode(self):
        if not hasattr(self.win, "getProperties"):
            return False
        return (
            self.win.getProperties().getMouseMode() == WindowProperties.M_relative
        )

    def _gather_input(self):
        state = self.world.input
        keys = self.keys
        state.move = (
            (1.0 if keys["right"] else 0.0) - (1.0 if keys["left"] else 0.0),
            (1.0 if keys["forward"] else 0.0) - (1.0 if keys["back"] else 0.0),
        )
        state.up_down = (1.0 if keys["up"] else 0.0) - (1.0 if keys["down"] else 0.0)
        state.brake = keys["brake"]
        state.roll = keys["roll"]

        dx, dy = self._read_mouse_delta()
        # Screen Y grows downward; negate so moving the mouse forward pitches up.
        state.look_impulse = (
            state.look_impulse[0] + dx * MOUSE_SENSITIVITY,
            state.look_impulse[1] - dy * MOUSE_SENSITIVITY,
        )

    # -- frame -------------------------------------------------------------

    def _update(self, task):
        frame_dt = self.clock.getDt()
        self._record_frame_time(frame_dt)
        dt = min(frame_dt, 0.25)
        self._gather_input()
        self.world.advance(dt)
        self._sync_scene()
        if self.show_hud:
            self._update_hud()
        return task.cont

    def _record_frame_time(self, frame_dt):
        """Keep frame times in a rolling ten-second history for the HUD."""
        # Clock dt should never be negative, but avoid letting a bad clock
        # sample make the rolling-window timestamps move backward.
        frame_dt = max(frame_dt, 0.0)
        self._last_frame_time = frame_dt
        self._performance_clock += frame_dt
        self._frame_times.append((self._performance_clock, frame_dt))

        cutoff = self._performance_clock - PERFORMANCE_WINDOW_SECONDS
        while self._frame_times and self._frame_times[0][0] < cutoff:
            self._frame_times.popleft()

    def _performance_hud_lines(self):
        frame_dt = self._last_frame_time
        fps = 1.0 / frame_dt if frame_dt > 0.0 else 0.0
        slowest_ms = max(dt for _, dt in self._frame_times) * 1000.0
        slowest_fps = 1000.0 / slowest_ms if slowest_ms > 0.0 else 0.0
        return [
            "frame rate {:8.1f} FPS".format(fps),
            "worst frame{:7.1f} ms ({:5.1f} FPS, last 10 s)".format(
                slowest_ms, slowest_fps
            ),
        ]

    def _sync_scene(self):
        for node, body in zip(self.planet_nodes, self.world.planets):
            node.setPos(*self.world.interpolated_position(body))
        self.sun_light_np.setPos(*self.world.interpolated_position(self.world.planets[-1]))

        player = self.world.player
        player_position = self.world.interpolated_position(player)
        self.camera.setPos(*player_position)
        self.camera.setQuat(self.world.interpolated_camera_quat())
        self.starfield.setPos(*player_position)

    def _update_hud(self):
        world = self.world
        movement = world.movement
        nearest, gap = world.nearest_planet()

        speed_ms = world.player_speed / 100.0
        gap_m = (gap or 0.0) / 100.0
        # Gravity only -- acceleration would fold in jetpack thrust.
        pull = world.player.gravity_force.length() / (world.player.mass_self or 1.0) / 100.0

        grounded = movement.grounded
        lines = [
            *self._performance_hud_lines(),
            "",
            "mode       {}".format(
                "ON FOOT -- {}".format(movement.ground_body.name)
                if grounded else "flying"),
            "speed      {:8.1f} m/s".format(speed_ms),
            "nearest    {:>8}  {:.0f} m".format(nearest.name if nearest else "-", gap_m),
            "gravity    {:8.2f} m/s^2  ({})".format(
                pull, "nearest body" if world.gravity_mode == GRAVITY_NEAREST
                else "all bodies, as shipped"),
            "zero-g     {}".format("on" if world.player.is_zero_g else "OFF"),
            "n-body     {}".format(
                "planets attract" if world.gravity.planets_attract_each_other
                else "planets static (as shipped)"
            ),
            "",
        ]
        if grounded:
            lines += [
                "WASD walk     space jump            mouse look",
                "F1 hud  F2 zero-g  F3 n-body  F4 gravity  Esc release mouse",
            ]
        else:
            lines += [
                "WASD thrust   space/ctrl up-down   shift brake",
                "mouse look    Q+mouse roll         R recentre view",
                "F1 hud  F2 zero-g  F3 n-body  F4 gravity  Esc release mouse",
            ]
        self.hud.setText("\n".join(lines))
