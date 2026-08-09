"""Panda3D front end: scene, input, HUD.

All the behaviour lives in world/gravity/movement; this module only draws what
they produce and feeds them input.
"""

from direct.showbase.ShowBase import ShowBase
from direct.gui.OnscreenText import OnscreenText
from panda3d.core import (
    AmbientLight,
    LQuaternion,
    NodePath,
    PointLight,
    TextNode,
    Vec4,
    WindowProperties,
    loadPrcFileData,
)

from .constants import CharacterVariables
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


def configure():
    loadPrcFileData(
        "",
        "\n".join(
            [
                "window-title Outer Wilds Player Controller -- Panda3D",
                "win-size 1280 720",
                "framebuffer-multisample 1",
                "multisamples 4",
                "sync-video 1",
                "text-minfilter linear",
            ]
        ),
    )


class OuterWildsApp(ShowBase):
    def __init__(self, variables=None):
        ShowBase.__init__(self)
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
        self.taskMgr.add(self._update, "ow-update")

    # -- scene -------------------------------------------------------------

    def _build_scene(self):
        self.planet_nodes = []
        for definition, body in zip(self.world.definitions, self.world.planets):
            node = NodePath(make_sphere(definition.visual_radius, 48, 24, definition.color))
            node.reparentTo(self.render)
            node.setPos(*body.position)
            if definition.emissive:
                node.setLightOff()
                node.setColorScale(1.6, 1.4, 1.0, 1.0)
            self.planet_nodes.append(node)

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
        dt = min(self.clock.getDt(), 0.25)
        self._gather_input()
        self.world.advance(dt)
        self._sync_scene()
        if self.show_hud:
            self._update_hud()
        return task.cont

    def _sync_scene(self):
        for node, body in zip(self.planet_nodes, self.world.planets):
            node.setPos(*body.position)
        self.sun_light_np.setPos(*self.world.planets[-1].position)

        player = self.world.player
        self.camera.setPos(*player.position)
        self.camera.setQuat(LQuaternion(self.world.movement.camera_quat))
        self.starfield.setPos(*player.position)

    def _update_hud(self):
        world = self.world
        movement = world.movement
        nearest, gap = world.nearest_planet()

        speed_ms = world.player_speed / 100.0
        gap_m = (gap or 0.0) / 100.0
        # Gravity only -- acceleration would fold in jetpack thrust.
        pull = world.player.gravity_force.length() / (world.player.mass_self or 1.0) / 100.0

        lines = [
            "speed      {:8.1f} m/s".format(speed_ms),
            "nearest    {:>8}  {:.0f} m".format(nearest.name if nearest else "-", gap_m),
            "gravity    {:8.2f} m/s^2".format(pull),
            "roll       {}".format("ON" if movement.is_rolling else "off"),
            "zero-g     {}".format("on" if world.player.is_zero_g else "OFF"),
            "n-body     {}".format(
                "planets attract" if world.gravity.planets_attract_each_other
                else "planets static (as shipped)"
            ),
            "",
            "WASD thrust   space/ctrl up-down   shift brake",
            "mouse look    Q+mouse roll         R recentre view",
            "F1 hud  F2 zero-g  F3 n-body  Esc release mouse",
        ]
        self.hud.setText("\n".join(lines))
