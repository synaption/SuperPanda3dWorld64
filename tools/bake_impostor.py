"""Bake an animated actor into a sprite atlas, for drawing crowds of it.

The enemies are cheap to simulate and ruinous to draw: each one is a skinned
character on its own node, and the moment its billboard quads are taken over the
whole character drops onto the CPU. A few dozen on screen is the ceiling. But an
SM64 goomba is, in spirit, already a sprite -- a face and a body billboarded to
face the camera -- so nothing is really lost by making it one in fact.

This renders the model from every side and across its walk cycle into one
texture, laid out as a grid of angle (down the rows) by animation frame (across
the columns), and writes the grid plus the numbers a renderer needs to place a
quad from it: how much world the cell covers, and where in the cell the feet
sit. sm64py/impostor.py draws thousands of these as one instanced quad each,
billboarded and cell-picked on the GPU.

    python3 tools/bake_impostor.py assets/actors/goomba.glb
    python3 tools/bake_impostor.py assets/actors/scuttlebug.glb --frames 24

The background is the same magenta key the workbench measures against -- nothing
in the palette is this colour -- and it is turned into transparency here, so
multisampling is left off to keep the edges a clean cut rather than a fringe of
half-keyed pixels.
"""

import argparse
import json
import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

# Nothing in the project's palette is this, so "far from magenta" is a safe
# test for "part of the model" -- the same key the workbench uses.
KEY = np.array([255, 0, 255], dtype=np.int16)
KEY_TOLERANCE = 40

DEFAULT_OUT = os.path.join(ROOT, "assets", "impostors")


def _configure(cell):
    from panda3d.core import loadPrcFileData
    loadPrcFileData("", f"win-size {cell} {cell}")
    loadPrcFileData("", "window-type offscreen")
    loadPrcFileData("", "audio-library-name null")
    loadPrcFileData("", "sync-video 0")
    # A clean-cut silhouette keys far more reliably than an anti-aliased one,
    # whose edge pixels are half magenta and read as neither model nor key.
    loadPrcFileData("", "framebuffer-multisample 0")
    loadPrcFileData("", "multisamples 0")


def _light(render):
    from panda3d.core import AmbientLight, DirectionalLight, Vec4
    # The same two lights the game uses, so a baked goomba is lit the way a
    # drawn one would have been and does not read as belonging to another scene.
    ambient = AmbientLight("ambient")
    ambient.set_color(Vec4(0.55, 0.55, 0.60, 1.0))
    render.set_light(render.attach_new_node(ambient))
    sun = DirectionalLight("sun")
    sun.set_color(Vec4(0.75, 0.72, 0.65, 1.0))
    sun_np = render.attach_new_node(sun)
    sun_np.set_hpr(-40, -60, 0)
    render.set_light(sun_np)


def _screenshot_rgb(base, cell):
    """The framebuffer as an (H, W, 3) uint8 array, top row first."""
    base.graphics_engine.render_frame()
    # Two render_frames: the first applies this cell's pose and heading, the
    # second is what is actually read, so the capture is never a frame behind.
    base.graphics_engine.render_frame()
    tex = base.win.get_screenshot()
    ram = tex.get_ram_image_as("RGBA")
    arr = np.frombuffer(bytes(ram), dtype=np.uint8).reshape(
        tex.get_y_size(), tex.get_x_size(), 4)
    # Panda hands back the image bottom row first; flip to the natural order.
    arr = arr[::-1]
    # Resize is never needed -- the window is the cell size -- so take RGB.
    return arr[:, :, :3].copy()


def _keyed_cell(rgb):
    """Turn the magenta background of one render into transparency."""
    h, w, _ = rgb.shape
    out = np.zeros((h, w, 4), dtype=np.uint8)
    out[:, :, :3] = rgb
    dist = np.abs(rgb.astype(np.int16) - KEY).sum(axis=2)
    out[:, :, 3] = np.where(dist <= KEY_TOLERANCE, 0, 255).astype(np.uint8)
    return out


def _measure(actor, base, clip, sample=6):
    """Film size, aim height and foot line, from bounds over the walk cycle.

    Sampled across several frames because a limb thrown out at one point of the
    cycle would clip a sprite framed only on the rest pose. The horizontal size
    is a radius rather than a width, so no heading rotates the model out of its
    own cell.
    """
    ctrl = actor.get_anim_control(clip)
    frames = ctrl.get_num_frames()
    min_z, max_z, radius = math.inf, -math.inf, 0.0
    for i in range(sample):
        actor.pose(clip, int(i * frames / sample))
        base.graphics_engine.render_frame()
        lo, hi = actor.get_tight_bounds()
        min_z, max_z = min(min_z, lo.z), max(max_z, hi.z)
        radius = max(radius,
                     math.hypot(max(abs(lo.x), abs(hi.x)),
                                max(abs(lo.y), abs(hi.y))))
    return min_z, max_z, radius


def bake(glb, out_dir, angles, frames, cell, elevation, margin):
    _configure(cell)
    from direct.showbase.ShowBase import ShowBase
    from panda3d.core import (Filename, OrthographicLens, Texture, Vec3)
    from direct.actor.Actor import Actor

    base = ShowBase()
    base.set_background_color(1.0, 0.0, 1.0, 1.0)
    _light(base.render)

    name = os.path.splitext(os.path.basename(glb))[0]
    actor = Actor(Filename.from_os_specific(os.path.abspath(glb)))
    actor.reparent_to(base.render)
    # The model's own billboard quads are single-sided; drawn flat here they
    # would drop out at the angles that see their back, so show both faces.
    actor.set_two_sided(True)
    clip = actor.get_anim_names()[0]

    min_z, max_z, radius = _measure(actor, base, clip)
    film = max(2.0 * radius, max_z - min_z) * (1.0 + margin)
    aim_z = 0.5 * (min_z + max_z)
    # Fraction of the cell, from the bottom, at which the feet (z = 0) sit --
    # what lets the renderer stand the sprite on the ground instead of centring
    # it in the air.
    foot_v = (0.5 * film - aim_z) / film

    lens = OrthographicLens()
    lens.set_film_size(film, film)
    lens.set_near_far(-10000.0, 10000.0)
    base.cam.node().set_lens(lens)
    e = math.radians(elevation)
    # Behind the model (-Y) and above it, looking down by `elevation`. Distance
    # is arbitrary under an orthographic lens; only the direction sets the view.
    base.camera.set_pos(0.0, -2000.0 * math.cos(e), aim_z + 2000.0 * math.sin(e))
    base.camera.look_at(0.0, 0.0, aim_z)

    cols, rows = frames, angles
    atlas = np.zeros((rows * cell, cols * cell, 4), dtype=np.uint8)
    clip_frames = actor.get_anim_control(clip).get_num_frames()
    angle_list = [round(a * 360.0 / angles, 3) for a in range(angles)]

    for r, heading in enumerate(angle_list):
        actor.set_h(heading)
        for c in range(frames):
            actor.pose(clip, int(c * clip_frames / frames))
            cellimg = _keyed_cell(_screenshot_rgb(base, cell))
            y, x = r * cell, c * cell
            atlas[y:y + cell, x:x + cell] = cellimg

    os.makedirs(out_dir, exist_ok=True)
    png = os.path.join(out_dir, name + ".png")
    meta = os.path.join(out_dir, name + ".json")

    tex = Texture(name)
    tex.setup_2d_texture(atlas.shape[1], atlas.shape[0],
                         Texture.T_unsigned_byte, Texture.F_rgba)
    # Panda stores textures bottom row first; hand it the flipped array so the
    # file it writes reads the right way up.
    tex.set_ram_image_as(atlas[::-1].tobytes(), "RGBA")
    tex.write(Filename.from_os_specific(png))

    with open(meta, "w") as fh:
        json.dump({
            "model": name,
            "clip": clip,
            "cell_px": cell,
            "angles": angles,
            "frames": frames,
            "cols": cols,
            "rows": rows,
            "angle_list": angle_list,
            "elevation": elevation,
            "film": round(film, 3),
            "foot_v": round(foot_v, 5),
            "atlas": name + ".png",
        }, fh, indent=2)
        fh.write("\n")

    covered = int((atlas[:, :, 3] > 0).sum())
    print(f"{name}: {cols}x{rows} cells at {cell}px "
          f"-> {atlas.shape[1]}x{atlas.shape[0]}  film {film:.0f}u  "
          f"foot_v {foot_v:.3f}  ({covered} lit texels)")
    print(f"  {png}")
    print(f"  {meta}")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("glb", help="the actor .glb to bake")
    ap.add_argument("--out", default=None,
                    help="output directory (default assets/impostors/)")
    ap.add_argument("--angles", type=int, default=16,
                    help="views around the vertical (default 16)")
    ap.add_argument("--frames", type=int, default=16,
                    help="animation frames sampled from the clip (default 16)")
    ap.add_argument("--cell", type=int, default=128,
                    help="pixels per cell, square (default 128)")
    ap.add_argument("--elevation", type=float, default=15.0,
                    help="degrees the bake camera looks down (default 15)")
    ap.add_argument("--margin", type=float, default=0.12,
                    help="fraction of padding around the silhouette")
    args = ap.parse_args()
    out = args.out or os.path.join(DEFAULT_OUT)
    bake(args.glb, out, args.angles, args.frames, args.cell,
         args.elevation, args.margin)


if __name__ == "__main__":
    main()
