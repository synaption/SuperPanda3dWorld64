"""Run Mario's movement system on the castle grounds in Panda3D.

The game logic runs at a fixed 30 Hz, as the original does -- every physics
constant in the action code is per-frame at that rate, so rendering is
decoupled and the simulation is stepped in whole ticks.

Controls:
    W / A / S / D or arrows   analog stick (camera-relative)
    Space                     A -- jump
    Left Shift                B -- punch / dive
    Left Ctrl                 Z -- crouch / ground pound / long jump
    Q / E or mouse drag       swing the camera
    R                         re-centre the camera behind Mario
    F3                        toggle the collision overlay
    F1                        toggle the debug readout
    Escape                    quit
"""

import math
import os
import sys

from direct.showbase.ShowBase import ShowBase
from direct.gui.OnscreenText import OnscreenText
from panda3d.core import (
    AmbientLight,
    ClockObject,
    DirectionalLight,
    Fog,
    TextNode,
    Vec4,
    WindowProperties,
    loadPrcFileData,
)

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from sm64py import surfaces  # noqa: E402
from sm64py.camera import FollowCamera  # noqa: E402
from sm64py.level import load_collision_geometry, load_level_geometry  # noqa: E402
from sm64py.mario import Controller, MarioState, execute_action  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402
from sm64py.math_util import s16, s16_to_degrees, to_panda  # noqa: E402

ASSETS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
CASTLE_GROUNDS = os.path.join(ASSETS, "castle_grounds")

# The game ticks at 30 Hz; all movement constants assume it.
TICK_RATE = 30.0
TICK_DT = 1.0 / TICK_RATE

# Mario spawns where the level script places him, facing the castle.
SPAWN = (-1328.0, 260.0, 4664.0)
SPAWN_YAW = 180.0

# Below the lowest collision in the level; reaching it means Mario is lost.
DEATH_PLANE = -4000.0

ACTION_NAMES = {v: k for k, v in vars(C).items() if k.startswith("ACT_")}

loadPrcFileData("", "window-title SM64 movement in Panda3D")
loadPrcFileData("", "framebuffer-multisample 1")
loadPrcFileData("", "multisamples 4")
loadPrcFileData("", "sync-video 1")


class Game(ShowBase):
    def __init__(self):
        ShowBase.__init__(self)

        self.disable_mouse()
        self.set_background_color(0.32, 0.60, 0.86)

        self.surfaces = surfaces.load(os.path.join(CASTLE_GROUNDS, "collision.npz"))

        self.level = load_level_geometry(os.path.join(CASTLE_GROUNDS, "mesh.npz"))
        self.level.reparent_to(self.render)

        self.collision_view = load_collision_geometry(
            os.path.join(CASTLE_GROUNDS, "collision.npz")
        )
        self.collision_view.reparent_to(self.render)
        self.collision_view.set_render_mode_wireframe()
        self.collision_view.hide()

        self.controller = Controller()
        self.mario = MarioState(self.surfaces, self.controller)
        self.mario.spawn(*SPAWN, SPAWN_YAW)

        self.mario_node = self._build_mario_placeholder()
        self.follow_camera = FollowCamera(self.surfaces, self.mario)

        self.camLens.set_fov(45)
        self.camLens.set_near_far(10, 30000)

        self._setup_lighting()
        self._setup_input()
        self._setup_hud()

        self._accumulator = 0.0
        self._show_debug = True
        self._mouse_anchor = None
        self._reset_interpolation()

        self.task_mgr.add(self._update, "update")

    # -- scene ---------------------------------------------------------------

    def _setup_lighting(self):
        ambient = AmbientLight("ambient")
        ambient.set_color(Vec4(0.55, 0.55, 0.60, 1.0))
        self.render.set_light(self.render.attach_new_node(ambient))

        sun = DirectionalLight("sun")
        sun.set_color(Vec4(0.75, 0.72, 0.65, 1.0))
        sun_np = self.render.attach_new_node(sun)
        sun_np.set_hpr(-40, -60, 0)
        self.render.set_light(sun_np)

        # The level mesh carries baked vertex colour, so keep lighting off it.
        self.level.set_light_off()

        fog = Fog("distance")
        fog.set_color(0.32, 0.60, 0.86)
        fog.set_linear_range(9000, 20000)
        self.render.set_fog(fog)

    def _build_mario_placeholder(self):
        """A stand-in body.

        Mario's real model is a rigged F3D hierarchy with its own animation
        format; until that is imported this is a readable proxy that makes
        position, facing and height unambiguous.
        """
        root = self.render.attach_new_node("mario")

        def block(parent, scale, pos, color):
            node = self.loader.load_model("models/box")
            node.reparent_to(parent)
            node.set_scale(*scale)
            node.set_pos(*pos)
            node.set_texture_off(1)
            node.set_color(*color)
            return node

        body = root.attach_new_node("body")
        # Box model is a unit cube anchored at a corner, hence the offsets.
        block(body, (60, 40, 70), (-30, -20, 0), (0.85, 0.20, 0.20, 1))
        block(body, (60, 40, 45), (-30, -20, 70), (0.20, 0.35, 0.80, 1))
        block(body, (46, 46, 40), (-23, -23, 115), (0.95, 0.78, 0.62, 1))
        block(body, (52, 52, 16), (-26, -26, 150), (0.85, 0.20, 0.20, 1))
        # Nose, so facing is obvious at a glance.
        block(body, (14, 26, 14), (-7, -46, 125), (0.95, 0.78, 0.62, 1))

        return root

    # -- input ---------------------------------------------------------------

    def _setup_input(self):
        self.keys = {}
        bindings = [
            ("w", "up"), ("s", "down"), ("a", "left"), ("d", "right"),
            ("arrow_up", "up"), ("arrow_down", "down"),
            ("arrow_left", "left"), ("arrow_right", "right"),
            ("space", "a"), ("lshift", "b"), ("lcontrol", "z"),
            ("q", "cam_left"), ("e", "cam_right"),
            ("r", "cam_center"),
        ]
        for key, name in bindings:
            self.keys.setdefault(name, False)
            self.accept(key, self._set_key, [name, True])
            self.accept(f"{key}-up", self._set_key, [name, False])

        self.accept("escape", sys.exit)
        self.accept("f1", self._toggle_debug)
        self.accept("f3", self._toggle_collision)

        self._dragging = False
        for button in ("mouse1", "mouse3"):
            self.accept(button, self._set_dragging, [True])
            self.accept(f"{button}-up", self._set_dragging, [False])

        props = WindowProperties()
        props.set_cursor_hidden(False)
        self.win.request_properties(props)

    def _set_key(self, name, value):
        self.keys[name] = value

    def _set_dragging(self, value):
        self._dragging = value

    def _toggle_debug(self):
        self._show_debug = not self._show_debug
        (self.hud.show if self._show_debug else self.hud.hide)()

    def _toggle_collision(self):
        if self.collision_view.is_hidden():
            self.collision_view.show()
            self.level.hide()
        else:
            self.collision_view.hide()
            self.level.show()

    def _setup_hud(self):
        self.hud = OnscreenText(
            text="",
            pos=(-1.3, 0.92),
            scale=0.045,
            fg=(1, 1, 1, 1),
            shadow=(0, 0, 0, 0.8),
            align=TextNode.A_left,
            mayChange=True,
        )

    def _poll_controller(self):
        right = 1.0 if self.keys["right"] else 0.0
        left = 1.0 if self.keys["left"] else 0.0
        up = 1.0 if self.keys["up"] else 0.0
        down = 1.0 if self.keys["down"] else 0.0

        # Both stick axes come out mirrored from screen space: the heading is
        # built as atan2s(-stick_y, stick_x) and then rotated by the camera
        # yaw, which flips Y and mirrors X. Hence left-minus-right, not the
        # other way round.
        self.controller.set_stick(left - right, down - up)

        buttons = 0
        if self.keys["a"]:
            buttons |= C.A_BUTTON
        if self.keys["b"]:
            buttons |= C.B_BUTTON
        if self.keys["z"]:
            buttons |= C.Z_TRIG
        self.controller.set_buttons(buttons)

    # -- loop ----------------------------------------------------------------

    def _update(self, task):
        dt = min(self.clock.get_dt(), 0.25)

        self._update_camera_input(dt)

        # Step the simulation in whole 30 Hz ticks.
        self._accumulator += dt
        steps = 0
        while self._accumulator >= TICK_DT and steps < 8:
            # Remember where Mario was so the render can interpolate out of it.
            self._prev_pos = list(self.mario.gfx_pos)
            self._prev_yaw = self.mario.gfx_angle[1]

            self._poll_controller()
            self.mario.camera_yaw = self.follow_camera.mario_yaw
            execute_action(self.mario)
            self._accumulator -= TICK_DT
            steps += 1

            # Standing in for the death warp: if Mario loses the floor
            # entirely he would otherwise fall forever.
            if self.mario.floor is None or self.mario.pos[1] < DEATH_PLANE:
                self.mario.spawn(*SPAWN, SPAWN_YAW)
                self._reset_interpolation()

        # The simulation only moves in 33 ms steps, so drawing its raw output
        # judders on a faster display. Draw a blend between the last two ticks
        # instead, positioned by however much time is left in the accumulator.
        alpha = min(max(self._accumulator / TICK_DT, 0.0), 1.0)
        pos, yaw = self._interpolated_transform(alpha)

        self.follow_camera.update(dt, target_pos=pos,
                                  recenter=self.keys["cam_center"])
        self.follow_camera.apply_to(self.camera)

        self.mario_node.set_pos(*to_panda(*pos))
        self.mario_node.set_h(s16_to_degrees(yaw))

        if self._show_debug:
            self._update_hud()

        return task.cont

    def _interpolated_transform(self, alpha):
        current = self.mario.gfx_pos
        pos = [
            self._prev_pos[i] + (current[i] - self._prev_pos[i]) * alpha
            for i in range(3)
        ]
        # Turn the short way round, so crossing the angle wrap does not spin.
        delta = s16(self.mario.gfx_angle[1] - self._prev_yaw)
        return pos, self._prev_yaw + delta * alpha

    def _reset_interpolation(self):
        """Drop the blend after a teleport, so Mario does not streak across."""
        self._prev_pos = list(self.mario.gfx_pos)
        self._prev_yaw = self.mario.gfx_angle[1]
        self._accumulator = 0.0

    def _update_camera_input(self, dt):
        speed = 150.0
        if self.keys["cam_left"]:
            self.follow_camera.rotate(-speed * dt)
        if self.keys["cam_right"]:
            self.follow_camera.rotate(speed * dt)

        # Dragging with a mouse button held swings the camera too.
        if self._dragging and self.mouseWatcherNode.has_mouse():
            pos = self.mouseWatcherNode.get_mouse()
            current = (pos.get_x(), pos.get_y())
            if self._mouse_anchor is not None:
                dx = current[0] - self._mouse_anchor[0]
                dy = current[1] - self._mouse_anchor[1]
                self.follow_camera.rotate(dx * 180.0)
                self.follow_camera.tilt(-dy * 0.9)
            self._mouse_anchor = current
        else:
            self._mouse_anchor = None

    def _update_hud(self):
        m = self.mario
        action = ACTION_NAMES.get(m.action, hex(m.action))
        floor_type = f"0x{m.floor.type:04X}" if m.floor else "none"

        # DirectGUI classes keep camelCase, unlike the C++ bindings.
        self.hud.setText(
            f"action   {action}  ({m.anim_name})\n"
            f"pos      {m.pos[0]:8.1f} {m.pos[1]:8.1f} {m.pos[2]:8.1f}\n"
            f"vel      fwd {m.forward_vel:6.2f}   y {m.vel[1]:7.2f}\n"
            f"yaw      {s16_to_degrees(m.face_angle[1]):7.1f} deg\n"
            f"floor    {floor_type}  height {m.floor_height:8.1f}\n"
            f"fps      {self.clock.get_average_frame_rate():5.1f}\n"
            f"\nWASD move   Space jump   Shift dive   Ctrl crouch\n"
            f"Q/E camera  R recentre    F3 collision  F1 hud"
        )


def main():
    missing = [
        p for p in (
            os.path.join(CASTLE_GROUNDS, "collision.npz"),
            os.path.join(CASTLE_GROUNDS, "mesh.npz"),
        ) if not os.path.exists(p)
    ]
    if missing:
        print("Missing converted assets:")
        for path in missing:
            print(f"  {path}")
        print("\nRun the converters first:")
        print("  python3 tools/parse_collision.py "
              "reference/Render96ex/levels/castle_grounds/areas/1/collision.inc.c "
              "assets/castle_grounds/collision.npz")
        print("  python3 tools/parse_f3d.py "
              "reference/Render96ex/levels/castle_grounds 1 "
              "assets/castle_grounds/mesh.npz")
        return 1

    Game().run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
