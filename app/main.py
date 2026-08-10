"""Run the Hero on the castle grounds in Panda3D, with Mario's level and physics.

The Hero is the character being played; Mario is an NPC wandering the field,
and can be switched to at any time to compare the two. They share the level,
the collision and the 30 Hz tick, and nothing above that: the Hero runs his own
action machine (sm64py/hero/) built around the twenty clips he actually has,
while Mario still runs the decomp port (sm64py/mario/) and its 209.

The game logic runs at a fixed 30 Hz, as the original does -- every physics
constant in the action code is per-frame at that rate, so rendering is
decoupled and the simulation is stepped in whole ticks.

Controls, as the Hero:
    W / A / S / D or arrows   analog stick (camera-relative)
    Space                     jump (held longer, jumps higher)
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
    Q / E or mouse drag       swing the camera
    R                         re-centre the camera
    `  (backquote / tilde)    open the debug console, pausing the game
    F1                        toggle the debug readout
    F2                        swap between the Hero and Mario
    F3                        toggle the collision overlay
    Escape                    close the console, or quit

The console shows the readout, everything the game has printed -- scroll back
through it with the wheel -- and a command line; typing the name of one of the
tunables registered in `_register_tunables` puts a slider for it on screen,
which stays up once the console is dismissed so it can be dragged while
playing. The game is paused for as long as the console is open. `help` inside
it lists the rest.
"""

import json
import math
import os
import sys

from direct.actor.Actor import Actor
from direct.showbase.ShowBase import ShowBase
from direct.gui.OnscreenText import OnscreenText
from panda3d.core import (
    AmbientLight,
    ClockObject,
    DirectionalLight,
    Filename,
    Fog,
    TextNode,
    Vec4,
    WindowProperties,
    loadPrcFileData,
)

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from sm64py import audio, console, objects, surfaces  # noqa: E402
from sm64py.camera import FollowCamera  # noqa: E402
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
LEVEL_OBJECTS = os.path.join(CASTLE_GROUNDS, "collision_objects.json")

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

SKY_COLOUR = (0.32, 0.60, 0.86)
UNDERWATER_COLOUR = (0.06, 0.28, 0.36)


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
                 node, actor, yaw_offset):
        self.name = name
        self.state = state
        self.execute = execute              # one tick of the action machine
        self.anims = anims                  # module with resolve/start_frame
        self.action_names = action_names
        self.node = node
        self.actor = actor
        self.yaw_offset = yaw_offset
        # Which clip is playing, so a change can be told from a repeat.
        self.current_anim = None

    @property
    def action_name(self):
        return self.action_names.get(self.state.action, hex(self.state.action))

    def show(self):
        self.node.show()

    def hide(self):
        self.node.hide()


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
        self.object_renderer = ObjectRenderer(
            ACTORS, self.loader, self.render,
            # Mario is an NPC now, and his model is the one the game already
            # ships rather than a second copy under assets/actors/.
            model_paths={"mario": MARIO_MODEL},
        )
        drawn = self.object_renderer.build(self.objects)
        print(f"Objects: {len(self.objects.objects)} spawned, {drawn} drawn")

        self.collision_view = load_collision_geometry(
            os.path.join(CASTLE_GROUNDS, "collision.npz")
        )
        self.collision_view.reparent_to(self.render)
        self.collision_view.set_render_mode_wireframe()
        self.collision_view.hide()

        self.controller = Controller()
        self._build_players()
        self.follow_camera = FollowCamera(self.surfaces, self.state)

        self.camLens.set_fov(45)
        self.camLens.set_near_far(10, 30000)

        self._setup_lighting()
        self._setup_hud()
        # Before the input, which asks the console whether it has the keyboard.
        self._setup_console()
        self._setup_input()

        self._accumulator = 0.0
        self._show_debug = True
        self._mouse_anchor = None
        self._hud_timer = 0.0
        self._water_time = 0.0
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
            MARIO_MODEL, MODEL_YAW_OFFSET, "mario",
            build_hint="python3 tools/export_actor_gltf.py --actor mario "
                       "--anims all -o assets/mario/mario.glb")

        self.players = [
            Player("hero", hero, hero_actions.execute_action, hero_animations,
                   HC.ACTION_NAMES, hero_node, hero_actor, HERO_YAW_OFFSET),
            Player("mario", mario, execute_action, animations,
                   ACTION_NAMES, mario_node, mario_actor, MODEL_YAW_OFFSET),
        ]
        # The Hero is who the game is about now; Mario is switched to.
        self.player = self.players[0]
        self.players[1].hide()

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

        # Mario is only an NPC while somebody else is being played; two of him
        # standing in the same field reads as a bug rather than a cameo.
        self.mario_npc.active = self.player.name != "mario"

        self._reset_interpolation()
        # Swapping is allowed from inside the console, and the character that
        # just came on stage has not been stopped yet.
        if self.console.visible:
            self._freeze_animation()
        print(f"Playing as {self.player.name}")

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
            # The N64 Z trigger is on left control, so the Z key itself is
            # free -- and it is the obvious one to put the zombie on.
            ("z", "zombie"),
            ("q", "cam_left"), ("e", "cam_right"),
            ("r", "cam_center"),
        ]
        for key, name in bindings:
            self.keys.setdefault(name, False)
            self.accept(key, self._set_key, [name, True])
            self.accept(f"{key}-up", self._set_key, [name, False])

        # The skates are the one control that latches. Holding a key to stay
        # on ice would mean never letting go, since the whole level is the
        # rink.
        self._skating = False
        self.accept("c", self._toggle_skating)

        self.accept("escape", self._escape)
        self.accept("f1", self._toggle_debug)
        self.accept("f2", self._switch_player)
        self.accept("f3", self._toggle_collision)

        self._dragging = False
        for button in ("mouse1", "mouse3"):
            self.accept(button, self._set_dragging, [True])
            self.accept(f"{button}-up", self._set_dragging, [False])

        props = WindowProperties()
        props.set_cursor_hidden(False)
        self.win.request_properties(props)

    def _set_key(self, name, value):
        # Releases always land, so a key held as the console opened does not
        # stay stuck down; presses do not, so typing is only typing.
        if value and self.console.visible:
            return
        self.keys[name] = value

    def _set_dragging(self, value):
        self._dragging = value

    def _toggle_skating(self):
        if self.console.visible:
            return
        self._skating = not self._skating

    def _escape(self):
        """Back out of the console if it is open; otherwise quit."""
        if self.console.visible:
            self.console.hide()
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

        t.add("attack_lunge", HC, "ATTACK_LUNGE_SPEED", 0.0, 40.0,
              "the forward travel handed back to the sword swings")
        t.add("spin_kick_speed", HC, "SPIN_KICK_SPEED", 0.0, 60.0,
              "how fast the spin kick carries him")
        t.add("spin_kick_min", HC, "SPIN_KICK_MIN_SPEED", 0.0, 32.0,
              "speed needed to spin kick rather than swing")
        t.add("wade_scale", HC, "WADE_SPEED_SCALE", 0.05, 1.0,
              "what deep water leaves of his speed")

        t.add("cam_distance", self.follow_camera, "distance", 250.0, 5000.0,
              "how far the camera sits behind him")
        t.add("cam_height", self.follow_camera, "height", -500.0, 1500.0,
              "how far above him it looks from")
        return t

    def _console_toggled(self, visible):
        """The console has the keyboard while it is open, and the readout.

        Held keys are dropped rather than left set, since the key-up for a key
        pressed before the console opened arrives while it is open and would
        otherwise be the only thing that ever cleared it.
        """
        if visible:
            self.hud.hide()
            for name in self.keys:
                self.keys[name] = False
            self._freeze_animation()
        elif self._show_debug:
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
            self.controller.zombie = False
            self.controller.skating = self._skating
            return

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

        # Purely a costume: the action code never reads it, so Mario walks and
        # jumps exactly as he always did while it is held.
        self.controller.zombie = self.keys["zombie"]
        # The skates are not a costume -- this one drives an action.
        self.controller.skating = self._skating

    # -- loop ----------------------------------------------------------------

    def _update(self, task):
        dt = min(self.clock.get_dt(), 0.25)

        # The console pauses the game. Nothing accumulates while it is open
        # either, so coming back steps a single tick rather than replaying
        # however long was spent typing -- and since the task still runs every
        # frame, the clock's dt is one frame's worth, not the whole pause.
        if self.console.visible:
            self.console.update(dt)
            return task.cont

        self._update_camera_input(dt)

        # Step the simulation in whole 30 Hz ticks.
        self._accumulator += dt
        steps = 0
        while self._accumulator >= TICK_DT and steps < 8:
            state = self.state
            # Remember where he was so the render can interpolate out of it.
            self._prev_pos = list(state.gfx_pos)
            self._prev_angle = list(state.gfx_angle)

            self._poll_controller()
            state.camera_yaw = self.follow_camera.mario_yaw
            self.player.execute(state)
            self.objects.update(state)
            # After both have moved, so a stomp is judged on where they
            # actually ended up rather than where they started.
            self.interactions.resolve(state)
            # Drained inside the tick loop, not after it: a frame that runs
            # two ticks would otherwise drop the first tick's sounds.
            self.sounds.play_events(state)
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

        self.follow_camera.update(dt, target_pos=pos,
                                  recenter=self.keys["cam_center"])
        self.follow_camera.apply_to(self.camera)

        # Its own clock rather than the frame time, so a spell in the console
        # does not leave the sheet somewhere else when the game comes back.
        self._water_time += dt
        animate_water(self.water, self._water_time)
        self.object_renderer.sync(self.follow_camera.pos)
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

    def _update_camera_input(self, dt):
        speed = 150.0
        if self.keys["cam_left"]:
            self.follow_camera.rotate(-speed * dt)
        if self.keys["cam_right"]:
            self.follow_camera.rotate(speed * dt)

        # Dragging with a mouse button held swings the camera too -- unless
        # the pointer is on the console or one of its sliders, where a drag
        # means the slider and nothing else.
        if (self._dragging and not self.console.wants_mouse()
                and self.mouseWatcherNode.has_mouse()):
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

    def _hud_text(self):
        """The readout, as text.

        Built rather than drawn, because it has two places to go now: the
        OnscreenText F1 toggles, and the top of the console panel, which draws
        the same thing in the console's own monospace font.
        """
        m = self.state
        action = self.player.action_name
        floor_type = f"0x{m.floor.type:04X}" if m.floor else "none"

        return (
            f"playing  {self.player.name}  (F2 to swap)"
            f"{'   -- PAUSED, close the console to run' if self.console.visible else ''}\n"
            f"action   {action}  ({m.anim_name})\n"
            f"pos      {m.pos[0]:8.1f} {m.pos[1]:8.1f} {m.pos[2]:8.1f}\n"
            f"vel      fwd {m.forward_vel:6.2f}   y {m.vel[1]:7.2f}\n"
            f"yaw      {s16_to_degrees(m.face_angle[1]):7.1f} deg\n"
            f"floor    {floor_type}  height {m.floor_height:8.1f}\n"
            f"enemies  {self._enemies_left()} left"
            f"   defeated {self.interactions.defeated}"
            f"   hits {self.interactions.hits_taken}\n"
            f"fps      {self.clock.get_average_frame_rate():5.1f}\n"
            f"\n{self._control_legend()}\n"
            f"Q/E camera  R recentre  F2 swap  F3 collision  F1 hud  ` console"
        )

    def _enemies_left(self):
        """Things that can actually be fought.

        Counted by type rather than by "everything that is not a tree": Mario
        stands in the same field now, and he is scenery with opinions, not an
        enemy.
        """
        return sum(1 for o in self.objects.objects
                   if o.active and isinstance(o, (objects.Goomba,
                                                  objects.Scuttlebug)))

    def _control_legend(self):
        """The moves the character being played actually has.

        Listing Mario's while the Hero is out is worse than listing nothing:
        the Hero has no dive, no crouch and no skates, and a legend offering
        them reads as four broken keys.
        """
        if self.player.name == "hero":
            return ("WASD move   Space jump   Shift attack (again to chain, "
                    "running to spin)   Ctrl sword")
        return (f"WASD move   Space jump   Shift dive   Ctrl crouch   "
                f"Z zombie   C skates{' ON' if self._skating else ''}")


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
