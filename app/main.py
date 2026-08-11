"""Run the Hero on the castle grounds in Panda3D, with Mario's level and physics.

The Hero is the character being played; Mario is an NPC wandering the field,
and can be switched to at any time to compare the two. They share the level,
the collision and the 30 Hz tick, and nothing above that: the Hero runs his own
action machine (sm64py/hero/) built around the twenty clips he actually has,
while Mario still runs the decomp port (sm64py/mario/) and its 209.

The game logic runs at a fixed 30 Hz, as the original does -- every physics
constant in the action code is per-frame at that rate, so rendering is
decoupled and the simulation is stepped in whole ticks.

The camera is a third-person shooter's -- a spring arm off his shoulder, aimed
with the mouse, which the window takes hold of at startup. Escape gives the
pointer back and clicking the window takes it again. Nothing but the player
ever points it: it does not drift back behind him, and R is the only thing that
moves it on its own. See sm64py/camera.py for what it does and why.

Controls, as the Hero:
    W / A / S / D or arrows   analog stick (camera-relative)
    mouse                     look
    Right mouse or F (held)   aim: the camera comes in over his shoulder, the
                              view narrows, the crosshair closes up, and his
                              upper body turns to point where it does -- his
                              legs keep running wherever they were running.
                              See docs/aim.md and sm64py/aim.py
    Space                     jump, and only a jump -- unless he is skating,
                              where it takes off on the jets instead
    V (held)                  the jetpack. On the ground it puts him on his
                              skates; in the air it flies him
    Left Shift                attack -- again mid-swing to chain the second,
                              or while running for the spin kick
    Left Ctrl                 draw or sheathe the sword

Controls, as Mario -- the original's, unchanged:
    Space                     A -- jump
    Left Shift                B -- punch / dive
    Left Ctrl                 Z -- crouch / ground pound / long jump
    Z (held)                  shamble like a zombie
    C                         put the skates on, and take them off again

Both:
    X or left mouse (held)    a circle grows on the ground where the crosshair
                              points, with the arc of a throw to it; let go and
                              every ally inside it follows you
    X or left mouse (tapped)  send the squad to the spot being aimed at
    Q / E                     swing the camera without the mouse
    R                         re-centre the camera behind him
    `  (backquote / tilde)    open the debug console, pausing the game
    F1                        toggle the debug readout
    F2                        swap between the Hero and Mario
    F3                        toggle the collision overlay
    Escape                    close the console, then release the mouse, then
                              quit

On a gamepad, if one is plugged in -- the same set, added to the keyboard
rather than replacing it (see app/gamepad.py):
    left stick / d-pad        analog stick
    right stick               look
    right stick (clicked)     aim -- a latch, since the thumb aiming with it
                              cannot also hold it in
    left stick (held in)      the right stick sets how far out the camera
                              sits, rather than where it looks
    left trigger              the jetpack: on the ground he skates on it, in
                              the air he flies on it
    A                         jump -- or, off the skates, take off
    B                         attack -- Mario's B
    X                         the squad, held or tapped as above
    right trigger             Z
    right shoulder            re-centre the camera
    left shoulder (held)      shamble like a zombie
    Y                         the skates
    Start                     swap between the Hero and Mario

The console shows the readout, everything the game has printed -- scroll back
through it with the wheel -- and a command line; typing the name of one of the
tunables registered in `_register_tunables` puts a slider for it on screen,
which stays up once the console is dismissed so it can be dragged while
playing. The game is paused for as long as the console is open. `help` inside
it lists the rest.
"""

from collections import deque
import json
import math
import os
import sys
import time

from direct.actor.Actor import Actor
from direct.showbase.ShowBase import ShowBase
from direct.gui.OnscreenText import OnscreenText
from panda3d.core import (
    AmbientLight,
    ClockObject,
    DirectionalLight,
    Filename,
    Fog,
    LineSegs,
    TextNode,
    TransparencyAttrib,
    Vec4,
    WindowProperties,
    loadPrcFileData,
)

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

# Through the package rather than as a sibling module: `tools/check_hero.py`
# imports this file as `app.main`, and in that reading the directory this file
# sits in is not on the path -- only the root inserted above is.
from app.gamepad import Gamepad  # noqa: E402
from sm64py import audio, console, objects, squad, surfaces  # noqa: E402
from sm64py.impostor import ImpostorSet  # noqa: E402
from sm64py.aim import AimController, melee_tracking  # noqa: E402
from sm64py.camera import FollowCamera, smooth_damp  # noqa: E402
from sm64py.hero import HeroState  # noqa: E402
from sm64py.hero import actions as hero_actions  # noqa: E402
from sm64py.hero import animations as hero_animations  # noqa: E402
from sm64py.hero import constants as HC  # noqa: E402
from sm64py.level import (  # noqa: E402
    ObjectRenderer,
    animate_water,
    build_water_surface,
    load_collision_geometry,
    load_level_geometry,
    preload,
    use_linear_textures,
    use_mipmaps,
)
from sm64py.mario import Controller, MarioState, execute_action  # noqa: E402
from sm64py.mario import animations  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402
from sm64py.math_util import s16, s16_to_degrees, to_panda  # noqa: E402

ASSETS = os.path.abspath(
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
)
CASTLE_GROUNDS = os.path.join(ASSETS, "castle_grounds")
MARIO_MODEL = os.path.join(ASSETS, "mario", "mario.glb")
MARIO_CLIPS = os.path.join(ASSETS, "mario", "mario_clips.json")
HERO_MODEL = os.path.join(ASSETS, "hero", "hero.glb")
HERO_CLIPS = os.path.join(ASSETS, "hero", "hero_clips.json")
SOUNDS = os.path.join(ASSETS, "sounds", "mario64")
ACTORS = os.path.join(ASSETS, "actors")
IMPOSTORS = os.path.join(ASSETS, "impostors")
LEVEL_OBJECTS = os.path.join(CASTLE_GROUNDS, "collision_objects.json")

# Enemy models drawn as instanced sprites rather than as skinned actors, with
# the draw scale their objects carry -- baked by tools/bake_impostor.py, drawn
# by sm64py/impostor.py. A model with no bake on disk is simply drawn the old
# way, so this is a list of which crowds have been prepared, not a requirement.
IMPOSTOR_MODELS = {"goomba": objects.Goomba.draw_scale,
                   "scuttlebug": objects.Scuttlebug.draw_scale}

# What counts as an enemy: the things that can be fought, as against the trees
# and the Marios, which are scenery and company respectively. Named once
# because two places ask the question -- the readout counting what is left, and
# the tuning that only wants the pipes producing these.
ENEMY_TYPES = (objects.Goomba, objects.Scuttlebug)

# Enemies the level does not place itself. Castle grounds has no goombas or
# scuttlebugs in the original, so these are placed by hand -- out across the
# field rather than on top of the spawn, so they are something to walk toward
# instead of something already standing on you.
ENEMY_SPAWNS = [
    (objects.Goomba, -300.0, 300.0, 2600.0),
    (objects.Goomba, -2400.0, 300.0, 2900.0),
    (objects.Goomba, 900.0, 300.0, 3400.0),
    (objects.Scuttlebug, -2900.0, 300.0, 2100.0),
    (objects.Scuttlebug, 400.0, 300.0, 1900.0),
]

# Warp pipes, and what comes out of each: where it stands, what it produces,
# and how many of them it keeps going. One by the spawn on the castle path,
# and one in each far corner of the map, so the two enemy pipes are somewhere
# to go rather than something to trip over on the way out of the gate.
#
# The pipes are drawn but not collided with -- the level's own collision is
# what the physics reads, and nothing here adds to it -- so a pipe is scenery
# that you can walk through and that things come out of.
PIPE_SPAWNS = [
    (objects.Mario, -915.3, 260.0, 4629.5),
    (objects.Goomba, -5509.3, 543.0, -3924.8),
    (objects.Scuttlebug, 4681.0, 545.0, -6808.4),
]

SKY_COLOUR = (0.32, 0.60, 0.86)
UNDERWATER_COLOUR = (0.06, 0.28, 0.36)


def _across(start, end, half_width):
    """A horizontal offset at right angles to the line between two points."""
    dx = end[0] - start[0]
    dz = end[2] - start[2]
    length = math.hypot(dx, dz)
    if length < 1e-3:
        return (half_width, 0.0)
    return (-dz / length * half_width, dx / length * half_width)


def panda_path(path):
    """Convert an OS path into the form Panda3D's loaders expect.

    Panda3D uses its own path syntax rather than the platform's, so a native
    string has to be translated before any loader sees it. It matters most on
    Windows, where a drive path becomes /c/... and a UNC share becomes
    /hosts/server/share/...; handing a loader the raw string instead makes it
    treat the path as relative and search the model path in vain.
    """
    return Filename.from_os_specific(os.path.abspath(path))

# Yaw correction between the model's facing and Mario's game-side yaw.
# It works out to zero: the model faces -Y in Panda3D, and game yaw 0 means
# facing +Z, which to_panda also maps to -Y. Kept named so the coincidence is
# on purpose rather than something to rediscover.
MODEL_YAW_OFFSET = 0.0

# The Hero comes out of Blender at his own scale -- 1.9 units standing -- since
# nothing in the export pipeline knows about game units. Scaled here rather
# than baked into the .glb so it is one number to change instead of a
# re-export. 81 puts him at ~154 units, the height Mario is exported at, which
# is what keeps a shared collision radius and a shared jump height honest.
HERO_SCALE = 81.0

# Mario's export is already in game units.  Keep him deliberately smaller
# than the Hero, and keep this in step with MarioState.motion_scale so what
# the player sees and how far he moves tell the same story.
MARIO_SCALE = 2.0 / 3.0

# Which way the Hero's model faces once loaded -- the same way Mario does, so
# the same (absent) correction. Measured rather than assumed: in the exported
# .glb his toe joints sit forward of his ankles along +Z, and +Z is the way
# game yaw 0 points. An earlier guess of 180 here had him running backwards.
HERO_YAW_OFFSET = 0.0

# Where Mario stands about now that he is not the one being played. Out in the
# field in front of the castle, clear of the spawn so the Hero does not start
# inside him.
MARIO_NPC_SPAWN = (-1750.0, 300.0, 3600.0)

# The game ticks at 30 Hz; all movement constants assume it.
TICK_RATE = 30.0
TICK_DT = 1.0 / TICK_RATE

# One out of each pipe every thirty seconds, up to five of them alive at once.
# The pipe counts in ticks, as everything in the simulation does; these are in
# seconds, which is the unit the number is worth thinking about in, and the
# unit the console slider drags in.
PIPE_INTERVAL_SECONDS = 30.0
PIPE_INTERVAL = int(PIPE_INTERVAL_SECONDS * TICK_RATE)
PIPE_LIMIT = 5

# What the enemy_rate and enemy_limit sliders are allowed to reach. Now that
# the enemies are drawn as instanced sprites rather than skinned actors, the
# cap is a crowd rather than a handful -- the old ceiling of thirty was set by
# what the renderer could carry, not by anything the simulation minds. A limit
# of zero at the bottom turns the pipes off without a restart. To fill the
# field at once rather than one every few seconds, see MARIO_ENEMY_STRESS.
ENEMY_RATE_RANGE = (0.5, 120.0)
ENEMY_LIMIT_RANGE = (0, 2000)

# A crowd spawned at startup, split between the enemy types and scattered over
# the field, for looking at the sprite renderer under load without waiting for
# the pipes to fill. Off by default; set the environment variable to a count.
ENEMY_STRESS = int(os.environ.get("MARIO_ENEMY_STRESS", "0"))
ENEMY_STRESS_SPREAD = 6000.0

# Mario spawns where the level script places him, facing the castle.
SPAWN = (-1328.0, 260.0, 4664.0)
SPAWN_YAW = 180.0

# Below the lowest collision in the level; reaching it means Mario is lost.
DEATH_PLANE = -4000.0

ACTION_NAMES = {v: k for k, v in vars(C).items() if k.startswith("ACT_")}

# How often the debug readout is rebuilt, in seconds. Assigning to an
# OnscreenText only marks it dirty; the glyph geometry is regenerated inside
# the following cull traversal, which measured ~0.4 ms per frame and spiked
# far higher. The numbers are unreadable at 60 Hz anyway.
HUD_INTERVAL = 0.1

# Keep enough real time to make an isolated hitch visible in the debug readout
# without letting an old loading spike colour the rest of a play session.
FRAME_HISTORY_SECONDS = 5.0

# The crosshair, in aspect2d units -- a fortieth of the window's height across
# the gap and a little over that again in each arm. Small on purpose: it marks
# the centre of the view, and anything larger starts covering what is standing
# there.
CROSSHAIR_GAP = 0.012
CROSSHAIR_ARM = 0.028

# It does not hold still. A crosshair that opens while he runs and closes when
# he stops and aims is the cheapest honest thing a shooter's HUD can say, and
# it is read without being looked at: the size is how settled the aim is.
#
# The spread is a multiplier on the whole reticle. 1.0 is standing at the hip;
# a sprint pushes it out to SPREAD_RUNNING, being in the air adds SPREAD_AIR on
# top, and the sights pull it down to SPREAD_AIM -- where the dot in the middle
# also fades up, because at that size the four ticks are further from the point
# being aimed at than the point is wide.
CROSSHAIR_SPREAD_SMOOTH = 0.12
CROSSHAIR_SPREAD_RUNNING = 1.9
CROSSHAIR_SPREAD_AIR = 0.6
CROSSHAIR_SPREAD_AIM = 0.55
CROSSHAIR_DOT = 0.0045
# What it fades to at the hip. Full brightness belongs to the aim; the rest of
# the time this is a mark saying where the middle is, not an instrument.
CROSSHAIR_HIP_ALPHA = 0.55

# Keeping the pointer off the edges of the window, where it would stop
# reporting movement, on the platforms without a relative mouse mode. It is
# shoved back to the middle once it leaves this share of the way out from the
# centre; a warp is then in flight for up to this many frames, and readings are
# dropped until one lands within this many pixels of where it was sent. See
# `Game._read_mouse` for why any of that is necessary.
MOUSE_MARGIN = 0.5
MOUSE_WARP_FRAMES = 4
MOUSE_SETTLED = 8.0

# Degrees of view per unit of a loose-pointer drag, whose coordinates run -1 to
# 1 across the window rather than in pixels.
DRAG_YAW = 180.0
DRAG_PITCH = 55.0

# The bone tools/aim_rig.py inserts for the procedural aim layer to write to.
AIM_JOINT = "AIM_TORSO"

# Above this speed he counts as moving under his own steam, which buys the
# torso the full twist before his feet are asked to help. See
# `Game._update_torso_aim` and sm64py/aim.py.
AIM_TURN_MAX_SPEED = 2.0

# A landing kicks the camera. Below the first speed nothing is felt and nothing
# is drawn; at the second the kick is at full strength, which is a fall of
# about a thousand units.
LAND_SHAKE_MIN_SPEED = 45.0
LAND_SHAKE_FULL_SPEED = 110.0
LAND_SHAKE_AMOUNT = 0.55

# The squad reticle, drawn in the world rather than on the screen: the arc from
# his hands to the spot being aimed at, a ring on that spot, and the whistle
# circle around it while the button is held.
#
# The rings are traced along the ground they sit on rather than drawn flat, so
# a circle over the slope up to the castle follows the slope; the lift keeps
# them off the surface they are tracing, which they would otherwise z-fight
# with. The point counts are what they are because every one of them on the
# whistle circle costs a collision query, and those only run while aiming.
AIM_CHEST = 110.0
ARC_RUNG_EVERY = 4
ARC_RUNG_HALF_WIDTH = 26.0
SELECT_RING_POINTS = 28
TARGET_RING_POINTS = 18
TARGET_RING_RADIUS = 130.0
RETICLE_LIFT = 6.0
# Heavy enough to hold up against the level under it. These are world-space
# lines seen at a distance and drawn over grass, sand and a white castle wall,
# so a hairline reads as an artefact rather than as something the game is
# telling you.
RETICLE_THICKNESS = 5.0

# How long the reticle lingers, fading, once the button comes up. Without it
# the whole thing vanishes on the frame the order is given and there is nothing
# to see where it went.
RETICLE_FLASH = 0.4

loadPrcFileData("", "window-title SM64 movement in Panda3D")
loadPrcFileData("", "framebuffer-multisample 1")
loadPrcFileData("", "multisamples 4")
# Off by default: vsync removes tearing, but it ties each frame to the refresh,
# so a frame that overruns its interval waits for the next one and reads as a
# stutter. Turning it off was what cleared up the microstutters here, and it is
# how the ModernGL front end has always run. Set MARIO_VSYNC=1 to put it back.
loadPrcFileData("", f"sync-video {os.environ.get('MARIO_VSYNC', '0')}")


class Player:
    """One playable character: what simulates it, and what draws it.

    Two characters now run in the same level and they have nothing in common
    below the neck -- different skeletons, different clip names, different
    action machines, different ideas about what pressing B means. What they do
    share is the shape of the frame: step the state, resolve a clip, draw a
    node. That shape is all this holds, so the update loop can stay written
    once instead of forking on which character is active.
    """

    def __init__(self, name, state, execute, anims, action_names,
                 node, actor, yaw_offset, aim=None):
        self.name = name
        self.state = state
        self.execute = execute              # one tick of the action machine
        self.anims = anims                  # module with resolve/start_frame
        self.action_names = action_names
        self.node = node
        self.actor = actor
        self.yaw_offset = yaw_offset
        # The procedural aim layer, or None for a skeleton without the pivot
        # tools/aim_rig.py inserts -- Mario's, as things stand.
        self.aim = aim
        # Which clip is playing, so a change can be told from a repeat.
        self.current_anim = None

    @property
    def action_name(self):
        return self.action_names.get(self.state.action, hex(self.state.action))

    def show(self):
        self.node.show()

    def hide(self):
        self.node.hide()


class PipeTuning:
    """The enemy pipes' rate and cap, as two numbers rather than two per pipe.

    A tunable reads and writes one attribute of one object, and the numbers it
    wants live on every enemy pipe -- so this stands in for the group: reading
    the first, writing all of them. The Mario pipe is deliberately not in here;
    it produces company rather than enemies, and a slider labelled "enemy"
    dragging it too would be a surprise.

    A rate cut also pulls in a countdown already running, since a pipe part way
    through a thirty-second wait would otherwise ignore the new number until
    the old one had finished elapsing -- which reads as a slider that does
    nothing for half a minute.
    """

    def __init__(self, pipes):
        self.pipes = list(pipes)

    @property
    def seconds(self):
        if not self.pipes:
            return PIPE_INTERVAL_SECONDS
        return self.pipes[0].interval / TICK_RATE

    @seconds.setter
    def seconds(self, value):
        # At least one tick: an interval of zero would fire the pipe on every
        # frame the cap left room, which is not a rate but a fountain.
        ticks = max(1, int(round(value * TICK_RATE)))
        for pipe in self.pipes:
            pipe.interval = ticks
            pipe.countdown = min(pipe.countdown, ticks)

    @property
    def limit(self):
        return self.pipes[0].limit if self.pipes else PIPE_LIMIT

    @limit.setter
    def limit(self, value):
        for pipe in self.pipes:
            pipe.limit = int(value)


class Game(ShowBase):
    def __init__(self):
        ShowBase.__init__(self)

        self.disable_mouse()
        self.set_background_color(*SKY_COLOUR)

        self.surfaces = surfaces.load(os.path.join(CASTLE_GROUNDS, "collision.npz"))

        self.level = load_level_geometry(os.path.join(CASTLE_GROUNDS, "mesh.npz"))
        self.level.reparent_to(self.render)
        # The loader keeps one node per material group so each can carry its
        # own state; groups sharing a state can still be merged, which halves
        # the node count here. The level never moves, so this is free.
        self.level.flatten_strong()

        self.water = build_water_surface(self.surfaces)
        self.water.reparent_to(self.render)

        self.objects = objects.ObjectSet(self.surfaces)
        self._spawn_objects()
        self.interactions = objects.Interactions(self.objects)
        # The enemy crowds go to the instanced sprite renderer; whatever it has
        # a bake for is left off the actor renderer below so it is not drawn
        # twice. A missing bake leaves its model out of the set, and the actor
        # renderer picks it up as before.
        self.impostors = ImpostorSet(IMPOSTORS, self.render, IMPOSTOR_MODELS)
        self.impostor_drawn = 0
        self.object_renderer = ObjectRenderer(
            ACTORS, self.loader, self.render,
            # Mario is an NPC now, and his model is the one the game already
            # ships rather than a second copy under assets/actors/.
            model_paths={"mario": MARIO_MODEL},
            skip_models=set(self.impostors.fields),
        )
        drawn = self.object_renderer.build(self.objects)
        print(f"Objects: {len(self.objects.objects)} spawned, {drawn} drawn as "
              f"actors, {len(self.impostors.fields)} enemy types as impostors")

        self.collision_view = load_collision_geometry(
            os.path.join(CASTLE_GROUNDS, "collision.npz")
        )
        self.collision_view.reparent_to(self.render)
        self.collision_view.set_render_mode_wireframe()
        self.collision_view.hide()

        self.controller = Controller()
        self._build_players()
        self.follow_camera = FollowCamera(self.surfaces, self.state)

        # The camera owns the field of view from here -- it narrows for the
        # sights and widens a little with his speed -- and sets it every frame
        # it changes. This is only the value the first frame is drawn at.
        self.camLens.set_fov(self.follow_camera.fov)
        self.camLens.set_near_far(10, 30000)

        self._setup_lighting()
        self._setup_hud()
        self._setup_crosshair()
        self._setup_squad()
        # Before the input, which asks the console whether it has the keyboard.
        self._setup_console()
        self._setup_input()

        self._accumulator = 0.0
        self._show_debug = True
        self._mouse_anchor = None
        self._hud_timer = 0.0
        self._water_time = 0.0
        self._frame_history = deque()
        self._reset_interpolation()

        # Everything is in the scene graph by now, so get it onto the GPU
        # before the first frame rather than during play.
        preload(self.render, self.win.get_gsg())

        self._setup_audio()

        self.task_mgr.add(self._update, "update")

    def _spawn_objects(self):
        """Trees come from the level data; the enemies are placed by hand."""
        if os.path.exists(LEVEL_OBJECTS):
            with open(LEVEL_OBJECTS, "r", encoding="utf-8") as fh:
                self.objects.load_special_objects(json.load(fh))
        for cls, x, y, z in ENEMY_SPAWNS:
            self.objects.spawn(cls, x, y, z)
        self.mario_npc = self.objects.spawn(objects.Mario, *MARIO_NPC_SPAWN)
        self.pipes = [
            self.objects.spawn(objects.WarpPipe, x, y, z, spawns=cls,
                               interval=PIPE_INTERVAL, limit=PIPE_LIMIT)
            for cls, x, y, z in PIPE_SPAWNS
        ]
        if ENEMY_STRESS:
            self._spawn_stress_crowd(ENEMY_STRESS)

        # The ones the enemy sliders speak for; see PipeTuning.
        self.pipe_tuning = PipeTuning(
            pipe for pipe in self.pipes if pipe.spawns in ENEMY_TYPES
        )

    def _spawn_stress_crowd(self, count):
        """Scatter a crowd of enemies over the field, for testing under load.

        Split evenly between the enemy types and dropped onto the floor
        wherever they land; ones that come down over the moat or off the map
        find no floor and are quietly skipped rather than falling out of the
        world. The point is a lot of them on screen at once, not a fair fight.
        """
        import random
        rng = random.Random(1)
        types = list(ENEMY_TYPES)
        placed = 0
        for i in range(count):
            cls = types[i % len(types)]
            x = rng.uniform(-ENEMY_STRESS_SPREAD, ENEMY_STRESS_SPREAD)
            z = rng.uniform(-ENEMY_STRESS_SPREAD, ENEMY_STRESS_SPREAD)
            height, floor = self.surfaces.find_floor(x, 2000.0, z)
            if floor is None:
                continue
            self.objects.spawn(cls, x, height, z, rng.randint(0, 0xFFFF))
            placed += 1
        print(f"Stress crowd: {placed} enemies placed")

    # -- players -------------------------------------------------------------

    def _build_players(self):
        """Both characters, sharing one controller and one spawn point.

        Both are simulated from the same Controller instance, but only the
        active one is stepped, so the other simply holds its last state until
        it is switched back to.
        """
        hero = HeroState(self.surfaces, self.controller)
        hero.spawn(*SPAWN, SPAWN_YAW)
        hero_animations.load_clip_metadata(HERO_CLIPS)
        hero_node, hero_actor = self._build_actor(
            HERO_MODEL, HERO_YAW_OFFSET, "hero", scale=HERO_SCALE,
            build_hint="python3 tools/export_hero_gltf.py (inside Blender)")

        mario = MarioState(self.surfaces, self.controller)
        mario.spawn(*SPAWN, SPAWN_YAW)
        animations.load_clip_metadata(MARIO_CLIPS)
        mario_node, mario_actor = self._build_actor(
            MARIO_MODEL, MODEL_YAW_OFFSET, "mario", scale=MARIO_SCALE,
            build_hint="python3 tools/export_actor_gltf.py --actor mario "
                       "--anims all -o assets/mario/mario.glb")

        self.players = [
            Player("hero", hero, hero_actions.execute_action, hero_animations,
                   HC.ACTION_NAMES, hero_node, hero_actor, HERO_YAW_OFFSET,
                   aim=AimController(self._aim_pivot(hero_actor))),
            # Mario gets a controller with nothing to write to. His skeleton
            # has no pivot and never will -- it is the decomp's, and its clips
            # are the decomp's too -- but the half of the layer that turns a
            # character on his feet needs no bone, and without it he would be
            # the one character who cannot look where he is aiming.
            Player("mario", mario, execute_action, animations,
                   ACTION_NAMES, mario_node, mario_actor, MODEL_YAW_OFFSET,
                   aim=AimController(None)),
        ]
        # The Hero is who the game is about now; Mario is switched to.
        self.player = self.players[0]
        self.players[1].hide()

    @staticmethod
    def _aim_pivot(actor):
        """The AIM_TORSO joint as something that can be written to, or None.

        `controlJoint` takes the joint out of the animation's hands, which is
        exactly right here and would be wrong on any other bone in the file:
        AIM_TORSO carries no keyframes, so there is nothing to take. Anything
        else and the clip would stop playing on it.

        None when the model is missing altogether, or when it predates
        tools/aim_rig.py -- the game runs either way, with the torso simply
        pointing wherever the clip points it.
        """
        if actor is None:
            return None
        if actor.get_joints(jointName=AIM_JOINT):
            return actor.control_joint(None, "modelRoot", AIM_JOINT)
        print(f"no {AIM_JOINT} joint in the model; the upper body will not aim."
              "\nBuild it with:\n  python3 tools/aim_rig.py assets/hero/hero.glb")
        return None

    @property
    def state(self):
        """The active character's simulation state."""
        return self.player.state

    def _switch_player(self):
        """Hand control to the other character, where the current one stands.

        Moving him rather than letting him keep his own position is the whole
        point: switching is for comparing how the two move over the same
        ground, and that is no use if they are in different parts of the level.
        """
        self.player.hide()
        previous = self.player.state

        self.player = self.players[1 - self.players.index(self.player)]
        self.player.show()

        state = self.player.state
        state.spawn(previous.pos[0], previous.pos[1], previous.pos[2],
                    s16_to_degrees(previous.face_angle[1]))
        self.player.current_anim = None
        # He has just been put down somewhere facing a new way, so whatever the
        # torso was twisted toward belongs to the last place he stood.
        if self.player.aim is not None:
            self.player.aim.reset()
        # The camera reads his facing and his speed -- to recentre, and to
        # drift back behind him while he runs -- so it has to be following the
        # character that is actually being played rather than the one it was
        # constructed with.
        self.follow_camera.target = state

        self._stand_down_mario_npcs()
        # The squad is made of Marios, and half of it has just been switched
        # off -- or, going the other way, has just stopped being an NPC at all.
        # Either way it is not a squad any more.
        self.squad.disband()
        self._cancel_aim()

        self._reset_interpolation()
        # Swapping is allowed from inside the console, and the character that
        # just came on stage has not been stopped yet.
        if self.console.visible:
            self._freeze_animation()
        print(f"Playing as {self.player.name}")

    def _stand_down_mario_npcs(self):
        """Only one Mario in the field at a time.

        Mario is only an NPC while somebody else is being played; two of him
        standing in the same field reads as a bug rather than a cameo. Applied
        to every copy of him rather than to the one the level placed, since
        his pipe produces four more -- and re-applied each tick, because it
        can produce one while Mario himself is the one being played.

        They are switched off, not killed, so the pipe still counts them and
        does not spend the swap making replacements.
        """
        wanted = self.player.name != "mario"
        for npc in self.objects.of_class(objects.Mario):
            npc.active = wanted

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

        self.air_fog = Fog("distance")
        self.air_fog.set_color(*SKY_COLOUR)
        self.air_fog.set_linear_range(9000, 20000)

        # Underwater the view closes in hard and goes green-blue. This is what
        # sells being submerged far more than the surface quad does -- without
        # it the camera below the waterline looks identical to the camera above
        # it, since the water is a single flat sheet with nothing behind it.
        self.water_fog = Fog("underwater")
        self.water_fog.set_color(*UNDERWATER_COLOUR)
        self.water_fog.set_linear_range(200, 4200)

        self._camera_submerged = None
        self._apply_camera_medium(False)

    def _apply_camera_medium(self, submerged):
        """Swap fog and sky for whichever side of the surface the camera is on."""
        if submerged == self._camera_submerged:
            return
        self._camera_submerged = submerged
        if submerged:
            self.render.set_fog(self.water_fog)
            self.set_background_color(*UNDERWATER_COLOUR)
        else:
            self.render.set_fog(self.air_fog)
            self.set_background_color(*SKY_COLOUR)

    def _update_camera_medium(self):
        """Is the camera itself under the water surface?

        Tested against the camera rather than Mario: swimming just below the
        surface leaves the camera in open air looking down through it, and
        tinting the whole world in that case looks wrong.
        """
        x, y, z = self.follow_camera.pos
        level = self.surfaces.find_water_level(x, z)
        self._apply_camera_medium(level is not None and y < level)

    def _build_actor(self, model, yaw_offset, name, scale=1.0, build_hint=""):
        """Load a converted actor, or fall back to a marker if it is missing.

        Mario's .glb comes out of tools/export_actor_gltf.py already at game
        units and so wants no scaling; the Hero's comes out of Blender at
        Blender's scale and does. Hence the argument rather than an assumption.
        """
        if not os.path.exists(model):
            print(f"{name} model not found at {model}; using a marker.")
            if build_hint:
                print(f"Build it with:\n  {build_hint}")
            marker = self.loader.load_model("models/box")
            marker.set_scale(50, 50, 150)
            marker.set_pos(-25, -25, 0)
            marker.set_texture_off(1)
            marker.set_color(0.85, 0.20, 0.20, 1)
            root = self.render.attach_new_node(name)
            marker.reparent_to(root)
            return root, None

        actor = Actor(panda_path(model))
        use_linear_textures(actor)
        use_mipmaps(actor)
        actor.reparent_to(self.render)
        # The model faces +Y in Panda3D once loaded, while yaw 0 in the game
        # means facing +Z, which maps to -Y. Hence the half turn.
        actor.set_h(yaw_offset)
        if scale != 1.0:
            actor.set_scale(scale)

        holder = self.render.attach_new_node(name)
        actor.reparent_to(holder)
        actor.set_pos(0, 0, 0)
        return holder, actor

    def _update_animation(self):
        """Play whatever clip the active character's action calls for."""
        player = self.player
        if player.actor is None:
            return
        state = player.state

        resolved = player.anims.resolve(state)
        if resolved is None:
            # The action asked to keep whatever is already playing.
            state.anim_reset = False
            return
        name, loop, rate = resolved

        if player.actor.get_anim_control(name) is None:
            # Not every action has a clip exported; keep the previous pose.
            state.anim_reset = False
            return

        # An action can ask for its clip to restart without the clip itself
        # changing, which is how a held swim stroke reads as one continuous
        # cycle instead of visibly retriggering.
        restart = state.anim_reset
        state.anim_reset = False

        if name != player.current_anim or restart:
            player.current_anim = name
            # Several clips carry lead-in frames the game never shows, so
            # playback starts where the animation header says it does.
            start = player.anims.start_frame(name)
            if loop:
                player.actor.loop(name, fromFrame=start)
            else:
                player.actor.play(name, fromFrame=start)

        # Reapplied every frame, not just on a change: the walk and run cycles
        # follow the character's current speed rather than a fixed cadence.
        player.actor.set_play_rate(rate, name)

    # -- input ---------------------------------------------------------------

    def _setup_input(self):
        self.keys = {}
        bindings = [
            ("w", "up"), ("s", "down"), ("a", "left"), ("d", "right"),
            ("arrow_up", "up"), ("arrow_down", "down"),
            ("arrow_left", "left"), ("arrow_right", "right"),
            ("space", "a"), ("lshift", "b"), ("lcontrol", "z"),
            # The jetpack's own control, which the pad has on a trigger and
            # the keyboard has nowhere obvious: A used to double as it and no
            # longer does, so without a key of its own the keyboard would have
            # no boosters at all.
            ("v", "thrust"),
            # The N64 Z trigger is on left control, so the Z key itself is
            # free -- and it is the obvious one to put the zombie on.
            ("z", "zombie"),
            ("q", "cam_left"), ("e", "cam_right"),
            ("r", "cam_center"),
        ]
        # The mouse is the aim now, so the keyboard keeps a key for the sights
        # too -- for anyone playing without one, and because binding the
        # sights to a mouse button alone makes them unavailable the moment the
        # pointer is let go of.
        bindings.append(("f", "aim"))
        for key, name in bindings:
            self.keys.setdefault(name, False)
            self.accept(key, self._set_key, [name, True])
            self.accept(f"{key}-up", self._set_key, [name, False])

        # The skates are the one control that latches. Holding a key to stay
        # on ice would mean never letting go, since the whole level is the
        # rink.
        self._skating = False
        self.accept("c", self._toggle_skating)

        # The squad button, where the press and the release are two different
        # commands. The release is accepted unconditionally, as the key-ups
        # above are: one that arrives while the console is open belongs to a
        # press the console already threw away, and `_squad_release` checks.
        self.accept("x", self._squad_press)
        self.accept("x-up", self._squad_release)

        self.accept("escape", self._escape)
        self.accept("f1", self._toggle_debug)
        self.accept("f2", self._switch_player)
        self.accept("f3", self._toggle_collision)

        # A pad, if there is one, driving the same controls the keys above do
        # rather than a set of its own. It is read by polling and not through
        # the event bindings here, so nothing above has to know about it.
        self.gamepad = Gamepad(self.devices)

        # The mouse has two lives. Captured -- which is how it starts, and how
        # it spends nearly all of its time -- it is the look control and its
        # buttons are gameplay: right aims, left commands the squad. Released,
        # it is a pointer for the console and its sliders, and a drag on it
        # still swings the view so the game is playable without the capture at
        # all. Escape and the console move between the two.
        self._mouse_captured = False
        self._was_captured = True
        # The pad's zoom latch. Not a key, so it survives the console taking
        # the keyboard, and it is dropped by hand where that matters.
        self._aim_toggle = False
        self._mouse_anchor = None
        self._pointer_origin = None
        self._warp_frames = 0
        self._dragging = False
        self.accept("mouse1", self._mouse_button, [1, True])
        self.accept("mouse1-up", self._mouse_button, [1, False])
        self.accept("mouse3", self._mouse_button, [3, True])
        self.accept("mouse3-up", self._mouse_button, [3, False])

        self._set_mouse_captured(True)

    def _set_key(self, name, value):
        # Releases always land, so a key held as the console opened does not
        # stay stuck down; presses do not, so typing is only typing.
        if value and self.console.visible:
            return
        self.keys[name] = value

    # -- the mouse ------------------------------------------------------------

    def _set_mouse_captured(self, captured):
        """Take the pointer for looking, or hand it back.

        Relative mouse mode is asked for rather than assumed: where the
        platform has it, the pointer is unhooked from the screen and its motion
        arrives as motion, which is what makes a fast turn keep turning instead
        of stopping at the edge of the window. Where it does not -- WSL over
        X11 is the case at hand -- the fallback is the old recipe of reading
        the pointer and putting it back in the middle every frame, and
        `_read_mouse` handles both by looking at what the window actually
        granted rather than at what was asked for.
        """
        if captured == self._mouse_captured:
            return
        self._mouse_captured = captured

        props = WindowProperties()
        props.set_cursor_hidden(captured)
        props.set_mouse_mode(WindowProperties.M_relative if captured
                             else WindowProperties.M_absolute)
        self.win.request_properties(props)

        # Whatever the pointer was doing before belongs to the other mode.
        self._pointer_origin = None
        self._warp_frames = 0
        self._mouse_anchor = None
        self._dragging = False

    def _mouse_button(self, button, down):
        if self.console.visible:
            return
        if not self._mouse_captured:
            # A click on the window is how the pointer is taken back, and a
            # drag is how the view is swung while it is not.
            if down and button == 1 and not self.console.wants_mouse():
                self._set_mouse_captured(True)
                return
            self._dragging = down
            return

        if button == 3:
            self.keys["aim"] = down
        elif button == 1:
            # The trigger, and what it does is give the squad an order -- the
            # same press and release the X key carries, so a tap sends and a
            # hold whistles.
            (self._squad_press if down else self._squad_release)()

    def _read_mouse(self):
        """Turn the pointer's movement into a look, and keep it off the edges.

        The delta is always against the last reading, never against the middle
        of the window, and that is the whole design. Where relative mouse mode
        is available the pointer free-runs and there is nothing else to do.
        Where it is not -- WSL over X11 is the case at hand, and it says so in
        the log -- the pointer has to be shoved back to the middle before it
        reaches an edge and stops reporting, and *that* is the part worth being
        careful about:

        `move_pointer` does not land synchronously. Reading the pointer back
        immediately after warping it still returns the old position; the warp
        arrives at some point before the next frame's read, or it does not. So
        a delta taken as `pointer - centre`, which assumes last frame's warp
        landed, is wrong exactly when it did not, and wrong by the width of the
        window -- which is how a single frame ends up turning the view ninety
        degrees and pinning the pitch at its limit. It cost an afternoon.

        Taking the delta against the last *observed* position removes the
        assumption. The one frame that cannot be read that way is the one the
        warp lands on, since it is not motion; that frame is dropped, which
        loses a sixtieth of a second of hand movement every couple of hundred
        pixels of travel and is not detectable. The warp is only asked for near
        an edge, so it is rare to begin with.
        """
        if not self._mouse_captured:
            return
        props = self.win.get_properties()
        if not props.get_foreground():
            # Whatever the pointer does while another window has it is not a
            # look, and where it comes back is not a delta.
            self._pointer_origin = None
            return

        pointer = self.win.get_pointer(0)
        current = (pointer.get_x(), pointer.get_y())
        origin, self._pointer_origin = self._pointer_origin, current

        if self._warp_frames > 0:
            # A warp is in flight. Drop readings until one arrives near where
            # it was sent -- that one is a good position to measure the next
            # frame against, so it is kept as the origin above.
            self._warp_frames -= 1
            if math.dist(current, self._window_centre()) < MOUSE_SETTLED:
                self._warp_frames = 0
            elif self._warp_frames == 0:
                # Given up on: a warp that never landed leaves the pointer
                # somewhere unrelated to where it was sent, and the difference
                # between the two is not hand movement. Take no delta at all
                # next frame rather than that one.
                self._pointer_origin = None
            return

        if origin is not None:
            # Panda3D's pointer counts down the screen and the camera counts up.
            self.follow_camera.look_mouse(current[0] - origin[0],
                                          -(current[1] - origin[1]))

        if props.get_mouse_mode() == WindowProperties.M_relative:
            # Free-running: it has no edge to reach.
            return

        # Keep it inside the middle of the window, where it has room to move in
        # every direction before the screen stops it.
        centre = self._window_centre()
        if (abs(current[0] - centre[0]) > centre[0] * MOUSE_MARGIN
                or abs(current[1] - centre[1]) > centre[1] * MOUSE_MARGIN):
            if self.win.move_pointer(0, *centre):
                self._warp_frames = MOUSE_WARP_FRAMES

    def _window_centre(self):
        return (self.win.get_x_size() // 2, self.win.get_y_size() // 2)

    def _toggle_skating(self):
        if self.console.visible:
            return
        self._skating = not self._skating

    def _escape(self):
        """Back out of whatever has the input, and quit only once nothing does.

        Three steps rather than one, because a captured pointer is a mode the
        player is in: quitting straight out of it would mean the key that gets
        the cursor back is also the key that closes the game.
        """
        if self.console.visible:
            self.console.hide()
            return
        if self._mouse_captured:
            self._set_mouse_captured(False)
            print("Mouse released -- click the window to look with it again.")
            return
        sys.exit()

    def _toggle_debug(self):
        self._show_debug = not self._show_debug
        # The console draws the readout itself while it is open, so leave it
        # hidden and let the toggle take effect when the console goes away.
        if self.console.visible:
            return
        (self.hud.show if self._show_debug else self.hud.hide)()

    def _toggle_collision(self):
        if self.collision_view.is_hidden():
            self.collision_view.show()
            self.level.hide()
        else:
            self.collision_view.hide()
            self.level.show()

    def _setup_audio(self):
        """Prepare the sound bank, synthesising stand-in samples if needed.

        The decomp has no audio -- its samples come out of a ROM at build
        time, exactly as the textures do -- so placeholders are generated once
        into assets/sounds/. Replace them with real files of the same name to
        get real audio. Failing to set any of this up is not fatal: the sound
        bank then does nothing, which is the usual outcome under WSL.
        """
        try:
            written = audio.generate_placeholders(SOUNDS, audio.USED_SOUNDS)
            if written:
                print(f"Synthesised {len(written)} placeholder sounds in {SOUNDS}")
        except OSError as exc:
            print(f"Could not write placeholder sounds: {exc}")

        manager = self.sfxManagerList[0] if self.sfxManagerList else None
        self.sounds = audio.SoundBank(SOUNDS, manager)

        # Always report the outcome. Silent audio is otherwise indistinguishable
        # from audio that was never wired up, and under WSL the difference is
        # usually the device, not the code.
        if self.sounds.enabled:
            count = len([f for f in os.listdir(SOUNDS) if f.endswith(".wav")]) \
                if os.path.isdir(SOUNDS) else 0
            source = audio.imported_from(SOUNDS)
            print(f"Audio: {type(manager).__name__} ready, {count} samples")
            if source:
                print(f"       imported from {source}")
            else:
                print("       synthesised placeholders -- run "
                      "tools/import_sounds.py for the real ones.")
        else:
            print("Audio: no usable device, running silent. "
                  "(Under WSL this is normal.)")

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

    def _setup_crosshair(self):
        """A reticle at the centre of the screen, in two parts.

        Two passes of the same four strokes, a thick dark one under a thin
        light one, because a single-colour crosshair disappears against
        whatever it happens to be over -- and over the castle grounds that is
        white in the sky, dark green on the hill and both at once on the
        skyline. The outline costs one more draw and works everywhere.

        The four ticks live under a node of their own so the spread can scale
        them without touching the dot in the middle, which has to stay the size
        it is: it marks a point, and a point that grows is not one.

        Drawn in aspect2d, whose centre is (0, 0) and whose vertical range is
        -1 to 1 either way, so it stays put and stays the same size as the
        window is resized.
        """
        self.crosshair = self.aspect2d.attach_new_node("crosshair")
        self._crosshair_arms = self.crosshair.attach_new_node("arms")
        for thickness, colour in ((4.5, (0.0, 0.0, 0.0, 0.55)),
                                  (2.0, (1.0, 1.0, 1.0, 0.9))):
            segs = LineSegs()
            segs.set_thickness(thickness)
            segs.set_color(*colour)
            # Four ticks around a gap rather than a solid cross: the gap is
            # what keeps the thing being aimed at visible.
            for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                segs.move_to(dx * CROSSHAIR_GAP, 0.0, dz * CROSSHAIR_GAP)
                segs.draw_to(dx * (CROSSHAIR_GAP + CROSSHAIR_ARM), 0.0,
                             dz * (CROSSHAIR_GAP + CROSSHAIR_ARM))
            self._crosshair_arms.attach_new_node(segs.create())

        # The dot, hidden until the sights are up. Drawn as a short fat stroke
        # rather than as a disc: at this size the two are the same handful of
        # pixels, and one of them is four vertices.
        segs = LineSegs()
        segs.set_thickness(5.0)
        segs.set_color(1.0, 1.0, 1.0, 1.0)
        segs.move_to(-CROSSHAIR_DOT, 0.0, 0.0)
        segs.draw_to(CROSSHAIR_DOT, 0.0, 0.0)
        self._crosshair_dot = self.crosshair.attach_new_node(segs.create())

        self.crosshair.set_transparency(TransparencyAttrib.M_alpha)
        self._spread = 1.0
        self._spread_vel = 0.0

    def _update_crosshair(self, dt):
        """Open the reticle with his movement and close it with the sights."""
        aim = self.follow_camera.aim_amount
        state = self.state
        speed = min(abs(state.forward_vel) / max(HC.MAX_RUN_SPEED, 1.0), 1.0)

        airborne = (state.floor is None
                    or state.pos[1] - state.floor_height > 60.0)
        target = 1.0 + (CROSSHAIR_SPREAD_RUNNING - 1.0) * speed
        if airborne:
            target += CROSSHAIR_SPREAD_AIR
        # The sights do not merely subtract from the spread, they take it over:
        # a settled aim is settled however fast he was going a moment ago.
        target += (CROSSHAIR_SPREAD_AIM - target) * aim

        self._spread, self._spread_vel = smooth_damp(
            self._spread, target, self._spread_vel,
            CROSSHAIR_SPREAD_SMOOTH, dt)
        self._crosshair_arms.set_scale(self._spread)

        alpha = CROSSHAIR_HIP_ALPHA + (1.0 - CROSSHAIR_HIP_ALPHA) * aim
        self.crosshair.set_color_scale(1.0, 1.0, 1.0, alpha)
        self._crosshair_dot.set_color_scale(1.0, 1.0, 1.0, aim * aim)

    # -- the squad -----------------------------------------------------------

    def _setup_squad(self):
        """The allies, and the node the aiming reticle is rebuilt into.

        The Marios are the squad: they are the only things in the field that
        are on your side, and they already fight, so an ally who has been told
        where to stand is an ally who fights there instead of wherever he
        happened to wander.
        """
        self.squad = squad.Squad(self.objects, objects.Mario)

        self.aim_node = self.render.attach_new_node("squad-aim")
        self.aim_node.set_transparency(TransparencyAttrib.M_alpha)
        # Depth tested but not written: a circle behind the hill is hidden by
        # it, which is what makes it read as lying on the ground, while the arc
        # crossing itself does not carve a hole out of its own far half.
        self.aim_node.set_depth_write(False)
        self.aim_node.set_light_off()
        self.aim_node.hide()

        # Seconds the squad button has been down, or None if it is not. The
        # aim point it last resolved to, so the release commands the same spot
        # that was being drawn rather than one a frame further on.
        self._aim_hold = None
        self._aim_point = None
        self._reticle_fade = 0.0

    def _squad_press(self):
        if self.console.visible:
            return
        self._aim_hold = 0.0
        # Nothing has been aimed for this press yet. Cleared rather than left,
        # so a press and release inside one frame cannot command the spot the
        # last one was drawn at.
        self._aim_point = None

    def _squad_release(self):
        """A tap sends the squad; a hold whistles up a new one.

        Both take the aim point the last frame drew rather than resolving a
        fresh one: it is the spot the player was looking at when they let go,
        and it is the one they were shown.
        """
        if self._aim_hold is None:
            return
        held, self._aim_hold = self._aim_hold, None
        target = self._aim_point
        if target is None:
            # Down and up inside a single frame -- a fast tap on a fast
            # machine. Resolve the aim here rather than dropping the command,
            # which is the one thing a button press must never do.
            target = squad.aim_point(self.surfaces, self.follow_camera,
                                     self.state.pos, self.state.face_angle[1])
            self._aim_point = target

        radius = None
        if held < squad.TAP_SECONDS:
            sent = self.squad.send(target)
            if sent:
                print(f"Squad: {sent} sent out")
        else:
            radius = squad.circle_radius(held - squad.TAP_SECONDS)
            joined = self.squad.recruit(target, radius)
            if joined:
                print(f"Squad: {joined} joined, {len(self.squad.members)} following")

        # Leave the reticle up for a moment, fading. Redrawn rather than left
        # as it was, so what fades is the spot and the circle that were
        # actually commanded -- which for a tap is a ring and no circle at all.
        self._draw_reticle(self.state.pos, target, radius)
        self.aim_node.set_color_scale(1.0, 1.0, 1.0, 1.0)
        self.aim_node.show()
        self._reticle_fade = RETICLE_FLASH

    def _cancel_aim(self):
        """Drop a half-made command -- what opening the console does to one."""
        self._aim_hold = None
        self._reticle_fade = 0.0
        self.aim_node.hide()

    def _selection_radius(self):
        """The whistle circle's radius, or None while the press is still a tap.

        Nothing is drawn for the first fifth of a second because nothing is
        decided yet: a press that comes up inside it was an order to the squad
        that already exists, and a circle flashing on every one of those would
        say the opposite.
        """
        held = self._aim_hold
        if held is None or held < squad.TAP_SECONDS:
            return None
        return squad.circle_radius(held - squad.TAP_SECONDS)

    def _update_aim(self, dt, player_pos):
        """Resolve and draw the reticle, or fade out the last one."""
        if self._aim_hold is not None:
            self._aim_hold += dt
            self._aim_point = squad.aim_point(
                self.surfaces, self.follow_camera, player_pos,
                self.state.face_angle[1])
            self._draw_reticle(player_pos, self._aim_point,
                               self._selection_radius())
            self.aim_node.set_color_scale(1.0, 1.0, 1.0, 1.0)
            self.aim_node.show()
            return

        if self._reticle_fade > 0.0:
            self._reticle_fade -= dt
            if self._reticle_fade <= 0.0:
                self.aim_node.hide()
            else:
                # Whatever the release drew, fading out where it was drawn.
                self.aim_node.set_color_scale(
                    1.0, 1.0, 1.0, self._reticle_fade / RETICLE_FLASH)

    def _draw_reticle(self, player_pos, target, radius):
        """Rebuild the arc, the target ring and the whistle circle.

        Rebuilt from scratch every frame rather than transformed, because the
        two things that change are the shape of the arc and how the rings
        follow the ground under them, and neither is a transform.
        """
        self.aim_node.node().remove_all_children()

        segs = LineSegs()
        segs.set_thickness(RETICLE_THICKNESS)

        start = (player_pos[0], player_pos[1] + AIM_CHEST, player_pos[2])
        points = squad.throw_arc(start, target)

        # Rungs across the arc, at right angles to the throw.
        #
        # The aim is always straight away from the camera, so the whole arc --
        # and anything else in the vertical plane it flies through, including a
        # shadow drawn under it -- projects to one vertical line on screen and
        # reads as a pole rather than as something going over. A rung across it
        # is the one part that is not in that plane: the rungs come out
        # horizontal, and they crowd together toward the top, which is what the
        # height of the lob looks like from behind it.
        across = _across(player_pos, target, ARC_RUNG_HALF_WIDTH)
        segs.set_color(1.0, 1.0, 1.0, 0.45)
        for i in range(ARC_RUNG_EVERY, len(points) - 1, ARC_RUNG_EVERY):
            x, y, z = points[i]
            segs.move_to(*to_panda(x - across[0], y, z - across[1]))
            segs.draw_to(*to_panda(x + across[0], y, z + across[1]))

        # The arc itself, dashed: drawn as every other segment, which reads as
        # travel rather than as a length of wire between him and the ground.
        segs.set_color(1.0, 1.0, 1.0, 0.85)
        for i in range(0, len(points) - 1, 2):
            segs.move_to(*to_panda(*points[i]))
            segs.draw_to(*to_panda(*points[i + 1]))

        segs.set_color(1.0, 0.80, 0.25, 0.9)
        self._ring(segs, target, TARGET_RING_RADIUS, TARGET_RING_POINTS,
                   snap=False)

        if radius is not None:
            segs.set_color(0.35, 1.0, 0.45, 0.85)
            self._ring(segs, target, radius, SELECT_RING_POINTS, snap=True)

        self.aim_node.attach_new_node(segs.create())

    def _ring(self, segs, centre, radius, points, snap):
        """A circle on the ground about `centre`.

        `snap` traces it over whatever it crosses, one collision query per
        point. Worth it for the whistle circle, which is wide enough to span a
        slope and would otherwise cut into it or float over it; not worth it
        for the small target ring, which is flat enough at that size.
        """
        for i in range(points + 1):
            angle = i / points * math.tau
            x = centre[0] + radius * math.sin(angle)
            z = centre[2] + radius * math.cos(angle)
            y = centre[1]
            if snap:
                height, floor = self.surfaces.find_floor(
                    x, centre[1] + 400.0, z)
                if floor is not None:
                    y = height
            point = to_panda(x, y + RETICLE_LIFT, z)
            if i == 0:
                segs.move_to(*point)
            else:
                segs.draw_to(*point)

    # -- console -------------------------------------------------------------

    def _setup_console(self):
        self.console = console.Console(
            self, self._register_tunables(),
            log=console.capture_output(),
            readout=self._hud_text,
            on_toggle=self._console_toggled,
        )

    def _register_tunables(self):
        """The numbers the console is allowed to put on a slider.

        Every one of these is looked up where it is used, on the frame it is
        used -- the action code says `H.MAX_RUN_SPEED` rather than copying it
        into the state -- so writing to the module is all it takes for a drag
        to land on the next 30 Hz tick. Anything cached at startup would need
        a setter that re-applies it, which is why the camera's two are
        registered against the camera object rather than against its module.
        """
        t = console.Tunables()

        # Both caps, together: the walking cap is what he accelerates toward
        # and the running cap is the ceiling nothing may exceed, and a Hero
        # whose ceiling sat below his target would accelerate into a wall he
        # could never cross.
        #
        # 120 rather than something rounder because that is where the movement
        # stops being trustworthy: the ground step splits a frame into four,
        # and at 120 units a frame each quarter is 30 units -- still inside the
        # 50-unit wall check, so he is stopped by walls rather than passing
        # through them. Mario's own top speed is 32, for scale.
        t.add("run_speed", HC, ("MAX_WALK_SPEED", "MAX_RUN_SPEED"), 0.0, 120.0,
              "top speed, units per 30 Hz frame (Mario's own is 32)")
        t.add("walk_accel", HC, "WALK_ACCEL", 0.0, 8.0,
              "acceleration per frame, before the taper")
        t.add("accel_taper", HC, "ACCEL_TAPER", 5.0, 200.0,
              "acceleration falls away as speed approaches this")
        t.add("decel", HC, "DECELERATION", 0.0, 10.0,
              "slowing per frame with no stick")
        t.add("brake_decel", HC, "BRAKE_DECELERATION", 0.0, 10.0,
              "slowing per frame when stopping hard")
        t.add("turn_rate", HC, "TURN_RATE", 0x0100, 0x4000,
              "s16 angle units per frame", integer=True)
        t.add("run_anim_speed", HC, "RUN_SPEED", 0.0, 32.0,
              "speed the run cycle takes over from the walk")

        t.add("jump_velocity", HC, "JUMP_VELOCITY", 0.0, 120.0,
              "take-off speed; 42 is the ~250 unit jump the level was built for")
        t.add("jump_speed_bonus", HC, "JUMP_SPEED_BONUS", 0.0, 1.0,
              "how much a running start lends to the jump")

        # Gravity takes 4 a frame back after every air step, so a thrust at or
        # under that hovers rather than climbs -- hence the floor of 4.
        t.add("jetpack_thrust", HC, "JETPACK_THRUST", 4.0, 40.0,
              "how hard the boosters push, per frame")
        t.add("jetpack_rise", HC, "JETPACK_RISE_SPEED", 0.0, 80.0,
              "the climb it settles at, units per frame")
        t.add("jetpack_launch", HC, "JETPACK_LAUNCH_SPEED", 0.0, 80.0,
              "the kick A gives when it takes him off his skates")

        t.add("skate_push", HC, "SKATE_PUSH", 0.0, 6.0,
              "speed the jets add per frame at full stick")
        t.add("skate_top", HC, "SKATE_TOP_SPEED", 0.0, 120.0,
              "how fast the skates will carry him")

        t.add("attack_lunge", HC, "ATTACK_LUNGE_SPEED", 0.0, 40.0,
              "the forward travel handed back to the sword swings")
        t.add("spin_kick_speed", HC, "SPIN_KICK_SPEED", 0.0, 60.0,
              "how fast the spin kick carries him")
        t.add("spin_kick_min", HC, "SPIN_KICK_MIN_SPEED", 0.0, 32.0,
              "speed needed to spin kick rather than swing")
        t.add("wade_scale", HC, "WADE_SPEED_SCALE", 0.05, 1.0,
              "what deep water leaves of his speed")

        # The aim layer's feel, on the profile the Hero is actually carrying
        # rather than on the module default, so a slider moves the character on
        # screen. docs/aim.md's weapon-specific numbers all live on this object;
        # when there is more than one weapon there will be more than one of it.
        aim_profile = self.players[0].aim.profile
        t.add("torso_limit", aim_profile, "yaw_limit", 15.0, 90.0,
              "how far the torso twists before his feet have to help")
        t.add("torso_response", aim_profile, "response", 0.02, 0.6,
              "spring time of the torso, seconds -- smaller is snappier")
        t.add("torso_pitch", aim_profile, "pitch_share", 0.0, 1.0,
              "how much of the shot's elevation the torso leans into")
        t.add("torso_comfort", aim_profile, "comfort_yaw", 0.0, 60.0,
              "twist he will square up out of when standing still")
        t.add("torso_turn_rate", aim_profile, "turn_rate", 0.5, 20.0,
              "how fast his feet come round to the excess")

        # The squad's numbers are module-level in sm64py/squad.py and read on
        # the frame they are used, the same way the movement constants are, so
        # a drag lands on the next command rather than needing a restart.
        t.add("squad_range", squad, "AIM_MAX_RANGE", 500.0, 6000.0,
              "how far out the aim can put a spot")
        t.add("squad_circle", squad, "CIRCLE_MAX_RADIUS", 300.0, 3000.0,
              "how wide the whistle circle grows")
        t.add("squad_grow", squad, "CIRCLE_GROW_SECONDS", 0.2, 4.0,
              "seconds for the circle to reach that size")
        t.add("squad_follow", squad, "FOLLOW_DISTANCE", 100.0, 1200.0,
              "how far behind you the group gathers")

        # The two numbers that decide how busy the field is. Both are written
        # straight onto the pipes, which read them on the tick they use them,
        # so a drag lands on the next spawn rather than the next run.
        t.add("enemy_rate", self.pipe_tuning, "seconds", *ENEMY_RATE_RANGE,
              doc="seconds between one enemy out of each pipe and the next")
        t.add("enemy_limit", self.pipe_tuning, "limit", *ENEMY_LIMIT_RANGE,
              doc="how many each enemy pipe keeps alive", integer=True)

        # The camera's, which are registered against the object rather than
        # against its module because it reads them off itself: a slider that
        # wrote to `sm64py.camera.HIP_DISTANCE` would move nothing, since the
        # constant was copied into the instance when it was built.
        cam = self.follow_camera
        t.add("cam_distance", cam, "distance", 250.0, 4000.0,
              "how far the camera sits behind him at the hip")
        t.add("cam_aim_distance", cam, "aim_distance", 150.0, 2000.0,
              "and how far behind him down the sights")
        t.add("cam_height", cam, "height", -200.0, 800.0,
              "how far above his feet the boom pivots")
        t.add("cam_shoulder", cam, "shoulder", -600.0, 600.0,
              "how far off the centre line he stands; negative swaps shoulders")
        t.add("cam_aim_shoulder", cam, "aim_shoulder", -600.0, 600.0,
              "the same, down the sights")
        t.add("cam_fov", cam, "base_fov", 25.0, 100.0,
              "field of view, degrees across")
        t.add("cam_aim_fov", cam, "aim_fov", 15.0, 90.0,
              "and what the sights pull it in to")
        t.add("cam_follow", cam, "follow_smooth", 0.0, 0.5,
              "seconds the camera takes to close on him -- 0 nails it to him")
        t.add("cam_shake", cam, "shake_scale", 0.0, 3.0,
              "how much a landing kicks the camera")

        t.add("mouse_sens", cam, "mouse_sensitivity", 2.0, 90.0,
              "degrees of view per hundred pixels of mouse")
        t.add("mouse_smooth", cam, "mouse_smoothing", 0.0, 0.12,
              "seconds a mouse delta is spread over; 0 is raw 1:1")
        t.add("stick_sens", cam, "stick_speed", 40.0, 700.0,
              "degrees per second at a full push of the look stick")
        t.add("stick_pitch", cam, "stick_pitch_speed", 30.0, 500.0,
              "the same, up and down")
        return t

    def _console_toggled(self, visible):
        """The console has the keyboard while it is open, and the readout.

        Held keys are dropped rather than left set, since the key-up for a key
        pressed before the console opened arrives while it is open and would
        otherwise be the only thing that ever cleared it.
        """
        if visible:
            self.hud.hide()
            # The console's panel is drawn in aspect2d too, and the crosshair
            # would sit on top of the text rather than behind it.
            self.crosshair.hide()
            # A whistle half-grown when the console opened has no release
            # coming: the key-up lands while the console has the keyboard.
            self._cancel_aim()
            # The console has sliders to drag, which needs a pointer.
            self._was_captured = self._mouse_captured
            self._set_mouse_captured(False)
            for name in self.keys:
                self.keys[name] = False
            self._aim_toggle = False
            self.follow_camera.set_aim(0.0)
            self._freeze_animation()
        else:
            self.crosshair.show()
            if self._was_captured:
                self._set_mouse_captured(True)
            if self._show_debug:
                self.hud.show()

    def _freeze_animation(self):
        """Hold every clip on its current frame while the game is paused.

        The simulation stops by simply not being stepped, but the clips are
        played by Panda3D off the render clock and would carry on walking on
        the spot. Nothing here has to be undone: `ObjectRenderer.sync` and
        `_update_animation` set every play rate from scratch each frame, and
        neither runs again until the game does.
        """
        self.object_renderer.freeze()
        player = self.player
        if player.actor is not None and player.current_anim is not None:
            player.actor.set_play_rate(0.0, player.current_anim)

    def _poll_controller(self):
        # Typing `run_speed` should not walk him across the field.
        if self.console.visible:
            self.controller.set_stick(0.0, 0.0)
            self.controller.set_buttons(0)
            self.controller.set_thrust(False)
            self.controller.zombie = False
            self.controller.skating = self._skating
            return

        right = 1.0 if self.keys["right"] else 0.0
        left = 1.0 if self.keys["left"] else 0.0
        up = 1.0 if self.keys["up"] else 0.0
        down = 1.0 if self.keys["down"] else 0.0

        # The pad's stick, added rather than chosen between: a key held while
        # the stick is pushed the other way cancels out, which is what either
        # one alone does anyway, and there is no mode to be in. `set_stick`
        # clamps the pair back inside the circular gate, so the sum of the two
        # cannot walk him faster than either.
        pad_x, pad_y = self.gamepad.stick

        # Both stick axes come out mirrored from screen space: the heading is
        # built as atan2s(-stick_y, stick_x) and then rotated by the camera
        # yaw, which flips Y and mirrors X. Hence left-minus-right, not the
        # other way round -- and hence the pad's axes, which point right and
        # up, coming through negated.
        self.controller.set_stick(left - right - pad_x, down - up - pad_y)

        buttons = self.gamepad.buttons
        if self.keys["a"]:
            buttons |= C.A_BUTTON
        if self.keys["b"]:
            buttons |= C.B_BUTTON
        if self.keys["z"]:
            buttons |= C.Z_TRIG
        self.controller.set_buttons(buttons)

        # The jetpack's own control: on the ground it skates him, in the air it
        # flies him, and it is the only thing that does either.
        self.controller.set_thrust(self.gamepad.thrust or self.keys["thrust"])

        # Purely a costume: the action code never reads it, so Mario walks and
        # jumps exactly as he always did while it is held.
        self.controller.zombie = self.keys["zombie"] or self.gamepad.zombie
        # The skates are not a costume -- this one drives an action.
        self.controller.skating = self._skating

    # -- loop ----------------------------------------------------------------

    def _record_frame(self, frame_dt):
        """Retain frame durations from the last few seconds of real time."""
        now = time.monotonic()
        self._frame_history.append((now, max(frame_dt, 0.0)))
        cutoff = now - FRAME_HISTORY_SECONDS
        while self._frame_history and self._frame_history[0][0] < cutoff:
            self._frame_history.popleft()

    def _slowest_frame(self):
        """The longest rendered frame in the rolling history, in seconds."""
        return max((dt for _, dt in self._frame_history), default=0.0)

    def _update(self, task):
        frame_dt = self.clock.get_dt()
        self._record_frame(frame_dt)
        dt = min(frame_dt, 0.25)

        # Once a frame, before anything reads it, and held neutral while the
        # console has the input -- the pad has no key-up to arrive later, so a
        # direction held as the console opened would otherwise stay held.
        self.gamepad.poll(active=not self.console.visible)
        # The two controls that latch rather than being held. Read here rather
        # than in the tick loop because a frame can run two ticks or none, and
        # a press is a press either way.
        if self.gamepad.pressed("skates"):
            self._toggle_skating()
        if self.gamepad.pressed("swap"):
            self._switch_player()
        if self.gamepad.pressed("zoom"):
            self._toggle_zoom()
        if self.gamepad.pressed("squad"):
            self._squad_press()
        if self.gamepad.released("squad"):
            self._squad_release()

        # The console pauses the game. Nothing accumulates while it is open
        # either, so coming back steps a single tick rather than replaying
        # however long was spent typing -- and since the task still runs every
        # frame, the clock's dt is one frame's worth, not the whole pause.
        if self.console.visible:
            self.console.update(dt)
            return task.cont

        # Before the ticks, so the frame's own look and the frame's own aim are
        # what the simulation is stepped against rather than the last one's.
        self._update_aim_mode()
        self._update_look(dt)
        self._update_torso_aim(dt)

        # Step the simulation in whole 30 Hz ticks.
        self._accumulator += dt
        steps = 0
        while self._accumulator >= TICK_DT and steps < 8:
            state = self.state
            # Remember where he was so the render can interpolate out of it.
            self._prev_pos = list(state.gfx_pos)
            self._prev_angle = list(state.gfx_angle)
            falling = state.vel[1]
            grounded = state.floor is not None and \
                state.pos[1] - state.floor_height < 1.0

            self._poll_controller()
            state.camera_yaw = self.follow_camera.mario_yaw
            self.player.execute(state)
            # Before the objects move, so an ally reads a goal set against
            # where the leader is this tick rather than the last one.
            self.squad.update(state)
            self.objects.update(state)
            self._stand_down_mario_npcs()
            # After both have moved, so a stomp is judged on where they
            # actually ended up rather than where they started.
            self.interactions.resolve(state)
            # Drained inside the tick loop, not after it: a frame that runs
            # two ticks would otherwise drop the first tick's sounds.
            self.sounds.play_events(state)
            self._kick_on_landing(grounded, falling)
            self._accumulator -= TICK_DT
            steps += 1

            # Standing in for the death warp: losing the floor entirely would
            # otherwise mean falling forever.
            if state.floor is None or state.pos[1] < DEATH_PLANE:
                state.spawn(*SPAWN, SPAWN_YAW)
                self._reset_interpolation()

        # The simulation only moves in 33 ms steps, so drawing its raw output
        # judders on a faster display. Draw a blend between the last two ticks
        # instead, positioned by however much time is left in the accumulator.
        alpha = min(max(self._accumulator / TICK_DT, 0.0), 1.0)
        pos, angle = self._interpolated_transform(alpha)

        self.follow_camera.update(
            dt, target_pos=pos,
            recenter=self.keys["cam_center"] or self.gamepad.recenter)
        self.follow_camera.apply_to(self.camera, self.camLens)

        # After the camera has been placed: the aim is a ray out of it, and
        # taking it beforehand would be aiming through last frame's view.
        self._update_aim(dt, pos)
        self._update_crosshair(dt)

        # Its own clock rather than the frame time, so a spell in the console
        # does not leave the sheet somewhere else when the game comes back.
        self._water_time += dt
        animate_water(self.water, self._water_time)
        # Slide every object's drawn position across the tick it is between, the
        # same blend the player is drawn at, so a reduced-rate crowd moves
        # smoothly rather than stepping. Must precede the renderers, which read
        # the interpolated position.
        self.objects.interpolate(alpha)

        # Anything a pipe has produced needs a node before it can be drawn.
        self.object_renderer.refresh(self.objects)
        self.object_renderer.sync(self.follow_camera.pos)
        # The enemy crowds are drawn straight from their simulation state, one
        # instanced quad each, with no per-object node to create or refresh.
        self.impostor_drawn = self.impostors.update(
            self.objects, self.follow_camera.pos)
        self._update_camera_medium()

        self.player.node.set_pos(*to_panda(*pos))
        # Panda3D takes heading, pitch, roll; the port stores pitch, yaw, roll.
        self.player.node.set_hpr(s16_to_degrees(angle[1]),
                                 s16_to_degrees(angle[0]),
                                 s16_to_degrees(angle[2]))
        self._update_animation()

        self.console.update(dt)

        self._hud_timer -= dt
        if self._show_debug and not self.console.visible and self._hud_timer <= 0.0:
            self._hud_timer = HUD_INTERVAL
            self.hud.setText(self._hud_text())

        return task.cont

    def _kick_on_landing(self, was_grounded, falling):
        """Shake the camera for a landing, in proportion to the fall.

        Taken from the tick rather than from the action he ended up in: both
        characters have several landing actions and the pair that matter here
        -- was he in the air, and how fast was he going down -- are the same
        two questions whichever of them he takes.
        """
        state = self.state
        if was_grounded or falling > -LAND_SHAKE_MIN_SPEED:
            return
        if state.floor is None or state.pos[1] - state.floor_height > 1.0:
            return
        share = ((-falling - LAND_SHAKE_MIN_SPEED)
                 / (LAND_SHAKE_FULL_SPEED - LAND_SHAKE_MIN_SPEED))
        self.follow_camera.shake(min(share, 1.0) * LAND_SHAKE_AMOUNT)

    def _interpolated_transform(self, alpha):
        state = self.state
        current = state.gfx_pos
        pos = [
            self._prev_pos[i] + (current[i] - self._prev_pos[i]) * alpha
            for i in range(3)
        ]
        # All three angles, because swimming pitches and rolls him; on land the
        # other two are simply zero. Each turns the short way round, so
        # crossing the angle wrap does not spin.
        angle = [
            self._prev_angle[i]
            + s16(state.gfx_angle[i] - self._prev_angle[i]) * alpha
            for i in range(3)
        ]
        return pos, angle

    def _reset_interpolation(self):
        """Drop the blend after a teleport, so he does not streak across."""
        self._prev_pos = list(self.state.gfx_pos)
        self._prev_angle = list(self.state.gfx_angle)
        self._accumulator = 0.0

    def _update_look(self, dt):
        """Everything that points the view, gathered into one call.

        The three sources are deliberately not three code paths: the keys and
        the pad's right stick are summed into one virtual look stick and go
        through the camera's response curve and turn ramp together, so Q held
        down accelerates into a turn exactly the way the stick does, and the
        mouse -- which reports movement rather than a held position, and so
        must not be scaled by dt -- is the one that goes its own way.
        """
        keys_x = (1.0 if self.keys["cam_right"] else 0.0) \
            - (1.0 if self.keys["cam_left"] else 0.0)
        pad_x, pad_y = self.gamepad.camera
        if self.gamepad.boom:
            # Left stick held in, and the right one stops being the view and
            # becomes the boom: push it forward and the camera comes in over
            # his shoulder, pull it back and it stands off. Both axes are taken
            # away from the look rather than only the one being used, so the
            # drift on a diagonal cannot quietly turn the view while the
            # distance is being set.
            self.follow_camera.dolly(-pad_y, dt)
            pad_x = pad_y = 0.0
        self.follow_camera.look_stick(keys_x + pad_x, pad_y, dt)

        self._read_mouse()

        # Dragging with a mouse button held swings the view while the pointer
        # is loose -- unless it is over the console or one of its sliders,
        # where a drag means the slider and nothing else.
        if (self._dragging and not self._mouse_captured
                and not self.console.wants_mouse()
                and self.mouseWatcherNode.has_mouse()):
            pos = self.mouseWatcherNode.get_mouse()
            current = (pos.get_x(), pos.get_y())
            if self._mouse_anchor is not None:
                # A drag grabs the world rather than the view, which is the
                # opposite sign to the captured pointer and the right one for
                # a hand that is holding something.
                self.follow_camera.look(
                    -(current[0] - self._mouse_anchor[0]) * DRAG_YAW,
                    -(current[1] - self._mouse_anchor[1]) * DRAG_PITCH)
            self._mouse_anchor = current
        else:
            self._mouse_anchor = None

    def _update_aim_mode(self):
        """Whether to be down the sights, from whichever control is asking.

        The pad latches and the other two are held, and that is not an
        inconsistency: clicking the right stick is a thing you do with the
        thumb that is aiming, so it cannot be held down while you aim with it.
        A mouse button and a key can both be held, so they are.
        """
        held = self.keys["aim"] or self._aim_toggle
        self.follow_camera.set_aim(1.0 if held else 0.0)

    def _toggle_zoom(self):
        self._aim_toggle = not self._aim_toggle

    def _tracking_strength(self):
        """How much of the aim the upper body is currently following.

        Two things ask for the torso, and the stronger of them wins.

        **The sights**, at whatever fraction they are blended in at, so the
        upper body comes round with the camera as the view closes rather than
        snapping to it the instant the button goes down. Out of the sights it
        is zero: a torso that swivelled after the mouse while he was running
        about would read as a bug rather than as aiming.

        **A swing in progress**, at whatever docs/aim.md's commitment curve
        allows this far into it. This is the melee half of the doc, and the
        reason it is `max` rather than a product: the interesting case is a
        sword swing with the sights *down*, where the attack steers toward the
        crosshair during the windup and stops listening once the blade is out.
        """
        state = self.state
        strength = self.follow_camera.aim_amount
        # The Hero's two attacks by name rather than by the attacking flag:
        # Mario's punches carry it too, and his are the decomp's animations
        # timed by the decomp's tables, with no windup this file may reinterpret.
        if state.action in (HC.ACT_HERO_ATTACK, HC.ACT_HERO_SPIN_KICK):
            length = HC.SPIN_KICK_FRAMES
            if state.action == HC.ACT_HERO_ATTACK:
                length = (HC.ATTACK1_FRAMES if state.combo_index == 0
                          else HC.ATTACK2_FRAMES)
            # From the action timer rather than from the clip's playhead: it is
            # the number the attack's own hit windows are cut against, so the
            # tracking and the gameplay agree about how far in he is.
            strength = max(strength,
                           melee_tracking(state.action_timer / max(length, 1)))
        return strength

    def _update_torso_aim(self, dt):
        """Point the upper body down the sights, and the feet after it.

        docs/aim.md's split, in the order it describes: the torso takes as much
        of the aim as it is allowed, and whatever is left over is what the
        character has to turn his feet for. `body_turn` returns that remainder
        rather than applying it, so the writing to the simulation's facing
        happens here, where the rest of the input does.

        Writing to `face_angle` from the render frame is safe for the same
        reason the old sight-turning was: nothing in the action machine sets it
        except the movement code, which reads it fresh each tick and will treat
        this as though he had turned himself.
        """
        player = self.player
        if player.aim is None:
            return
        state = player.state

        _, direction = self.follow_camera.aim_ray()
        player.aim.set_aim_direction(direction, state.face_angle[1])
        player.aim.set_tracking(self._tracking_strength())

        # Moving, he keeps the full twist: his legs are busy carrying him
        # somewhere and turning them would send him there sideways. Standing,
        # he is allowed to square up, and does.
        moving = (abs(state.forward_vel) > AIM_TURN_MAX_SPEED
                  or not state.action & C.ACT_FLAG_STATIONARY)
        turn = player.aim.body_turn(dt, moving)
        if turn:
            state.face_angle[1] = s16(state.face_angle[1] + turn)

        player.aim.update(dt)

    def _torso_text(self):
        """Where the aim layer has the upper body pointed."""
        aim = self.player.aim
        if aim is None or aim.joint is None:
            return (f"no pivot in this skeleton; he turns on his feet"
                    f"   (aim {aim.target_yaw:6.1f})" if aim else "none")
        return (f"yaw {aim.yaw:6.1f}  pitch {aim.pitch:6.1f}"
                f"   tracking {aim.tracking:4.2f}"
                f"   (aim {aim.target_yaw:6.1f} / {aim.target_pitch:5.1f})")

    def _hud_text(self):
        """The readout, as text.

        Built rather than drawn, because it has two places to go now: the
        OnscreenText F1 toggles, and the top of the console panel, which draws
        the same thing in the console's own monospace font.
        """
        m = self.state
        action = self.player.action_name
        floor_type = f"0x{m.floor.type:04X}" if m.floor else "none"
        slowest_frame = self._slowest_frame()
        slowest_ms = slowest_frame * 1000.0
        slowest_fps = 1.0 / slowest_frame if slowest_frame else 0.0

        return (
            f"playing  {self.player.name}  (F2 to swap)"
            f"{'   -- PAUSED, close the console to run' if self.console.visible else ''}\n"
            f"action   {action}  ({m.anim_name})\n"
            f"pos      {m.pos[0]:8.1f} {m.pos[1]:8.1f} {m.pos[2]:8.1f}\n"
            f"vel      fwd {m.forward_vel:6.2f}   y {m.vel[1]:7.2f}\n"
            f"yaw      {s16_to_degrees(m.face_angle[1]):7.1f} deg\n"
            f"torso    {self._torso_text()}\n"
            f"floor    {floor_type}  height {m.floor_height:8.1f}\n"
            f"enemies  {self._enemies_left()} left"
            f"   defeated {self.interactions.defeated}"
            f"   hits {self.interactions.hits_taken}\n"
            f"pipes    {self._pipe_text()}\n"
            f"squad    {self._squad_text()}\n"
            f"sprites  {self.impostor_drawn} drawn as impostors\n"
            f"fps      {self.clock.get_average_frame_rate():5.1f}"
            f"   frame {self.clock.get_dt() * 1000.0:5.1f} ms\n"
            f"worst 5s {slowest_ms:5.1f} ms ({slowest_fps:5.1f} fps)\n"
            f"\n{self._control_legend()}\n"
            f"mouse look  {'RMB' if self._mouse_captured else 'F'} aim  "
            f"R recentre  F2 swap  F3 collision  F1 hud  ` console"
            f"{'' if self._mouse_captured else '   (Esc again to quit)'}"
        )

    def _enemies_left(self):
        """Things that can actually be fought.

        Counted by type rather than by "everything that is not a tree": Mario
        stands in the same field now, and he is scenery with opinions, not an
        enemy.
        """
        return sum(1 for o in self.objects.objects
                   if o.active and isinstance(o, ENEMY_TYPES))

    def _pipe_text(self):
        """What each pipe is holding, and how long until the next one out.

        The seconds are the honest state of the countdown, so a pipe sitting
        at its cap shows the time it froze at rather than ticking towards a
        spawn that is not coming.
        """
        return "   ".join(
            f"{pipe.spawns.__name__.lower()} {pipe.population}/{pipe.limit}"
            f" {pipe.countdown / TICK_RATE:4.1f}s"
            for pipe in self.pipes
        )

    def _squad_text(self):
        """Who is following, who is on the way, and what the button is doing.

        The count inside the circle is the one number worth having while
        aiming: the circle is drawn on the ground some way off, and how many
        allies it has actually caught is not something you can see from behind
        the player.
        """
        line = (f"{len(self.squad.members)} following"
                f"   {self.squad.marching} on the way"
                f"   {self.squad.holding} holding a spot")
        radius = self._selection_radius()
        if radius is not None and self._aim_point is not None:
            caught = len(self.squad.in_circle(self._aim_point, radius))
            line += f"   whistling {radius:4.0f} units, {caught} in the circle"
        elif self._aim_hold is not None:
            line += "   aiming"
        return line

    def _control_legend(self):
        """The moves the character being played actually has.

        Listing Mario's while the Hero is out is worse than listing nothing:
        the Hero has no dive and no crouch, and a legend offering them reads as
        broken keys. He does skate, but on the jets rather than on C.
        """
        squad_keys = "X/LMB hold to whistle, tap to send"
        if self.player.name == "hero":
            return ("WASD move   Space jump   V jetpack (skates on the "
                    "ground, Space to take off)   Shift attack (again to "
                    f"chain, running to spin)   Ctrl sword   {squad_keys}")
        return (f"WASD move   Space jump   Shift dive   Ctrl crouch   "
                f"Z zombie   C skates{' ON' if self._skating else ''}   "
                f"{squad_keys}")


def main():
    # Before anything is built, so the console opens with the startup output
    # -- what spawned, what the audio device turned out to be -- already in it.
    console.capture_output()

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
