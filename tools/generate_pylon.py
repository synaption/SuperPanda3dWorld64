#!/usr/bin/env python3
"""Generate the network pylon as a binary glTF asset.

A pylon is the thing you plant to carry power across the level: a squat
footing, a tapered lattice mast, a collar of three arms, and an emitter head at
the top that the beams in `src/pylon.rs` are strung between.

Written procedurally for `tools/generate_stellarator.py`'s reasons. The shape is
a handful of numbers rather than a modelling session, the file it writes is the
one the game measures itself off, and `--blend` hands Blender an editable copy
whenever somebody would rather push the vertices around by hand.

Everything below is in game metres and in the game's own axes -- Y is up, the
mast stands on the origin -- so nothing has to be turned on the way in. For
scale: Luna is 1.93 m tall, a warp pipe is 3 m, and a stellarator is 32 m
across.
"""

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

# The node names the game looks the model's parts up by. `src/pylon.rs` finds
# the emitter to hang a beam off it and to pulse it while the network is live,
# exactly the way `src/stellarator.rs` finds its coils, so these two strings are
# a contract between this file and that one.
EMITTER_NODE = "Pylon Emitter"
MAST_NODE = "Pylon Mast"
FOOTING_NODE = "Pylon Footing"

# How tall a pylon stands, in game metres, and how much ground it takes up.
#
# Four times Luna's height: tall enough to be the thing you look for across
# a valley, short enough that a line of them does not read as a fence of
# skyscrapers. The footing is what the placement rule in `src/pylon.rs` keeps
# clear -- the game reads both numbers back off this file rather than repeating
# them, so this is the only place either is written down.
HEIGHT = 8.0
FOOT_RADIUS = 0.95

# Where the beam leaves from, as a fraction of the height. The emitter sits at
# the top of the mast with its own bulk above that, so this is a little under
# one.
EMITTER_AT = 0.90

# The frame's banded shading, dark to light with the sky.
#
# The same trick the stellarator's coils use and for the same reason: this
# renderer lights the world once, flatly, and a mast shaded only by that is a
# grey stick. Baking the environment into COLOR_0 gives it a top and a bottom.
# Stated in sRGB bytes because that is what a colour picker shows.
FRAME_RAMP = (
    (28, 30, 36),
    (54, 58, 68),
    (86, 92, 104),
    (124, 132, 146),
    (168, 176, 190),
)

# How much of the ramp the sky term reaches, and over how much of the normal's
# vertical range it climbs. Under one so that a surface facing straight up is a
# step short of the top band and there is somewhere left for a highlight to go.
FRAME_SKY_REACH = 0.85
FRAME_HORIZON = 0.9


def linear(srgb):
    """An sRGB byte triple as the linear floats a glTF and the shader work in."""
    c = np.asarray(srgb, dtype=np.float64) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def banded(normals, base):
    """COLOR_0 for the frame, from its normals.

    Divided through by the material's own base colour, so `FRAME_RAMP` is what
    reaches the screen rather than a multiplier on something else: the shader
    computes `base_color * COLOR_0`, and this is the one place the two have to
    agree.
    """
    n = np.asarray(normals, dtype=np.float64)
    n = n / np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
    sky = np.clip(0.5 + n[:, 1] / (2.0 * FRAME_HORIZON), 0.0, 1.0)
    step = np.clip((sky * FRAME_SKY_REACH * len(FRAME_RAMP)).astype(int), 0, len(FRAME_RAMP) - 1)
    colour = np.asarray([linear(band) for band in FRAME_RAMP])[step]
    return np.column_stack((colour / np.asarray(base[:3]), np.ones(len(n))))


def faceted(triangles):
    """Turn a list of triangles into positions, normals and indices.

    Every triangle gets its own three vertices and one flat normal. That is
    three times the vertices a shared-vertex mesh would have and it is what
    makes the pylon read as folded plate rather than as a smooth extrusion --
    the same faceted look the rest of this game's low-poly actors have. The
    whole model is a few hundred vertices either way.
    """
    vertices, normals, faces = [], [], []
    for a, b, c in triangles:
        a, b, c = (np.asarray(p, dtype=np.float64) for p in (a, b, c))
        normal = np.cross(b - a, c - a)
        length = np.linalg.norm(normal)
        if length < 1e-12:
            # A degenerate triangle has no facing to shade it by and nothing to
            # draw. Dropped rather than written out with a zero normal, which
            # is a black facet.
            continue
        normal = normal / length
        base = len(vertices)
        vertices.extend((a, b, c))
        normals.extend((normal, normal, normal))
        faces.append((base, base + 1, base + 2))
    return (np.asarray(vertices), np.asarray(normals),
            np.asarray(faces, dtype=np.uint32))


def ring(radius, height, sides, twist=0.0):
    """One horizontal ring of `sides` points, at `height`."""
    return [(radius * math.cos(twist + 2 * math.pi * i / sides),
             height,
             radius * math.sin(twist + 2 * math.pi * i / sides))
            for i in range(sides)]


def skin(lower, upper):
    """The band of triangles between two rings of the same length."""
    out = []
    for i in range(len(lower)):
        j = (i + 1) % len(lower)
        out.append((lower[i], upper[i], upper[j]))
        out.append((lower[i], upper[j], lower[j]))
    return out


def cap(points, height, upward):
    """A fan closing a ring off, wound so its normal points the way asked."""
    centre = (0.0, height, 0.0)
    out = []
    for i in range(len(points)):
        j = (i + 1) % len(points)
        out.append((centre, points[i], points[j]) if upward
                   else (centre, points[j], points[i]))
    return out


def strut(start, end, radius, sides=4):
    """A square-section bar between two points, closed at both ends.

    Used for the collar arms and the diagonal braces. The section is built in a
    frame perpendicular to the run, so a brace leaning out from the mast keeps
    its thickness instead of being sheared into a wedge.
    """
    start = np.asarray(start, dtype=np.float64)
    end = np.asarray(end, dtype=np.float64)
    along = end - start
    length = np.linalg.norm(along)
    if length < 1e-9:
        return []
    along /= length
    # Any vector not parallel to the run gives a frame; up unless the run is
    # itself vertical, in which case the x axis will do.
    other = np.array([0.0, 1.0, 0.0])
    if abs(np.dot(other, along)) > 0.99:
        other = np.array([1.0, 0.0, 0.0])
    side = np.cross(along, other)
    side /= np.linalg.norm(side)
    up = np.cross(along, side)
    section = [(math.cos(2 * math.pi * i / sides), math.sin(2 * math.pi * i / sides))
               for i in range(sides)]
    lower = [tuple(start + radius * (u * side + v * up)) for u, v in section]
    upper = [tuple(end + radius * (u * side + v * up)) for u, v in section]
    out = skin(lower, upper)
    # Ends, as fans about each run's own centre rather than about the axis.
    for i in range(sides):
        j = (i + 1) % sides
        out.append((tuple(start), lower[j], lower[i]))
        out.append((tuple(end), upper[i], upper[j]))
    return out


def octahedron(centre, radius, squash=1.35):
    """The emitter head: eight facets, taller than it is wide."""
    cx, cy, cz = centre
    top = (cx, cy + radius * squash, cz)
    bottom = (cx, cy - radius * squash, cz)
    belt = ring(radius, cy, 4)
    belt = [(x + cx, y, z + cz) for x, y, z in belt]
    out = []
    for i in range(4):
        j = (i + 1) % 4
        out.append((belt[i], top, belt[j]))
        out.append((belt[i], belt[j], bottom))
    return out


def footing(sides=6):
    """The plinth the mast stands on, splayed a little at the ground."""
    lower = ring(FOOT_RADIUS, 0.0, sides)
    waist = ring(FOOT_RADIUS * 0.78, HEIGHT * 0.035, sides)
    upper = ring(FOOT_RADIUS * 0.55, HEIGHT * 0.075, sides)
    return (skin(lower, waist) + skin(waist, upper)
            + cap(lower, 0.0, upward=False) + cap(upper, HEIGHT * 0.075, upward=True))


def mast(sides=3):
    """The tapered lattice, and the braces across it.

    Three-sided, so the silhouette is a mast rather than a pipe from every
    angle, and split into segments so the taper has somewhere to bend and the
    braces have something to land on.
    """
    segments = 4
    top = HEIGHT * EMITTER_AT
    foot = HEIGHT * 0.075
    out = []
    rings = []
    for step in range(segments + 1):
        t = step / segments
        # Narrowing faster low down than high up, which is what makes it look
        # like it is carrying something rather than tapering to a point.
        radius = FOOT_RADIUS * (0.52 - 0.34 * t ** 0.7)
        rings.append(ring(radius, foot + (top - foot) * t, sides, twist=t * 0.35))
    for lower, upper in zip(rings, rings[1:]):
        out += skin(lower, upper)
        # One diagonal per face of each segment: the lattice, and the only part
        # of this that costs anything.
        for i in range(sides):
            j = (i + 1) % sides
            out += strut(lower[i], upper[j], FOOT_RADIUS * 0.055)
    out += cap(rings[0], rings[0][0][1], upward=False)
    # The collar: three arms thrown out under the emitter, which is what tells
    # you at a glance which way up the thing is.
    collar = HEIGHT * 0.78
    reach = FOOT_RADIUS * 1.25
    for i in range(3):
        angle = 2 * math.pi * i / 3 + 0.4
        out += strut((0.0, collar, 0.0),
                     (reach * math.cos(angle), collar + HEIGHT * 0.045,
                      reach * math.sin(angle)),
                     FOOT_RADIUS * 0.07)
    return out


def add_material(glb: GLB, name: str, colour, metallic=0.0, roughness=0.5, emissive=None):
    material = {
        "name": name,
        "pbrMetallicRoughness": {
            "baseColorFactor": list(colour),
            "metallicFactor": metallic,
            "roughnessFactor": roughness,
        },
        "doubleSided": True,
    }
    if emissive:
        material["emissiveFactor"] = list(emissive)
    glb.json["materials"].append(material)
    return len(glb.json["materials"]) - 1


def add_mesh(glb: GLB, name: str, vertices, normals, faces, material: int, colours=None):
    vertices = np.asarray(vertices, dtype=np.float32)
    normals = np.asarray(normals, dtype=np.float32)
    faces = np.asarray(faces, dtype=np.uint32).reshape(-1)
    attributes = {
        # Bounds asked for on the positions: `measure()` in `src/pylon.rs`
        # reads the model's own height and footprint straight out of the
        # accessor, so a pylon regenerated at another size needs no edit in the
        # game at all.
        "POSITION": glb.add_array(vertices.tolist(), FLOAT, "VEC3", ARRAY_BUFFER, True),
        "NORMAL": glb.add_array(normals.tolist(), FLOAT, "VEC3", ARRAY_BUFFER),
    }
    if colours is not None:
        attributes["COLOR_0"] = glb.add_array(
            np.asarray(colours, dtype=np.float32).tolist(), FLOAT, "VEC4", ARRAY_BUFFER)
    indices = glb.add_array(faces.tolist(), UNSIGNED_INT, "SCALAR", ELEMENT_ARRAY_BUFFER)
    glb.json["meshes"].append({
        "name": name,
        "primitives": [{"attributes": attributes, "indices": indices, "material": material}],
    })
    glb.json["nodes"].append({"name": name, "mesh": len(glb.json["meshes"]) - 1})
    glb.json["scenes"][0]["nodes"].append(len(glb.json["nodes"]) - 1)


def generate(output: Path, scale: float = 1.0):
    glb = GLB()
    glb.json["asset"]["generator"] = "Python pylon geometry generator"
    glb.json["scenes"][0]["name"] = "Pylon"

    frame_base = (0.42, 0.45, 0.52, 1)
    frame = add_material(glb, "Pylon Frame", frame_base, 0.85, 0.35)
    # The head is a light rather than a surface, so it carries no COLOR_0 and
    # no environment: multiplying one in would only dirty it.
    head = add_material(glb, "Pylon Emitter", (0.35, 0.85, 1.0, 1.0), 0.1, 0.25,
                        emissive=(0.15, 0.75, 1.0))

    parts = [
        (FOOTING_NODE, footing(), frame, True),
        (MAST_NODE, mast(), frame, True),
        (EMITTER_NODE, octahedron((0.0, HEIGHT * EMITTER_AT, 0.0), FOOT_RADIUS * 0.42),
         head, False),
    ]
    for name, triangles, material, shaded in parts:
        vertices, normals, faces = faceted(triangles)
        vertices = vertices * scale
        add_mesh(glb, name, vertices, normals, faces, material,
                 banded(normals, frame_base) if shaded else None)

    output.parent.mkdir(parents=True, exist_ok=True)
    glb.write(output)
    return len(glb.json["meshes"]), sum(
        a["count"] for a in glb.json["accessors"] if a["type"] == "VEC3")


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
        subprocess.run(command, check=True, timeout=60)
    except subprocess.TimeoutExpired:
        # Some headless Linux audio stacks keep Blender alive after it has
        # saved. `subprocess.run` has terminated it by now; the file on disk is
        # authoritative.
        if not blend_path.is_file():
            raise
    return blend_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", nargs="?", type=Path,
                        default=ROOT / "assets/actors/pylon.glb")
    parser.add_argument("--scale", type=float, default=1.0,
                        help="multiplier on the authored height of %.1f m" % HEIGHT)
    parser.add_argument("--blend", action="store_true",
                        help="also write an editable .blend beside the .glb, "
                             "overwriting whatever is there")
    args = parser.parse_args()
    meshes, vectors = generate(args.output, args.scale)
    print(f"Wrote {args.output} ({meshes} meshes, {vectors // 2} vertices, "
          f"{HEIGHT * args.scale:.2f} m tall)")
    if args.blend:
        print(f"Wrote {make_blend(args.output)}")


if __name__ == "__main__":
    main()
