"""SM64 movement port rendered directly with ModernGL and pygame."""

import json
import math
import os
import sys

import moderngl
import numpy as np
import pygame
from PIL import Image


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, ROOT)

from glb_actor import GLBActor
from sm64py import surfaces
from sm64py.camera import FollowCamera
from sm64py.mario import Controller, MarioState, execute_action
from sm64py.mario import animations
from sm64py.mario import constants as C
from sm64py.math_util import s16, s16_to_degrees


ASSETS = os.path.join(ROOT, "assets")
LEVEL_DIR = os.path.join(ASSETS, "castle_grounds")
MARIO_MODEL = os.path.join(ASSETS, "mario", "mario.glb")
SPAWN = (-1328.0, 260.0, 4664.0)
SPAWN_YAW = 180.0
DEATH_PLANE = -4000.0
TICK_DT = 1.0 / 30.0
ACTION_NAMES = {v: k for k, v in vars(C).items() if k.startswith("ACT_")}


VERTEX_SHADER = """
#version 330
uniform mat4 mvp;
in vec3 in_position;
in vec2 in_uv;
in vec4 in_color;
out vec2 uv;
out vec4 vertex_color;
out vec3 normal;
void main() {
    gl_Position = mvp * vec4(in_position, 1.0);
    uv = in_uv;
    vertex_color = in_color;
    normal = vec3(0.0, 1.0, 0.0);
}
"""

ACTOR_VERTEX_SHADER = """
#version 330
uniform mat4 mvp;
uniform mat4 model;
uniform mat4 bones[30];
in vec3 in_position;
in vec3 in_normal;
in vec2 in_uv;
in vec4 in_color;
in vec4 in_joints;
in vec4 in_weights;
out vec2 uv;
out vec4 vertex_color;
out vec3 normal;
void main() {
    mat4 skin = bones[int(in_joints.x)] * in_weights.x
              + bones[int(in_joints.y)] * in_weights.y
              + bones[int(in_joints.z)] * in_weights.z
              + bones[int(in_joints.w)] * in_weights.w;
    vec4 local = skin * vec4(in_position, 1.0);
    gl_Position = mvp * local;
    normal = mat3(model) * mat3(skin) * in_normal;
    uv = in_uv;
    vertex_color = in_color;
}
"""

FRAGMENT_SHADER = """
#version 330
uniform sampler2D image;
uniform vec4 base_color;
uniform bool textured;
uniform bool lit;
in vec2 uv;
in vec4 vertex_color;
in vec3 normal;
out vec4 frag_color;
void main() {
    vec4 texel = textured ? texture(image, uv) : vec4(1.0);
    float shade = lit ? (0.55 + 0.45 * max(dot(normalize(normal), normalize(vec3(.4,.7,.5))), 0.0)) : 1.0;
    frag_color = texel * vertex_color * base_color * vec4(vec3(shade), 1.0);
    if (frag_color.a < 0.05) discard;
}
"""


def normalize(v):
    v = np.asarray(v, dtype="f4")
    length = np.linalg.norm(v)
    return v / length if length > 1e-8 else v


def perspective(fov, aspect, near, far):
    f = 1.0 / math.tan(math.radians(fov) / 2.0)
    return np.array([[f/aspect, 0, 0, 0], [0, f, 0, 0],
                     [0, 0, (far+near)/(near-far), 2*far*near/(near-far)],
                     [0, 0, -1, 0]], dtype="f4")


def look_at(eye, target):
    forward = normalize(np.asarray(target) - eye)
    side = normalize(np.cross(forward, [0, 1, 0]))
    up = np.cross(side, forward)
    result = np.eye(4, dtype="f4")
    result[0, :3], result[1, :3], result[2, :3] = side, up, -forward
    result[:3, 3] = -result[:3, :3] @ np.asarray(eye)
    return result


def model_matrix(position, yaw):
    angle = math.radians(yaw)
    c, s = math.cos(angle), math.sin(angle)
    result = np.array([[c, 0, s, position[0]], [0, 1, 0, position[1]],
                       [-s, 0, c, position[2]], [0, 0, 0, 1]], dtype="f4")
    return result


def uniform_matrix(uniform, matrix):
    uniform.write(np.asarray(matrix, dtype="f4").T.tobytes())


def make_texture(ctx, image):
    image = image.convert("RGBA").transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    texture = ctx.texture(image.size, 4, image.tobytes())
    texture.build_mipmaps()
    texture.filter = (moderngl.LINEAR_MIPMAP_LINEAR, moderngl.LINEAR)
    return texture


class LevelRenderer:
    def __init__(self, ctx, program):
        data = np.load(os.path.join(LEVEL_DIR, "mesh.npz"))
        positions, colors = data["positions"].astype("f4"), data["colors"].astype("f4") / 255.0
        uvs = data["uvs"].astype("f4") if "uvs" in data else np.zeros((len(positions), 2), "f4")
        triangles = data["triangles"]
        with open(os.path.join(LEVEL_DIR, "mesh_materials.json"), encoding="utf-8") as fh:
            groups = json.load(fh)
        self.draws = []
        for group in groups:
            indices = triangles[group["first"]:group["first"]+group["count"]].reshape(-1)
            group_colors = colors[indices].copy()
            if group.get("lighting"):
                # For lit F3D groups these bytes encode signed normals, not
                # RGB.  Avoid interpreting normals as psychedelic colours;
                # the texture provides the diffuse colour in this renderer.
                group_colors[:, :3] = 1.0
            packed = np.hstack((positions[indices], uvs[indices], group_colors)).astype("f4")
            vbo = ctx.buffer(packed.tobytes())
            vao = ctx.vertex_array(program, [(vbo, "3f 2f 4f", "in_position", "in_uv", "in_color")])
            texture = None
            image_path = group.get("image")
            if image_path:
                image_path = image_path if os.path.isabs(image_path) else os.path.join(ROOT, image_path)
                if os.path.exists(image_path):
                    texture = make_texture(ctx, Image.open(image_path))
                    texture.repeat_x = group.get("wrap_s") != "clamp"
                    texture.repeat_y = group.get("wrap_t") != "clamp"
            self.draws.append((vao, texture, group.get("cull", True)))

    def render(self, ctx, program, view_projection):
        uniform_matrix(program["mvp"], view_projection)
        program["base_color"].value = (1, 1, 1, 1)
        program["lit"].value = False
        for vao, texture, cull in self.draws:
            ctx.enable(moderngl.CULL_FACE) if cull else ctx.disable(moderngl.CULL_FACE)
            program["textured"].value = texture is not None
            if texture:
                texture.use(0)
            vao.render(moderngl.TRIANGLES)


class CollisionRenderer:
    def __init__(self, ctx, program):
        data = np.load(os.path.join(LEVEL_DIR, "collision.npz"))
        verts = data["vertices"].astype("f4")
        tri = data["tri_verts"]
        lines = np.asarray([[verts[a], verts[b], verts[b], verts[c], verts[c], verts[a]] for a,b,c in tri], dtype="f4").reshape(-1, 3)
        colors = np.tile(np.array([0.1, 0.3, 1.0, 1.0], "f4"), (len(lines), 1))
        uvs = np.zeros((len(lines), 2), "f4")
        vbo = ctx.buffer(np.hstack((lines, uvs, colors)).astype("f4").tobytes())
        self.vao = ctx.vertex_array(program, [(vbo, "3f 2f 4f", "in_position", "in_uv", "in_color")])

    def render(self, ctx, program, vp):
        ctx.disable(moderngl.CULL_FACE)
        uniform_matrix(program["mvp"], vp)
        program["textured"].value = False
        program["lit"].value = False
        program["base_color"].value = (1, 1, 1, 1)
        self.vao.render(moderngl.LINES)


class MarioRenderer:
    def __init__(self, ctx, program):
        self.actor = GLBActor(MARIO_MODEL)
        self.draws = []
        texture_cache = {}
        for primitive in self.actor.primitives:
            attrs = primitive["attributes"]
            pos = self.actor.accessor(attrs["POSITION"]).astype("f4")
            normal = self.actor.accessor(attrs["NORMAL"]).astype("f4")
            uv = self.actor.accessor(attrs["TEXCOORD_0"]).astype("f4")
            col = self.actor.accessor(attrs["COLOR_0"]).astype("f4")
            joints = self.actor.accessor(attrs["JOINTS_0"]).astype("f4")
            weights = self.actor.accessor(attrs["WEIGHTS_0"]).astype("f4")
            packed = np.hstack((pos, normal, uv, col, joints, weights)).astype("f4")
            vbo = ctx.buffer(packed.tobytes())
            indices = self.actor.accessor(primitive["indices"]).astype("u4").ravel()
            ibo = ctx.buffer(indices.tobytes())
            vao = ctx.vertex_array(program, [(vbo, "3f 3f 2f 4f 4f 4f", "in_position", "in_normal", "in_uv", "in_color", "in_joints", "in_weights")], ibo)
            factor, tex_index = self.actor.material(primitive["material"])
            texture = None
            if tex_index is not None:
                if tex_index not in texture_cache:
                    texture_cache[tex_index] = make_texture(ctx, self.actor.image(tex_index))
                texture = texture_cache[tex_index]
            self.draws.append((vao, factor, texture))
        self.clip = None
        self.clip_time = 0.0

    def render(self, ctx, program, vp, position, yaw, mario, dt):
        clip, loop, rate = animations.resolve(mario)
        if clip != self.clip:
            self.clip, self.clip_time = clip, 0.0
        self.clip_time += dt * rate
        bones = self.actor.bone_matrices(clip, self.clip_time, loop)
        model = model_matrix(position, yaw)
        uniform_matrix(program["model"], model)
        uniform_matrix(program["mvp"], vp @ model)
        program["bones"].write(bones.transpose(0, 2, 1).astype("f4").tobytes())
        program["lit"].value = True
        # SM64 actor display lists mix winding across mirrored body parts.
        # Panda's glTF loader accounts for that; render them two-sided here.
        ctx.disable(moderngl.CULL_FACE)
        for vao, factor, texture in self.draws:
            program["base_color"].value = tuple(float(v) for v in factor)
            program["textured"].value = texture is not None
            if texture:
                texture.use(0)
            vao.render(moderngl.TRIANGLES)


class Game:
    def __init__(self):
        pygame.init()
        pygame.display.gl_set_attribute(pygame.GL_CONTEXT_MAJOR_VERSION, 3)
        pygame.display.gl_set_attribute(pygame.GL_CONTEXT_MINOR_VERSION, 3)
        pygame.display.gl_set_attribute(pygame.GL_CONTEXT_PROFILE_MASK, pygame.GL_CONTEXT_PROFILE_CORE)
        self.size = (1280, 720)
        pygame.display.set_mode(self.size, pygame.OPENGL | pygame.DOUBLEBUF | pygame.RESIZABLE)
        pygame.display.set_caption("SM64 movement in ModernGL")
        self.ctx = moderngl.create_context()
        self.ctx.enable(moderngl.DEPTH_TEST | moderngl.BLEND)
        self.ctx.blend_func = moderngl.SRC_ALPHA, moderngl.ONE_MINUS_SRC_ALPHA
        self.level_program = self.ctx.program(vertex_shader=VERTEX_SHADER, fragment_shader=FRAGMENT_SHADER)
        self.actor_program = self.ctx.program(vertex_shader=ACTOR_VERTEX_SHADER, fragment_shader=FRAGMENT_SHADER)
        for program in (self.level_program, self.actor_program):
            program["image"].value = 0
        self.level = LevelRenderer(self.ctx, self.level_program)
        self.collision = CollisionRenderer(self.ctx, self.level_program)
        self.mario_renderer = MarioRenderer(self.ctx, self.actor_program)
        self.surface_data = surfaces.load(os.path.join(LEVEL_DIR, "collision.npz"))
        self.controller = Controller()
        self.mario = MarioState(self.surface_data, self.controller)
        self.mario.spawn(*SPAWN, SPAWN_YAW)
        self.camera = FollowCamera(self.surface_data, self.mario)
        self.accumulator = 0.0
        self.prev_pos = list(self.mario.gfx_pos)
        self.prev_yaw = self.mario.gfx_angle[1]
        self.show_collision = False
        self.running = True

    def input(self, events, dt):
        for event in events:
            if event.type == pygame.QUIT:
                self.running = False
            elif event.type == pygame.VIDEORESIZE:
                self.size = event.size
                self.ctx.viewport = (0, 0, *self.size)
            elif event.type == pygame.KEYDOWN:
                if event.key == pygame.K_ESCAPE: self.running = False
                if event.key == pygame.K_F3: self.show_collision = not self.show_collision
            elif event.type == pygame.MOUSEMOTION and any(event.buttons):
                self.camera.rotate(event.rel[0] * 0.25)
                self.camera.tilt(-event.rel[1] * 0.003)
        keys = pygame.key.get_pressed()
        self.controller.set_stick(float(keys[pygame.K_a] or keys[pygame.K_LEFT]) - float(keys[pygame.K_d] or keys[pygame.K_RIGHT]),
                                  float(keys[pygame.K_s] or keys[pygame.K_DOWN]) - float(keys[pygame.K_w] or keys[pygame.K_UP]))
        buttons = (C.A_BUTTON if keys[pygame.K_SPACE] else 0) | (C.B_BUTTON if keys[pygame.K_LSHIFT] else 0) | (C.Z_TRIG if keys[pygame.K_LCTRL] else 0)
        self.controller.set_buttons(buttons)
        if keys[pygame.K_q]: self.camera.rotate(-150 * dt)
        if keys[pygame.K_e]: self.camera.rotate(150 * dt)
        return keys

    def update(self, dt, keys):
        self.accumulator += dt
        steps = 0
        while self.accumulator >= TICK_DT and steps < 8:
            self.prev_pos, self.prev_yaw = list(self.mario.gfx_pos), self.mario.gfx_angle[1]
            self.mario.camera_yaw = self.camera.mario_yaw
            execute_action(self.mario)
            self.accumulator -= TICK_DT
            steps += 1
            if self.mario.floor is None or self.mario.pos[1] < DEATH_PLANE:
                self.mario.spawn(*SPAWN, SPAWN_YAW)
                self.prev_pos, self.prev_yaw, self.accumulator = list(self.mario.gfx_pos), self.mario.gfx_angle[1], 0.0
        alpha = max(0.0, min(1.0, self.accumulator / TICK_DT))
        pos = [self.prev_pos[i] + (self.mario.gfx_pos[i]-self.prev_pos[i])*alpha for i in range(3)]
        yaw = self.prev_yaw + s16(self.mario.gfx_angle[1]-self.prev_yaw)*alpha
        self.camera.update(dt, target_pos=pos, recenter=bool(keys[pygame.K_r]))
        return pos, s16_to_degrees(yaw)

    def render(self, pos, yaw, dt):
        self.ctx.clear(0.32, 0.60, 0.86, 1.0, depth=1.0)
        aspect = max(self.size[0], 1) / max(self.size[1], 1)
        target = [self.camera.focus[0], self.camera.focus[1]+60, self.camera.focus[2]]
        vp = perspective(45, aspect, 10, 30000) @ look_at(self.camera.pos, target)
        if self.show_collision:
            self.collision.render(self.ctx, self.level_program, vp)
        else:
            self.level.render(self.ctx, self.level_program, vp)
        self.mario_renderer.render(self.ctx, self.actor_program, vp, pos, yaw, self.mario, dt)
        action = ACTION_NAMES.get(self.mario.action, hex(self.mario.action))
        pygame.display.set_caption(f"SM64 ModernGL | {action} | {self.mario.forward_vel:.1f} speed")
        pygame.display.flip()

    def run(self):
        clock = pygame.time.Clock()
        while self.running:
            dt = min(clock.tick(240) / 1000.0, 0.25)
            keys = self.input(pygame.event.get(), dt)
            pos, yaw = self.update(dt, keys)
            self.render(pos, yaw, dt)
        pygame.quit()


def main():
    required = [os.path.join(LEVEL_DIR, "mesh.npz"), os.path.join(LEVEL_DIR, "collision.npz"), MARIO_MODEL]
    missing = [path for path in required if not os.path.exists(path)]
    if missing:
        print("Missing assets:\n  " + "\n  ".join(missing))
        return 1
    Game().run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
