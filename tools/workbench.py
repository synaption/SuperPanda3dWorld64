"""A minimal scene for looking at, and measuring, a single asset.

Every wrong conclusion drawn about this project's models came from measuring
something that was not isolated -- counting pixels of a scuttlebug to check its
billboards, when leg geometry dominated the count and swung with the viewing
angle for unrelated reasons. So the point of this workbench is not that it
draws things, it is that it draws *one* thing against a known background, with
any part of it hideable, so a number taken off the screen means what it says.

Two ways in:

    # look at it
    python3 tools/workbench.py assets/actors/scuttlebug.glb

    # measure it, for CI or for an agent with no screen
    python3 tools/workbench.py assets/actors/scuttlebug.glb --headless --json

Useful flags:

    --isolate PATTERN   draw only parts whose node or joint name matches
    --hide PATTERN      draw everything except those
    --anim NAME         play a clip (default: the first one)
    --frame N           hold a single frame instead of playing
    --orbit N           measure the silhouette from N angles around it
    --compare PATH      report size against another asset (usually mario)
    --shots DIR         write a .png per orbit angle
    --expect            run the checks and exit non-zero if any fail

Billboards -- the actor parts SM64 turns to face the camera -- are driven
through the same module the game uses, and their settings can be adjusted here
and written back to a file the game reads:

    # what the geometry says: parents, extents, the rotations to cancel
    python3 tools/workbench.py assets/actors/goomba.glb --probe

    # look at them, and tune them live with , . - =
    python3 tools/workbench.py assets/actors/goomba.glb --billboard

    # let the orbit measurement pick the settings
    python3 tools/workbench.py assets/actors/goomba.glb --tune --save-tuning

    # try one setting without committing to it
    python3 tools/workbench.py assets/actors/goomba.glb --billboard \
        --set roll=0 --orbit 8

Interactive keys are printed on start.
"""

import argparse
import json
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))

from sm64py import billboard as billboard_tuning  # noqa: E402

# The background the silhouette is measured against. Nothing in the project's
# palette is this colour, so "not background" is a safe test for "asset".
KEY_COLOUR = (1.0, 0.0, 1.0)
KEY_TOLERANCE = 0.15

DEFAULT_SIZE = (640, 480)


def _configure(headless, size):
    from panda3d.core import loadPrcFileData
    loadPrcFileData("", f"win-size {size[0]} {size[1]}")
    loadPrcFileData("", "audio-library-name null")
    loadPrcFileData("", "sync-video 0")
    loadPrcFileData("", "window-title asset workbench")
    if headless:
        loadPrcFileData("", "window-type offscreen")


class Workbench:
    """One asset, one clean scene, and the measurements you can take off it."""

    def __init__(self, path, headless=True, size=DEFAULT_SIZE,
                 tuning_path=None):
        _configure(headless, size)
        from direct.showbase.ShowBase import ShowBase
        from panda3d.core import AmbientLight, Vec4

        self.base = ShowBase()
        self.headless = headless
        self.base.disable_mouse()
        self.base.set_background_color(*KEY_COLOUR, 1)

        # Flat white light: shading that varies with angle would show up in a
        # silhouette count as if the geometry had changed.
        ambient = AmbientLight("flat")
        ambient.set_color(Vec4(1, 1, 1, 1))
        self.base.render.set_light(self.base.render.attach_new_node(ambient))

        self.path = path
        # Settings are keyed by asset name, so an override made here lands on
        # the same actor the game builds from this file.
        self.actor_name = os.path.splitext(os.path.basename(path))[0]
        self.tuning_path = tuning_path
        self.tuning = billboard_tuning.Tuning.load(tuning_path)
        self.tuning_field = 0
        self.rigs = []
        self.clip_index = 0
        self.posed = None

        self.actor, self.node = self._load(path)
        self.clips = self.actor.get_anim_names() if self.actor else []
        self.hidden = []
        self.orbit_angle = 0.0
        self.elevation = 0.25
        # Degrees per frame. The first version turned 36 degrees a second,
        # which is too fast to actually look at anything.
        self.spin_rate = 0.12
        self.blend_frames = 20.0
        self._frame_camera()

    # -- loading ------------------------------------------------------------

    def _load(self, path):
        from direct.actor.Actor import Actor
        from panda3d.core import Filename
        from sm64py.level import use_linear_textures, use_mipmaps

        full = Filename.from_os_specific(os.path.abspath(path))
        try:
            actor = Actor(full)
        except Exception:
            node = self.base.loader.load_model(full)
            node.reparent_to(self.base.render)
            use_linear_textures(node)
            return None, node

        use_linear_textures(actor)
        use_mipmaps(actor)
        actor.reparent_to(self.base.render)
        return actor, actor

    # -- what is in it ------------------------------------------------------

    def describe(self):
        """Everything worth knowing before measuring anything."""
        lo, hi = self.node.get_tight_bounds()
        geoms = self.node.find_all_matches("**/+GeomNode")
        info = {
            "asset": self.path,
            "bounds": {
                "min": [round(v, 2) for v in lo],
                "max": [round(v, 2) for v in hi],
                "size": [round(hi[i] - lo[i], 2) for i in range(3)],
            },
            "geom_nodes": len(geoms),
            "geoms": sum(g.node().get_num_geoms() for g in geoms),
            "textures": len(self.node.find_all_textures()),
            "animations": list(self.clips),
        }
        if self.actor:
            joints = [j.get_name() for j in self.actor.get_joints()]
            info["joints"] = len(joints)
            info["joint_names"] = joints
        return info

    def parts(self):
        """Names that --isolate and --hide can match against."""
        names = [n.get_name() for n in self.node.find_all_matches("**/+GeomNode")]
        if self.actor:
            names += [j.get_name() for j in self.actor.get_joints()]
        return sorted(set(names))

    # -- isolating ----------------------------------------------------------

    def isolate(self, pattern, invert=False):
        """Draw only the parts matching `pattern` (or only those not matching).

        Joints are handled by scaling them away rather than by hiding a node:
        skinned geometry belongs to the mesh, not to the joint, so hiding the
        joint's node would do nothing. Collapsing the joint takes its vertices
        with it.
        """
        import re
        regex = re.compile(pattern)

        if self.actor and self.actor.get_joints():
            # A skinned actor keeps every part in a single GeomNode, so hiding
            # nodes by name would hide all of it or none of it. Collapsing the
            # joint is what removes its vertices.
            #
            # Ancestors of a match have to survive: collapsing a joint takes
            # its whole subtree with it, so flattening an unmatched parent
            # would flatten the matched child too and leave nothing drawn.
            keep = self._keep_set(regex, invert)
            for joint in self.actor.get_joints():
                name = joint.get_name()
                if name in keep:
                    continue
                controlled = self.actor.control_joint(None, "modelRoot", name)
                if controlled is not None:
                    controlled.set_scale(0.0001)
                    self.hidden.append(controlled)
            return

        for np_ in self.node.find_all_matches("**/+GeomNode"):
            match = bool(regex.search(np_.get_name()))
            (np_.hide if match == invert else np_.show)()

    def _keep_set(self, regex, invert):
        """Joint names to leave alone: the matches, plus their ancestors."""
        bundle = self.actor.get_part_bundle("modelRoot")
        parents = {}

        def walk(node, parent):
            name = node.get_name()
            parents[name] = parent
            for i in range(node.get_num_children()):
                walk(node.get_child(i), name)

        walk(bundle, None)

        keep = set()
        for name in parents:
            if bool(regex.search(name)) == invert:
                continue
            cursor = name
            while cursor is not None and cursor not in keep:
                keep.add(cursor)
                cursor = parents.get(cursor)
        return keep

    # -- camera -------------------------------------------------------------

    def _frame_camera(self):
        lo, hi = self.node.get_tight_bounds()
        self.centre = (lo + hi) * 0.5
        self.radius = max(max(hi[i] - lo[i] for i in range(3)), 1.0)
        self._place_camera()

    def _place_camera(self):
        distance = self.radius * 2.4
        angle = math.radians(self.orbit_angle)
        self.base.camera.set_pos(
            self.centre[0] + distance * math.sin(angle),
            self.centre[1] - distance * math.cos(angle),
            self.centre[2] + distance * self.elevation,
        )
        self.base.camera.look_at(self.centre)

    # -- measuring ----------------------------------------------------------

    def silhouette(self):
        """Fraction of the frame the asset covers, and its screen extent.

        Measured against the key colour with nothing else in the scene, which
        is the whole reason this is trustworthy where an in-game screenshot is
        not.
        """
        import numpy as np
        from panda3d.core import PNMImage, Texture

        for _ in range(2):
            self.base.graphics_engine.render_frame()
        image = PNMImage()
        self.base.win.get_screenshot(image)
        width, height = image.get_x_size(), image.get_y_size()

        # Through a Texture rather than PNMImage.get_xel per pixel: a sweep
        # over settings takes hundreds of these, and the per-pixel loop made
        # measuring slow enough that guessing looked cheaper than checking.
        texture = Texture()
        texture.load(image)
        pixels = np.frombuffer(texture.get_ram_image(), np.uint8)
        pixels = pixels.reshape(height, width, -1)[:, :, :3]

        # Texture memory is BGR, and the key colour is given as RGB.
        key = np.array([KEY_COLOUR[2], KEY_COLOUR[1], KEY_COLOUR[0]]) * 255.0
        asset = (np.abs(pixels.astype(np.int16) - key)
                 > KEY_TOLERANCE * 255.0).any(axis=2)

        rows = np.flatnonzero(asset.any(axis=1))
        columns = np.flatnonzero(asset.any(axis=0))
        return {
            "pixels": int(asset.sum()),
            "fraction": round(float(asset.mean()), 5),
            "screen_width": int(columns[-1] - columns[0] + 1) if columns.size else 0,
            "screen_height": int(rows[-1] - rows[0] + 1) if rows.size else 0,
        }

    def enable_billboards(self, prefix="billboard_"):
        """Drive matching joints through the same code the game renders with.

        Deliberately the shipping module rather than a copy of it, so what gets
        adjusted and measured here is what actually runs.
        """
        from sm64py import billboard

        if not self.actor:
            return 0
        self.rigs = billboard.claim(self.actor, prefix,
                                    actor_name=self.actor_name)
        return len(self.rigs)

    def _aim_billboards(self):
        if not getattr(self, "rigs", None):
            return
        target = self.base.camera.get_pos(self.base.render)
        for rig in self.rigs:
            rig.aim(self.base.render, target, self.tuning)

    def probe(self, prefix="billboard_"):
        """The measurements to reason from when the aiming is wrong.

        Reports each joint's parent, its vertex extent (a billboard quad is
        flat along exactly one axis, and that axis is its normal), and the
        parent rotation the aiming has to cancel.
        """
        from sm64py import billboard

        if not self.actor:
            return []
        return billboard.probe(self.actor, prefix)

    def billboard_quality(self, steps=8):
        """How well the billboards hold their width around an orbit.

        1.0 is a perfect billboard. Anything that stays flat-on to the camera
        keeps its width; anything that does not collapses toward a line twice
        per turn, which shows up as a small minimum against a large maximum.
        """
        angle, elevation = self.orbit_angle, self.elevation
        orbit = self.orbit(steps)
        self.orbit_angle, self.elevation = angle, elevation
        self._place_camera()
        return Workbench.check_billboard(orbit)

    def tune(self, steps=8, fields=("heading_offset", "pitch", "roll"),
             coarse=45.0, tie_fraction=0.1):
        """Search the tuning space for the setting that measures best.

        A sweep rather than an argument: every previous attempt at these
        constants was reasoned out and wrong, and the orbit measurement is the
        only thing that has reliably told them apart.
        """
        import itertools

        values = [v * coarse for v in range(int(round(360.0 / coarse)))]
        grid = [values if f in fields else [self.tuning.get(f, self.actor_name)]
                for f in ("heading_offset", "pitch", "roll")]

        original = {f: self.tuning.get(f, self.actor_name)
                    for f in ("heading_offset", "pitch", "roll")}
        results = []
        for heading, pitch, roll in itertools.product(*grid):
            self.tuning.set("heading_offset", heading, self.actor_name)
            self.tuning.set("pitch", pitch, self.actor_name)
            self.tuning.set("roll", roll, self.actor_name)
            check = self.billboard_quality(steps)
            results.append((check["ratio"], heading, pitch, roll, check))

        results.sort(key=lambda r: -r[0])

        # This sweep is for telling regimes apart, not for micro-optimising.
        # A working setting scores about fourteen times better than a broken
        # one, so anything within a tenth of the best is the same answer, and
        # among those the plainest wins. Letting a percent of pixel-quantisation
        # noise pick between them is how a meaningless 135-degree pitch ends up
        # written down as though it meant something.
        top = results[0][0]
        tied = [r for r in results if r[0] >= top * (1.0 - tie_fraction)]
        best = min(tied, key=lambda r: sum(abs(_signed(v)) for v in r[1:4]))

        for field, value in original.items():
            self.tuning.set(field, value, self.actor_name)
        return {
            "best": {"ratio": best[0], "heading_offset": _signed(best[1]),
                     "pitch": _signed(best[2]), "roll": _signed(best[3])},
            "top": [{"ratio": r[0], "heading_offset": _signed(r[1]),
                     "pitch": _signed(r[2]), "roll": _signed(r[3])}
                    for r in results[:8]],
            "tied_with_best": len(tied),
            "tried": len(results),
        }

    def apply_tuning(self, best):
        for field in ("heading_offset", "pitch", "roll"):
            self.tuning.set(field, best[field], self.actor_name)

    def orbit(self, steps=8, shots_dir=None):
        """Measure the silhouette all the way around."""
        from panda3d.core import Filename

        results = []
        for i in range(steps):
            self.orbit_angle = 360.0 * i / steps
            self._place_camera()
            self._aim_billboards()
            entry = self.silhouette()
            entry["angle"] = round(self.orbit_angle, 1)
            results.append(entry)
            if shots_dir:
                os.makedirs(shots_dir, exist_ok=True)
                path = os.path.join(shots_dir,
                                    f"{self.orbit_angle:05.1f}.png")
                self.base.win.save_screenshot(
                    Filename.from_os_specific(os.path.abspath(path)))
        self.orbit_angle = 0.0
        self._place_camera()
        return results

    def pose(self, clip=None, frame=None):
        if not self.actor or not self.clips:
            return None
        name = clip or self.clips[0]
        if name not in self.clips:
            return None
        if frame is None:
            self.actor.loop(name)
        else:
            self.actor.pose(name, frame)
        self.posed = name
        return name

    # -- checks -------------------------------------------------------------

    @staticmethod
    def check_billboard(orbit, tolerance=0.35):
        """Does this hold its apparent size all the way round?

        A billboard turns to face the camera, so its silhouette stays roughly
        constant. A flat quad that is *meant* to billboard but does not will
        collapse toward a line at two opposite angles, which shows up as a
        small minimum against a large maximum.
        """
        widths = [e["screen_width"] for e in orbit if e["screen_width"] > 0]
        if len(widths) < len(orbit):
            # An angle that drew nothing is the worst possible result, not a
            # missing measurement. Reporting it as a ratio keeps it comparable
            # with every other setting a sweep tries.
            return {"pass": False, "ratio": 0.0, "min_width": 0,
                    "max_width": max(widths) if widths else 0,
                    "tolerance": tolerance,
                    "reason": f"drew nothing at {len(orbit) - len(widths)} "
                              f"of {len(orbit)} angles"}
        ratio = min(widths) / float(max(widths))
        return {
            "pass": ratio >= tolerance,
            "min_width": min(widths),
            "max_width": max(widths),
            "ratio": round(ratio, 3),
            "tolerance": tolerance,
            "reason": ("holds its width around the orbit" if ratio >= tolerance
                       else "collapses at some angles, so it is not billboarding"),
        }

    @staticmethod
    def check_grounded(bounds, tolerance=2.0):
        """Does it stand on z=0 rather than straddling it?

        A rigged model loaded without a pose sits in its bind pose, which for
        SM64 actors is centred on the origin -- half of it below the floor.
        """
        base = bounds["min"][2]
        return {
            "pass": abs(base) <= tolerance,
            "base_z": base,
            "tolerance": tolerance,
            "reason": ("stands on the ground plane" if abs(base) <= tolerance
                       else "straddles or floats above the ground plane"),
        }

    # -- interactive --------------------------------------------------------

    def run_interactive(self):
        from direct.gui.OnscreenText import OnscreenText
        from panda3d.core import TextNode

        self.readout = OnscreenText(
            text="", pos=(-1.3, 0.92), scale=0.042, fg=(1, 1, 1, 1),
            shadow=(0, 0, 0, 0.8), align=TextNode.A_left, mayChange=True)
        self.spin = True
        self.blending = False
        self.blend_from = None

        # Blending has to be on before any cross-fade will show; with it off,
        # switching clips is an instant cut.
        if self.actor:
            self.actor.enable_blend()

        self.base.accept("escape", sys.exit)
        self.base.accept("arrow_left", self._nudge, [-15.0])
        self.base.accept("arrow_right", self._nudge, [15.0])
        self.base.accept("arrow_up", self._tilt, [0.1])
        self.base.accept("arrow_down", self._tilt, [-0.1])
        self.base.accept("space", self._toggle_spin)
        self.base.accept("[", self._spin_slower)
        self.base.accept("]", self._spin_faster)
        self.base.accept("n", self._next_clip)
        self.base.accept("p", self._prev_clip)
        self.base.accept("b", self._toggle_blend)
        self.base.accept("w", self._toggle_wireframe)
        self.base.accept("t", self._toggle_two_sided)
        self.base.accept("m", self._measure_now)

        # Billboard tuning, live. One field is selected at a time and stepped
        # with - and =, which keeps the scheme the same however many settings
        # there turn out to be.
        self.base.accept(",", self._prev_field)
        self.base.accept(".", self._next_field)
        self.base.accept("-", self._nudge_field, [-1])
        self.base.accept("=", self._nudge_field, [1])
        self.base.accept("g", self._toggle_override)
        self.base.accept("k", self._check_billboards)
        self.base.accept("y", self._auto_tune)
        self.base.accept("s", self._save_tuning)
        self.base.accept("l", self._reload_tuning)
        self.base.accept("r", self._reset_tuning)
        self.base.accept("d", self._dump_probe)

        # Number keys jump straight to a clip. With blending on, the jump is a
        # cross-fade instead of a cut, which is what makes a transition
        # visible at all.
        for digit in range(10):
            self.base.accept(str(digit), self._select_clip, [digit])

        print(__doc__.split("Interactive keys")[0])
        print("keys:")
        print("  arrows      orbit / tilt")
        print("  space       pause spin      [ ]  spin slower / faster")
        print("  0-9         jump to that clip")
        print("  n / p       next / previous clip")
        print("  b           cross-fade between clips (currently off)")
        print("  w           wireframe       t    two-sided")
        print("  m           print a measurement")
        print("  esc         quit")
        if self.rigs:
            print(f"\nbillboards ({len(self.rigs)} joints driven):")
            print("  , .         select the setting to change")
            print("  - =         change it")
            print("  g           make it an override for this actor only")
            print("  k           measure how well they billboard, right now")
            print("  y           search for the best setting and apply it")
            print("  s / l / r   save / reload / reset settings")
            print("  d           print what the geometry says about them")
        if self.clips:
            print(f"\n{len(self.clips)} clips:")
            for i, name in enumerate(self.clips[:10]):
                print(f"  {i}  {name}")
            if len(self.clips) > 10:
                print(f"  ... and {len(self.clips) - 10} more, use n / p")
        self.pose()
        self.base.task_mgr.add(self._spin_task, "spin")
        self.base.run()

    def _spin_slower(self):
        self.spin_rate = max(0.0, self.spin_rate - 0.05)

    def _spin_faster(self):
        self.spin_rate = min(2.0, self.spin_rate + 0.05)

    def _toggle_two_sided(self):
        self.two_sided = not getattr(self, "two_sided", False)
        self.node.set_two_sided(self.two_sided)

    def _toggle_blend(self):
        self.blending = not self.blending

    def _select_clip(self, index):
        if self.clips and index < len(self.clips):
            self._go_to_clip(index)

    def _prev_clip(self):
        if self.clips:
            self._go_to_clip((self.clip_index - 1) % len(self.clips))

    def _go_to_clip(self, index):
        """Switch clips, cross-fading if blending is on.

        A cross-fade needs both clips running at once with their control
        weights summing to one; the task below walks the weight across.
        """
        if not self.actor or index == self.clip_index:
            return
        previous = self.clips[self.clip_index]
        self.clip_index = index
        target = self.clips[index]

        if self.blending:
            self.blend_from = previous
            self.blend_t = 0.0
            self.actor.loop(target)
            self.actor.set_control_effect(target, 0.0)
            self.actor.set_control_effect(previous, 1.0)
        else:
            self.actor.stop()
            self.actor.loop(target)

    def _nudge(self, delta):
        self.orbit_angle = (self.orbit_angle + delta) % 360.0
        self._place_camera()

    def _tilt(self, delta):
        self.elevation = max(-0.9, min(1.6, self.elevation + delta))
        self._place_camera()

    def _toggle_spin(self):
        self.spin = not self.spin

    def _toggle_wireframe(self):
        self.base.toggle_wireframe()

    def _next_clip(self):
        if not self.clips:
            return
        self.clip_index = (self.clip_index + 1) % len(self.clips)
        self.pose(self.clips[self.clip_index])

    def _measure_now(self):
        print(json.dumps(self.silhouette(), indent=2))

    # -- billboard tuning ---------------------------------------------------

    @property
    def _field(self):
        return billboard_tuning.FIELDS[self.tuning_field][0]

    # Whether changes land globally or only on this actor. Off by default:
    # most of these settings turned out to be shared, and a global fix is
    # worth more than a pile of per-actor exceptions.
    def _toggle_override(self):
        self.override = not getattr(self, "override", False)
        if not self.override:
            self.tuning.clear(self.actor_name)
        print(f"changes now apply "
              f"{'to ' + self.actor_name + ' only' if self.override else 'globally'}")

    def _scope(self):
        return self.actor_name if getattr(self, "override", False) else None

    def _prev_field(self):
        self.tuning_field = ((self.tuning_field - 1)
                             % len(billboard_tuning.FIELDS))
        print(f"{self._field}: {billboard_tuning.HELP[self._field]}")

    def _next_field(self):
        self.tuning_field = ((self.tuning_field + 1)
                             % len(billboard_tuning.FIELDS))
        print(f"{self._field}: {billboard_tuning.HELP[self._field]}")

    def _nudge_field(self, direction):
        value = self.tuning.nudge(self._field, direction, self._scope())
        print(f"{self._field} = {value}")

    def _check_billboards(self):
        if not self.rigs:
            print("no billboard joints on this asset")
            return
        print(json.dumps(self.billboard_quality(), indent=2))

    def _auto_tune(self):
        if not self.rigs:
            print("no billboard joints on this asset")
            return
        print("searching...")
        result = self.tune()
        self.apply_tuning(result["best"])
        print(json.dumps(result["best"], indent=2))
        print("applied; press s to save")

    def _save_tuning(self):
        print(f"saved {self.tuning.save(self.tuning_path)}")

    def _reload_tuning(self):
        self.tuning = billboard_tuning.Tuning.load(self.tuning_path)
        print("reloaded")

    def _reset_tuning(self):
        self.tuning.clear(self._scope())
        print("reset to defaults")

    def _dump_probe(self):
        for row in self.probe():
            if row["billboard"]:
                print(json.dumps(row))

    def _advance_blend(self):
        """Walk the cross-fade weight from the old clip to the new one."""
        if self.blend_from is None:
            return 0.0
        self.blend_t = min(1.0, self.blend_t + 1.0 / self.blend_frames)
        target = self.clips[self.clip_index]
        self.actor.set_control_effect(target, self.blend_t)
        self.actor.set_control_effect(self.blend_from, 1.0 - self.blend_t)
        if self.blend_t >= 1.0:
            self.actor.stop(self.blend_from)
            self.blend_from = None
        return self.blend_t

    def _spin_task(self, task):
        if self.spin:
            self._nudge(self.spin_rate)
        # Every frame, not just on orbit changes: an animated parent moves the
        # quad even when the camera is still.
        self._aim_billboards()
        blend = self._advance_blend()

        clip = self.clips[self.clip_index] if self.clips else "-"
        lo, hi = self.node.get_tight_bounds()
        lines = [
            os.path.basename(self.path),
            f"clip     [{self.clip_index}] {clip}",
            f"size     {hi[0]-lo[0]:.1f} x {hi[1]-lo[1]:.1f} x {hi[2]-lo[2]:.1f}",
            f"base z   {lo[2]:.1f}",
            f"angle    {self.orbit_angle:.0f} deg   spin {self.spin_rate:.2f}",
            f"blend    {'on' if self.blending else 'off'}"
            + (f"   {self.blend_from} -> {clip}  {blend:.0%}"
               if self.blend_from else ""),
            f"2-sided  {'on' if getattr(self, 'two_sided', False) else 'off'}",
        ]
        if self.rigs:
            scope = self._scope()
            lines.append("")
            lines.append(f"billboards  {len(self.rigs)} joints"
                         f"   ({'this actor' if scope else 'global'})")
            for index, line in enumerate(self.tuning.describe(scope)):
                lines.append(("> " if index == self.tuning_field else "  ")
                             + line)
        self.readout.setText("\n".join(lines))
        return task.cont


def main(argv):
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("asset", help="path to a .glb or other model")
    parser.add_argument("--headless", action="store_true",
                        help="measure instead of opening a window")
    parser.add_argument("--json", action="store_true",
                        help="emit the report as JSON")
    parser.add_argument("--isolate", help="draw only parts matching this regex")
    parser.add_argument("--hide", help="draw everything except these")
    parser.add_argument("--anim", help="clip to play")
    parser.add_argument("--frame", type=int, help="hold this frame")
    parser.add_argument("--orbit", type=int, default=0, metavar="N",
                        help="measure the silhouette from N angles")
    parser.add_argument("--compare", help="another asset to size against")
    parser.add_argument("--shots", help="directory to write orbit frames to")
    parser.add_argument("--size", default="640x480")
    parser.add_argument("--list-parts", action="store_true")
    parser.add_argument("--billboard", nargs="?", const="billboard_",
                        metavar="PREFIX",
                        help="drive matching joints through the renderer's own "
                             "billboard aiming, to look at or test it")
    parser.add_argument("--billboard-axis", action="store_true",
                        help="turn the whole asset about its vertical axis to "
                             "face the camera, the way a tree does -- that "
                             "billboard comes from the behaviour, not from "
                             "anything in the model")
    parser.add_argument("--tuning", metavar="PATH",
                        help="billboard settings file to read and write "
                             f"(default {billboard_tuning.DEFAULT_PATH})")
    parser.add_argument("--set", action="append", default=[],
                        metavar="FIELD=VALUE",
                        help="override one billboard setting for this run")
    parser.add_argument("--probe", action="store_true",
                        help="report what the geometry says about the "
                             "billboard joints, and stop")
    parser.add_argument("--tune", action="store_true",
                        help="search for the billboard setting that measures "
                             "best around an orbit")
    parser.add_argument("--save-tuning", action="store_true",
                        help="write what --tune found back to the settings file")
    parser.add_argument("--expect", action="store_true",
                        help="run the checks and exit non-zero on failure")
    args = parser.parse_args(argv[1:])

    if not os.path.exists(args.asset):
        print(f"No such asset: {args.asset}")
        return 2

    width, _, height = args.size.partition("x")
    # Anything that asks for a number is asking to measure, not to look; an
    # --orbit that quietly opened a window and reported nothing was a trap.
    headless = bool(args.headless or args.expect or args.probe or args.tune
                    or args.orbit or args.json or args.list_parts)
    bench = Workbench(args.asset, headless=headless,
                      size=(int(width), int(height or width)),
                      tuning_path=args.tuning)

    for assignment in args.set:
        field, _, value = assignment.partition("=")
        bench.tuning.set(field.strip(), _parse_setting(value))

    if args.list_parts:
        for name in bench.parts():
            print(f"  {name}")
        return 0

    bench.pose(args.anim, args.frame)

    if args.probe:
        # Posed first, deliberately. In the rest pose every joint is identity,
        # so a probe taken there reports no parent rotation to cancel and is
        # useless -- the rotation only exists once a clip is applied, which is
        # why it has to be cancelled per frame rather than baked in.
        _print_probe(bench.probe(), pose=bench.posed, frame=args.frame)
        return 0

    if args.billboard or args.tune:
        count = bench.enable_billboards(args.billboard or "billboard_")
        if not args.json:
            print(f"driving {count} billboard joints\n")
        # Billboard quads are single-sided, so once one is turned to face the
        # camera it is invisible from half of every orbit. The renderer does
        # the same thing when it claims any.
        if count:
            bench.node.set_two_sided(True)
            bench.two_sided = True
    if args.billboard_axis:
        # Same call ObjectRenderer makes for an object whose behaviour is
        # BILLBOARD()/CYLBOARD(). Nothing in the asset says to do this.
        bench.node.set_billboard_axis()
    if args.isolate:
        bench.isolate(args.isolate)
    elif args.hide:
        bench.isolate(args.hide, invert=True)

    if args.tune:
        result = bench.tune()
        bench.apply_tuning(result["best"])
        if args.json:
            print(json.dumps(result, indent=2))
        else:
            print(f"tried {result['tried']} settings, best first:\n")
            for entry in result["top"]:
                print(f"  ratio {entry['ratio']:.3f}   "
                      f"heading_offset {entry['heading_offset']:6.1f}  "
                      f"pitch {entry['pitch']:6.1f}  roll {entry['roll']:6.1f}")
            best = result["best"]
            print(f"\n{result['tied_with_best']} settings are within a tenth "
                  f"of the best, so they are the same answer;"
                  f"\ntaking the plainest of them:")
            print(f"  ratio {best['ratio']:.3f}   "
                  f"heading_offset {best['heading_offset']:6.1f}  "
                  f"pitch {best['pitch']:6.1f}  roll {best['roll']:6.1f}")
        if args.save_tuning:
            print(f"\nsaved {bench.tuning.save(args.tuning)}")
        else:
            print("\nnot saved; pass --save-tuning to keep it")
        return 0

    if not headless:
        bench.run_interactive()
        return 0

    report = bench.describe()
    if args.isolate or args.hide:
        # Bounds change once parts are hidden, so re-read them.
        report["filtered"] = True
        report["bounds"] = bench.describe()["bounds"]

    if args.orbit:
        report["orbit"] = bench.orbit(args.orbit, args.shots)
        report["checks"] = {
            "billboard": Workbench.check_billboard(report["orbit"]),
            "grounded": Workbench.check_grounded(report["bounds"]),
        }
    else:
        report["checks"] = {
            "grounded": Workbench.check_grounded(report["bounds"]),
        }

    if args.compare and os.path.exists(args.compare):
        other = bench.node.attach_new_node("compare")
        other.remove_node()
        report["compare"] = _compare(bench, args.compare)

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        _print_report(report)

    if args.expect:
        failed = [k for k, v in report.get("checks", {}).items()
                  if not v.get("pass")]
        for name in failed:
            print(f"FAIL {name}: {report['checks'][name]['reason']}")
        return 1 if failed else 0
    return 0


def _signed(angle):
    """An angle in [-180, 180), so 315 reads as -45 rather than as far away."""
    return round((angle + 180.0) % 360.0 - 180.0, 4)


def _parse_setting(text):
    text = text.strip()
    if text.lower() in ("true", "yes", "on"):
        return True
    if text.lower() in ("false", "no", "off"):
        return False
    try:
        return float(text)
    except ValueError:
        return text


def _print_probe(rows, pose=None, frame=None):
    """The evidence for how the billboards need to be driven.

    A billboard quad is flat along exactly one axis, and that axis is its
    normal -- which is worth reading off the extent rather than the stored
    normals, since several of these assets export a uniform placeholder there.
    The parent rotation is what the aiming has to cancel.
    """
    if not rows:
        print("no joints; this asset is not skinned")
        return
    print(f"pose: {pose or 'rest'}"
          f"{'' if frame is None else f' frame {frame}'}\n")
    print(f"{'joint':<15} {'parent':<12} {'verts':>5}  "
          f"{'extent':<22} {'flat':<4} {'net hpr':<22} {'parent hpr'}")
    for row in rows:
        mark = "*" if row["billboard"] else " "
        extent = ",".join(f"{v:g}" for v in row["extent"])
        net = ",".join(f"{v:g}" for v in row["net_hpr"])
        parent = (",".join(f"{v:g}" for v in row["parent_hpr"])
                  if row["parent_hpr"] else "-")
        print(f"{mark}{row['joint']:<14} {str(row['parent']):<12} "
              f"{row['vertices']:>5}  {extent:<22} "
              f"{str(row['flat_axis'] or '-'):<4} {net:<22} {parent}")
    print("\n* = billboard joint. 'flat' is the axis the quad has no thickness "
          "along,\n  which is the direction it faces in its own frame.")


def _compare(bench, other_path):
    """Size this asset against another, posed the same way."""
    from direct.actor.Actor import Actor
    from panda3d.core import Filename

    other = Actor(Filename.from_os_specific(os.path.abspath(other_path)))
    other.reparent_to(bench.base.render)
    other.set_x(bench.radius * 2.0)
    clips = other.get_anim_names()
    if clips:
        other.pose(clips[0], 0)
    lo, hi = other.get_tight_bounds()
    mine_lo, mine_hi = bench.node.get_tight_bounds()
    other.detach_node()

    theirs = hi[2] - lo[2]
    mine = mine_hi[2] - mine_lo[2]
    return {
        "against": other_path,
        "their_height": round(theirs, 2),
        "my_height": round(mine, 2),
        "ratio": round(mine / theirs, 3) if theirs else None,
    }


def _print_report(report):
    size = report["bounds"]["size"]
    print(f"asset      {report['asset']}")
    print(f"size       {size[0]} x {size[1]} x {size[2]}")
    print(f"base z     {report['bounds']['min'][2]}")
    print(f"geometry   {report['geoms']} geoms in {report['geom_nodes']} nodes, "
          f"{report['textures']} textures")
    if "joints" in report:
        print(f"joints     {report['joints']}")
    if report["animations"]:
        print(f"animations {len(report['animations'])}: "
              f"{', '.join(report['animations'][:4])}"
              f"{' ...' if len(report['animations']) > 4 else ''}")

    if "orbit" in report:
        print("\norbit:")
        for entry in report["orbit"]:
            print(f"  {entry['angle']:6.1f} deg  {entry['pixels']:7d} px  "
                  f"{entry['screen_width']:4d} x {entry['screen_height']:4d}")

    if "compare" in report:
        c = report["compare"]
        print(f"\nvs {os.path.basename(c['against'])}: "
              f"{c['my_height']} against {c['their_height']} "
              f"= {c['ratio']}x")

    print("\nchecks:")
    for name, result in report.get("checks", {}).items():
        mark = "ok  " if result["pass"] else "FAIL"
        print(f"  [{mark}] {name}: {result['reason']}")


if __name__ == "__main__":
    sys.exit(main(sys.argv))
