#!/usr/bin/env python3
"""Generate a stylized modular-coil stellarator as a binary glTF asset."""

from __future__ import annotations

import argparse
import math
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from glb import ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER, FLOAT, UNSIGNED_INT, GLB


ROOT = Path(__file__).resolve().parents[1]
PROJECT_BLENDER = ROOT / "blender-5.2.0-linux-x64" / "blender"
BLEND_IMPORTER = Path(__file__).resolve().parent / "build_blender_sources.py"


# The machine's overall width, across the modular coils, in game metres at
# `--scale 1.0`.
#
# This is the one number that says how big a stellarator is. The geometry below
# is written in a unit of its own -- a major radius of 3, rings at 4.18 -- which
# is a convenient way to describe the *shape* and says nothing about size; every
# vertex is multiplied on the way out so that the machine spans this instead.
#
# It lives here rather than in the game because the game measures the file: see
# `machine()` in `src/stellarator.rs`, which reads the baked scale back off a
# plasma vertex and follows it. Nothing in the port has to be edited to match a
# change here -- the footprint ring, the overlap test, the lift that stands it on
# the ground and the plasma the wisps ride all come off the measurement.
#
# For reference, at the port's scale of one unit to the metre: Luna is 1.93 m
# tall, an ant's body spans 4.10 m and stands 1.72 m, and a warp pipe is 3 m.
MACHINE_WIDTH = 32.0

# What the shape below spans in its own unit, across its widest part -- which is
# the coils. A coil sweeps out to `major + 1.13 + 0.2` and carries a tube of
# 0.105 round that, reaching 4.435.
#
# Not a free parameter: it is read off the numbers in `generate` and has to
# follow them if those change. `the_machine_is_measured_off_the_file_it_is_drawn
# _from` in `src/stellarator.rs` is what notices if it stops agreeing.
SHAPE_WIDTH = 2 * (3.0 + 1.13 + 0.2 + 0.105)


def surface_mesh(position, nu: int, nv: int):
    """Sample a closed parametric surface and return positions/normals/triangles."""
    p = np.empty((nu, nv, 3), dtype=np.float64)
    for i in range(nu):
        for j in range(nv):
            p[i, j] = position(2 * math.pi * i / nu, 2 * math.pi * j / nv)

    du = np.roll(p, -1, axis=0) - np.roll(p, 1, axis=0)
    dv = np.roll(p, -1, axis=1) - np.roll(p, 1, axis=1)
    n = np.cross(du, dv)
    n /= np.maximum(np.linalg.norm(n, axis=2, keepdims=True), 1e-12)

    faces = []
    for i in range(nu):
        for j in range(nv):
            a = i * nv + j
            b = ((i + 1) % nu) * nv + j
            c = ((i + 1) % nu) * nv + (j + 1) % nv
            d = i * nv + (j + 1) % nv
            faces.extend(((a, b, c), (a, c, d)))
    return p.reshape(-1, 3), n.reshape(-1, 3), np.asarray(faces, dtype=np.uint32)


def tube_mesh(points: np.ndarray, radius: float, sides: int = 10):
    """Sweep a circular tube along a closed polyline using stable local frames."""
    count = len(points)
    tangent = np.roll(points, -1, axis=0) - np.roll(points, 1, axis=0)
    tangent /= np.linalg.norm(tangent, axis=1, keepdims=True)
    radial = points.copy()
    radial[:, 2] = 0
    radial /= np.maximum(np.linalg.norm(radial, axis=1, keepdims=True), 1e-9)
    binormal = np.cross(tangent, radial)
    binormal /= np.maximum(np.linalg.norm(binormal, axis=1, keepdims=True), 1e-9)
    normal = np.cross(binormal, tangent)

    vertices, normals = [], []
    for i in range(count):
        for j in range(sides):
            a = 2 * math.pi * j / sides
            direction = math.cos(a) * normal[i] + math.sin(a) * binormal[i]
            vertices.append(points[i] + radius * direction)
            normals.append(direction)
    faces = []
    for i in range(count):
        ni = (i + 1) % count
        for j in range(sides):
            nj = (j + 1) % sides
            a, b = i * sides + j, ni * sides + j
            c, d = ni * sides + nj, i * sides + nj
            faces.extend(((a, b, c), (a, c, d)))
    return np.asarray(vertices), np.asarray(normals), np.asarray(faces, dtype=np.uint32)


# What a polished tube catches, as a ramp of flat bands.
#
# The game has no metallic anything -- `src/n64.rs` reads five fields off a
# glTF material and `metallicFactor` is not one of them, because the console
# had no specular lobe to put a reflection in. So the reflection is baked here,
# into COLOR_0, and the shader multiplies it into the tint the way the combiner
# multiplied the vertex colour by the texture.
#
# **Bands rather than a curve, and that is the whole trick.** A smooth ramp was
# tried first and every version of it read as a matte cylinder, for a reason
# that is structural rather than a matter of tuning: a vertex colour cannot
# carry a highlight narrower than the gap between two vertices. Round a
# twelve-sided tube that gap is thirty degrees, so a tight specular either
# falls between samples and vanishes or Gouraud smears it into exactly the soft
# top-to-bottom gradient that says "plastic". Quantising instead puts adjacent
# vertices on the *same* value, so the span between them interpolates flat and
# the step lands where the band changes. Flat bands with hard edges is also what
# N64 chrome actually looked like -- it was a ramp texture, for the same reason.
#
# Written in sRGB bytes because that is what the eye reads and what a colour
# picker shows. `linear` converts on the way in; the shader's own terms are
# linear and the surface converts back on the way out.
COIL_RAMP = (
    (40, 20, 12),      # the underside, in shadow of the machine's own shape
    (88, 44, 24),      # body, turned away
    (150, 84, 44),     # body -- the copper this reads as at a glance
    (206, 138, 86),    # the lit face
    (255, 232, 200),   # the hot line, gone to white the way polished metal does
)

# Lifted so the middle of the ramp survives the shader's own term, which runs
# from `N64Lighting`'s ambient at about 0.44 to a little over 1.0 and is a
# multiplier on all of this. Without it the bands above arrive a third darker
# than they are written.
COIL_EXPOSURE = 1.30

# How far up the ramp each of the two things a coil reflects can push it.
#
# The sky alone stops short of the top band: an upward-facing surface is *body*
# brightness, not a highlight, which is what keeps the machine from going pale
# all over when the camera looks down on it. Only the sun reaching the surface
# opens the last band, so the white line appears where a reflection would be
# and nowhere else.
COIL_SKY_REACH = 0.55
COIL_GLINT_REACH = 0.45

# How much of the normal's vertical span the sky-to-ground turn is spread over.
# Small is a mirror and large is a matte ball.
COIL_HORIZON = 0.34

# The direction the sun sits at noon -- the same vector as
# `N64Lighting::default().to_light` in `src/n64.rs` -- and how tight the
# reflection of it is.
#
# Baked against noon and not against the hour, because a vertex colour cannot
# follow the sun. `sky.rs` does move the key light and the live term this is
# multiplied by follows it; what is fixed here is only where the *sharp* glint
# sits. The castle's whole lighting is baked the same way and for the same
# reason. Its job is to break the vertical symmetry the sky term has on its
# own, which is most of the difference between a polished coil and a pipe.
COIL_GLINT_TOWARDS = (0.35, 0.86, 0.37)
COIL_GLINT_TIGHTNESS = 4.0


def linear(srgb):
    """An sRGB byte triple as the linear floats a glTF and the shader work in."""
    c = np.asarray(srgb, dtype=np.float64) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def polished(normals, base):
    """COLOR_0 for a metal surface, from its game-space normals.

    Divided through by the material's own base colour so that `COIL_RAMP` is
    the colour that actually reaches the screen rather than a multiplier on
    something else. The shader computes `base_color * COLOR_0`, so this is the
    one place the two have to agree, and stating the ramp absolutely is what
    lets it be written in the sRGB a colour picker shows.

    Returned as float VEC4 rather than normalised bytes so the highlights can
    carry values over 1.0 instead of clipping at the top of a byte.
    """
    n = np.asarray(normals, dtype=np.float64)
    n = n / np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)

    # Up towards the sky and down towards the dirt, over `COIL_HORIZON`.
    sky = np.clip(0.5 + n[:, 1] / (2.0 * COIL_HORIZON), 0.0, 1.0)

    towards = np.asarray(COIL_GLINT_TOWARDS, dtype=np.float64)
    towards /= np.linalg.norm(towards)
    glint = np.clip(n @ towards, 0.0, None) ** COIL_GLINT_TIGHTNESS

    # How far up the ramp this surface sits, and then which band that is. The
    # floor is what makes the bands flat; interpolating the ramp instead would
    # put the smooth gradient straight back.
    t = np.clip(sky * COIL_SKY_REACH + glint * COIL_GLINT_REACH, 0.0, 1.0)
    bands = np.asarray([linear(step) for step in COIL_RAMP]) * COIL_EXPOSURE
    colour = bands[np.clip((t * len(bands)).astype(int), 0, len(bands) - 1)]

    # Opaque: the shader multiplies the vertex colour into the tint whole, so
    # anything but 1.0 here is a coil you can see through.
    return np.column_stack((colour / np.asarray(base[:3]), np.ones(len(n))))


def add_material(glb: GLB, name: str, color, metallic=0.0, roughness=0.5, emissive=None):
    material = {
        "name": name,
        "pbrMetallicRoughness": {
            "baseColorFactor": list(color),
            "metallicFactor": metallic,
            "roughnessFactor": roughness,
        },
        "doubleSided": True,
    }
    if emissive:
        material["emissiveFactor"] = list(emissive)
    glb.json["materials"].append(material)
    return len(glb.json["materials"]) - 1


def to_game(vectors):
    """The shape's own Z-up into the Y-up a glTF is read in.

    Everything above is written the way a stellarator is described -- the
    machine lies in the x/y plane and its axis is z -- and this is the one place
    that becomes a file. It used to be nowhere: the file was written Z-up and
    read by a game that turned the model a quarter about X on the way in, with
    `flux_point` in `src/stellarator.rs` making the same turn a second time in
    arithmetic. Two corrections, in another language, in another repository
    directory, for a file that could simply have been right.

    Blender does not read either of them. Linked into a level for placing, the
    machine lay on its side.
    """
    v = np.asarray(vectors, dtype=np.float64)
    return np.column_stack((v[:, 0], v[:, 2], -v[:, 1]))


def add_mesh(glb: GLB, name: str, vertices, normals, faces, material: int, colors=None):
    vertices = np.asarray(vertices, dtype=np.float32)
    normals = np.asarray(normals, dtype=np.float32)
    faces = np.asarray(faces, dtype=np.uint32).reshape(-1)
    pos = glb.add_array(vertices.tolist(), FLOAT, "VEC3", ARRAY_BUFFER, True)
    nor = glb.add_array(normals.tolist(), FLOAT, "VEC3", ARRAY_BUFFER)
    ind = glb.add_array(faces.tolist(), UNSIGNED_INT, "SCALAR", ELEMENT_ARRAY_BUFFER)
    attributes = {"POSITION": pos, "NORMAL": nor}
    if colors is not None:
        attributes["COLOR_0"] = glb.add_array(
            np.asarray(colors, dtype=np.float32).tolist(), FLOAT, "VEC4", ARRAY_BUFFER)
    glb.json["meshes"].append({
        "name": name,
        "primitives": [{"attributes": attributes, "indices": ind,
                        "material": material}],
    })
    glb.json["nodes"].append({"name": name, "mesh": len(glb.json["meshes"]) - 1})
    glb.json["scenes"][0]["nodes"].append(len(glb.json["nodes"]) - 1)


def generate(output: Path, field_periods: int = 5, coils: int = 10,
             plasma_segments: int = 160, plasma_sides: int = 32,
             coil_segments: int = 128, coil_sides: int = 12,
             scale: float = 1.0):
    # `--scale` is a multiplier on the machine's real width rather than on the
    # shape's arbitrary unit, so `1.0` is a stellarator the size a stellarator
    # is and the flag is only ever reached for to make an odd one.
    scale *= MACHINE_WIDTH / SHAPE_WIDTH
    glb = GLB()
    glb.json["asset"]["generator"] = "Python stellarator geometry generator"
    glb.json["scenes"][0]["name"] = "Stellarator"

    plasma_mat = add_material(glb, "Plasma", (0.16, 0.52, 1.0, 0.58), 0.05, 0.22,
                              emissive=(0.05, 0.2, 0.8))
    # This colour and the metallic and roughness beside it are what the material
    # is *for a glTF viewer*: the game reads none of the three. `polished`
    # divides this back out of COLOR_0, so what reaches the screen in game is
    # `COIL_RAMP` whatever is written here -- which leaves it free to be the
    # honest description of the surface that Blender wants.
    coil_base = (0.55, 0.26, 0.15, 1)
    coil_mat = add_material(glb, "Polished Copper Coils", coil_base, 1.0, 0.25)

    major, minor = 3.0, 0.72

    def plasma(u, v):
        # A toroidal plasma surface with a rotating elliptical cross section.
        twist = field_periods * u
        xsec = minor * (1.0 + 0.16 * math.cos(twist))
        r = major + xsec * math.cos(v) + 0.16 * math.cos(twist)
        z = 0.72 * minor * math.sin(v) + 0.18 * math.sin(twist)
        return (r * math.cos(u), r * math.sin(u), z)

    verts, norms, faces = surface_mesh(plasma, plasma_segments, plasma_sides)
    # The plasma carries no COLOR_0: it is a glow rather than a surface light
    # falls on, and multiplying an environment into it would only dirty it.
    parts = [("Twisted Plasma Surface", verts, norms, faces, plasma_mat, False)]

    # Non-planar modular coils wrap around the plasma boundary.
    for k in range(coils):
        phase = 2 * math.pi * k / coils
        t = np.linspace(0, 2 * math.pi, coil_segments, endpoint=False)
        u = phase + 0.24 * np.sin(t) + 0.07 * np.sin(2 * t + field_periods * phase)
        local_twist = field_periods * u
        r = major + 1.13 * np.cos(t) + 0.2 * np.cos(local_twist)
        z = 0.92 * np.sin(t) + 0.22 * np.sin(local_twist)
        points = np.column_stack((r * np.cos(u), r * np.sin(u), z))
        parts.append((f"Modular Coil {k + 1:02d}",) + tube_mesh(points, 0.105, coil_sides)
                     + (coil_mat, True))

    # Turned upright, brought to the size asked for, and stood on its own
    # origin -- which is the shape a model in this game is authored in, and what
    # lets a level place one by dropping an empty on the ground. The lift is the
    # machine's own lowest point rather than a number written here, so a coil
    # that reaches further down carries the whole machine up with it.
    parts = [(name, to_game(v) * scale, to_game(n), f, m, metal)
             for name, v, n, f, m, metal in parts]
    stands_on = min(float(v[:, 1].min()) for _, v, _, _, _, _ in parts)
    for name, v, n, f, m, metal in parts:
        # After `to_game`, so the environment is sampled against the up the
        # game reads the file in rather than the z the shape is written in.
        add_mesh(glb, name, v - (0.0, stands_on, 0.0), n, f, m,
                 polished(n, coil_base) if metal else None)

    output.parent.mkdir(parents=True, exist_ok=True)
    glb.write(output)
    return len(glb.json["meshes"]), sum(a["count"] for a in glb.json["accessors"] if a["type"] == "VEC3")


def make_blend(glb_path: Path):
    """Import the generated GLB and save an editable sibling Blender file."""
    glb_path = glb_path.resolve()
    blend_path = glb_path.with_suffix(".blend")
    command = [
        str(PROJECT_BLENDER), "--background", "-noaudio", "--factory-startup",
        "--python", str(BLEND_IMPORTER), "--python-exit-code", "1", "--",
        str(glb_path), str(blend_path),
    ]
    try:
        subprocess.run(command, check=True, timeout=20)
    except subprocess.TimeoutExpired:
        # Some headless Linux audio stacks keep Blender alive after it has
        # finished and saved. subprocess.run has terminated it by this point;
        # the completed file is authoritative.
        if not blend_path.is_file():
            raise
    return blend_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", nargs="?", type=Path, default=Path("stellarator.glb"))
    parser.add_argument("--field-periods", type=int, default=5)
    parser.add_argument("--coils", type=int, default=10)
    parser.add_argument("--low-poly", action="store_true",
                        help="use a compact, visibly faceted geometry preset")
    parser.add_argument("--ultra-low-poly", action="store_true",
                        help="use an aggressively simplified geometry preset")
    parser.add_argument("--plasma-segments", type=int, default=160)
    parser.add_argument("--plasma-sides", type=int, default=32)
    parser.add_argument("--coil-segments", type=int, default=128)
    parser.add_argument("--coil-sides", type=int, default=12)
    parser.add_argument("--scale", type=float, default=1.0,
                        help="multiplier on MACHINE_WIDTH, baked into vertex "
                             "positions; 1.0 is the machine's real size")
    parser.add_argument("--no-blend", action="store_true",
                        help="write only the GLB instead of a sibling .blend")
    args = parser.parse_args()
    if args.ultra_low_poly:
        args.plasma_segments, args.plasma_sides = 24, 8
        args.coil_segments, args.coil_sides = 24, 4
    elif args.low_poly:
        args.plasma_segments, args.plasma_sides = 48, 12
        # Twelve sides round the coil rather than the six this preset used
        # before, and the reason is `COIL_RAMP` rather than the silhouette. A
        # baked reflection lives in the vertices, so the tube's own resolution
        # is the finest band it can hold: at six sides the normals are sixty
        # degrees apart, the hot band spans a whole facet, and Gouraud washes
        # it into a gradient that reads as shiny plastic. Twelve is where the
        # band closes up into a line. It costs 2,400 vertices over the whole
        # machine, which is a fifth of what the plasma alone spends.
        args.coil_segments, args.coil_sides = 40, 12
    meshes, vector_count = generate(
        args.output, args.field_periods, args.coils,
        args.plasma_segments, args.plasma_sides,
        args.coil_segments, args.coil_sides,
        args.scale,
    )
    print(f"Wrote {args.output} ({meshes} meshes, {vector_count // 2} vertices, "
          f"{MACHINE_WIDTH * args.scale:.2f} m across)")
    if not args.no_blend:
        blend = make_blend(args.output)
        print(f"Wrote {blend}")


if __name__ == "__main__":
    main()
