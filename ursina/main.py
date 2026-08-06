"""Ursina front end for the SM64 movement port.

Ursina uses Panda3D underneath, so the converted level geometry, Actor, and
collision/movement code are shared with the original ``app/main.py``.  This
module only replaces the application, input, camera, and HUD plumbing.
"""

import os
import sys

from panda3d.core import (
    AmbientLight,
    ClockObject,
    DirectionalLight,
    Filename,
    Fog,
    NodePath,
    Vec4,
    loadPrcFileData,
)


PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, PROJECT_ROOT)

# Ursina creates its audio manager while it is being imported.  On WSL,
# probing PulseAudio and ALSA can block for a long time when neither has a
# usable device.  The port does not have sound yet, so choose Panda's null
# backend before importing Ursina instead of paying for doomed device probes.
loadPrcFileData("", "audio-library-name null")

try:
    from ursina import (
        Entity,
        Text,
        Ursina,
        application,
        camera,
        color,
        held_keys,
        mouse,
        scene,
        time,
        window,
    )
except ImportError:
    print("Ursina is not installed. Install it with: python3 -m pip install ursina")
    raise SystemExit(1)

from direct.actor.Actor import Actor

from sm64py import surfaces
from sm64py.camera import FollowCamera
from sm64py.level import load_collision_geometry, load_level_geometry
from sm64py.mario import Controller, MarioState, execute_action
from sm64py.mario import animations
from sm64py.mario import constants as C
from sm64py.math_util import s16, s16_to_degrees


ASSETS = os.path.join(PROJECT_ROOT, "assets")
CASTLE_GROUNDS = os.path.join(ASSETS, "castle_grounds")
MARIO_MODEL = os.path.join(ASSETS, "mario", "mario.glb")

TICK_DT = 1.0 / 30.0
SPAWN = (-1328.0, 260.0, 4664.0)
SPAWN_YAW = 180.0
DEATH_PLANE = -4000.0
MODEL_YAW_OFFSET = 0.0
ACTION_NAMES = {v: k for k, v in vars(C).items() if k.startswith("ACT_")}


def panda_path(path):
    return Filename.from_os_specific(os.path.abspath(path))


def to_ursina(x, y, z):
    """SM64 Y-up coordinates -> Ursina's Panda3D Y-up-left coordinates."""
    return (x, y, -z)


class Game(Entity):
    """An Ursina Entity whose update/input hooks drive the shared game."""

    def __init__(self):
        super().__init__(parent=scene)

        self.surfaces = surfaces.load(os.path.join(CASTLE_GROUNDS, "collision.npz"))
        self.level = load_level_geometry(
            os.path.join(CASTLE_GROUNDS, "mesh.npz"),
            coordinate_transform=to_ursina,
        )
        self.level.reparent_to(scene)

        self.collision_view = load_collision_geometry(
            os.path.join(CASTLE_GROUNDS, "collision.npz"),
            coordinate_transform=to_ursina,
        )
        self.collision_view.reparent_to(scene)
        self.collision_view.set_render_mode_wireframe()
        self.collision_view.hide()

        self.controller = Controller()
        self.mario = MarioState(self.surfaces, self.controller)
        self.mario.spawn(*SPAWN, SPAWN_YAW)
        self.mario_node, self.mario_actor = self._build_mario()
        self._current_anim = None

        self.follow_camera = FollowCamera(self.surfaces, self.mario)
        camera.lens.set_fov(45)
        camera.lens.set_near_far(10, 30000)
        self._setup_lighting()

        self.hud = Text(
            text="",
            x=-0.87,
            y=0.47,
            origin=(-0.5, 0.5),
            scale=0.8,
            color=color.white,
        )
        self._show_debug = True
        self._dragging = False
        self._mouse_anchor = None
        self._accumulator = 0.0
        self._reset_interpolation()

    def _setup_lighting(self):
        ambient = AmbientLight("ambient")
        ambient.set_color(Vec4(0.55, 0.55, 0.60, 1.0))
        scene.set_light(scene.attach_new_node(ambient))

        sun = DirectionalLight("sun")
        sun.set_color(Vec4(0.75, 0.72, 0.65, 1.0))
        sun_node = scene.attach_new_node(sun)
        sun_node.set_hpr(-40, -60, 0)
        scene.set_light(sun_node)
        self.level.set_light_off()

        fog = Fog("distance")
        fog.set_color(0.32, 0.60, 0.86)
        fog.set_linear_range(9000, 20000)
        scene.set_fog(fog)

    def _build_mario(self):
        # A plain Panda3D node is intentional.  An Ursina Entity installs its
        # default shader, which then overrides the glTF Actor's skinning shader
        # and turns the animated model into exploded, rainbow geometry.
        holder = scene.attach_new_node("mario")
        if not os.path.exists(MARIO_MODEL):
            print(f"Mario model not found at {MARIO_MODEL}; using a marker.")
            Entity(
                parent=holder,
                model="cube",
                scale=(50, 50, 150),
                position=(-25, -25, 0),
                color=color.rgb(217, 51, 51),
            )
            return holder, None

        actor = Actor(panda_path(MARIO_MODEL))
        actor.reparent_to(holder)
        actor.set_pos(0, 0, 0)
        actor.set_h(MODEL_YAW_OFFSET)
        return holder, actor

    def _update_animation(self):
        if self.mario_actor is None:
            return
        name, loop, rate = animations.resolve(self.mario)
        if self.mario_actor.get_anim_control(name) is None:
            return
        if name != self._current_anim:
            self._current_anim = name
            (self.mario_actor.loop if loop else self.mario_actor.play)(name)
        # Reapplied every frame, not just on a change: the walk and run cycles
        # follow Mario's current speed rather than a fixed cadence.
        self.mario_actor.set_play_rate(rate, name)

    def input(self, key):
        if key == "escape":
            application.quit()
        elif key == "f1":
            self._show_debug = not self._show_debug
            self.hud.enabled = self._show_debug
        elif key == "f3":
            if self.collision_view.is_hidden():
                self.collision_view.show()
                self.level.hide()
            else:
                self.collision_view.hide()
                self.level.show()
        elif key in ("left mouse down", "right mouse down"):
            self._dragging = True
            self._mouse_anchor = (mouse.x, mouse.y)
        elif key in ("left mouse up", "right mouse up"):
            self._dragging = False
            self._mouse_anchor = None

    @staticmethod
    def _down(*names):
        return any(held_keys[name] for name in names)

    def _poll_controller(self):
        right = float(self._down("d", "right arrow"))
        left = float(self._down("a", "left arrow"))
        up = float(self._down("w", "up arrow"))
        down = float(self._down("s", "down arrow"))
        self.controller.set_stick(left - right, down - up)

        buttons = 0
        if held_keys["space"]:
            buttons |= C.A_BUTTON
        if held_keys["left shift"] or held_keys["shift"]:
            buttons |= C.B_BUTTON
        if held_keys["left control"] or held_keys["control"]:
            buttons |= C.Z_TRIG
        self.controller.set_buttons(buttons)

    def update(self):
        dt = min(float(time.dt), 0.25)
        self._update_camera_input(dt)

        self._accumulator += dt
        steps = 0
        while self._accumulator >= TICK_DT and steps < 8:
            self._prev_pos = list(self.mario.gfx_pos)
            self._prev_yaw = self.mario.gfx_angle[1]
            self._poll_controller()
            self.mario.camera_yaw = self.follow_camera.mario_yaw
            execute_action(self.mario)
            self._accumulator -= TICK_DT
            steps += 1

            if self.mario.floor is None or self.mario.pos[1] < DEATH_PLANE:
                self.mario.spawn(*SPAWN, SPAWN_YAW)
                self._reset_interpolation()

        alpha = min(max(self._accumulator / TICK_DT, 0.0), 1.0)
        pos, yaw = self._interpolated_transform(alpha)
        self.follow_camera.update(
            dt, target_pos=pos, recenter=bool(held_keys["r"])
        )
        self._apply_camera()

        self.mario_node.set_pos(*to_ursina(*pos))
        self.mario_node.set_h(s16_to_degrees(yaw))
        self._update_animation()
        if self._show_debug:
            self._update_hud()

    def _apply_camera(self):
        """Apply the shared camera state through Ursina's Entity API.

        Ursina's Entity ``look_at`` uses its own forward-axis convention.  The
        level and shared coordinate bridge use Panda3D's native convention, so
        explicitly invoke NodePath's implementation for the camera Entity.
        """
        camera.set_pos(*to_ursina(*self.follow_camera.pos))
        target_x, target_y, target_z = to_ursina(
            self.follow_camera.focus[0],
            self.follow_camera.focus[1] + 60.0,
            self.follow_camera.focus[2],
        )
        NodePath.look_at(camera, target_x, target_y, target_z)

    def _interpolated_transform(self, alpha):
        current = self.mario.gfx_pos
        pos = [
            self._prev_pos[i] + (current[i] - self._prev_pos[i]) * alpha
            for i in range(3)
        ]
        delta = s16(self.mario.gfx_angle[1] - self._prev_yaw)
        return pos, self._prev_yaw + delta * alpha

    def _reset_interpolation(self):
        self._prev_pos = list(self.mario.gfx_pos)
        self._prev_yaw = self.mario.gfx_angle[1]
        self._accumulator = 0.0

    def _update_camera_input(self, dt):
        speed = 150.0
        if held_keys["q"]:
            self.follow_camera.rotate(-speed * dt)
        if held_keys["e"]:
            self.follow_camera.rotate(speed * dt)

        if self._dragging:
            current = (mouse.x, mouse.y)
            if self._mouse_anchor is not None:
                dx = current[0] - self._mouse_anchor[0]
                dy = current[1] - self._mouse_anchor[1]
                self.follow_camera.rotate(dx * 180.0)
                self.follow_camera.tilt(-dy * 0.9)
            self._mouse_anchor = current

    def _update_hud(self):
        m = self.mario
        action = ACTION_NAMES.get(m.action, hex(m.action))
        floor_type = f"0x{m.floor.type:04X}" if m.floor else "none"
        fps = ClockObject.get_global_clock().get_average_frame_rate()
        self.hud.text = (
            f"action   {action}  ({m.anim_name})\n"
            f"pos      {m.pos[0]:8.1f} {m.pos[1]:8.1f} {m.pos[2]:8.1f}\n"
            f"vel      fwd {m.forward_vel:6.2f}   y {m.vel[1]:7.2f}\n"
            f"yaw      {s16_to_degrees(m.face_angle[1]):7.1f} deg\n"
            f"floor    {floor_type}  height {m.floor_height:8.1f}\n"
            f"fps      {fps:5.1f}\n\n"
            "WASD move   Space jump   Shift dive   Ctrl crouch\n"
            "Q/E camera  R recentre    F3 collision  F1 hud"
        )


def main():
    required = (
        os.path.join(CASTLE_GROUNDS, "collision.npz"),
        os.path.join(CASTLE_GROUNDS, "mesh.npz"),
    )
    missing = [path for path in required if not os.path.exists(path)]
    if missing:
        print("Missing converted assets:")
        for path in missing:
            print(f"  {path}")
        print("Run the converters listed in README.md first.")
        return 1

    app = Ursina(title="SM64 movement in Ursina", borderless=False)
    window.color = color.rgb(82, 153, 219)
    Game()
    app.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
