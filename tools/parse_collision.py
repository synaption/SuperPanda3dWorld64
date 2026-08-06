"""Convert a decomp `collision.inc.c` into the .npz the runtime loads.

The file is a flat s16 stream dressed up in macros, so parsing it only means
reading the macro calls in source order and following the same state machine
the game's terrain loader uses:

    COL_INIT / COL_VERTEX_INIT(n) / COL_VERTEX(x,y,z) * n
    COL_TRI_INIT(type, n) / COL_TRI(a,b,c) * n     ... repeated per type
    COL_TRI_STOP
    SPECIAL_OBJECT*(...)                            ... optional
    COL_WATER_BOX_INIT(n) / COL_WATER_BOX(...) * n  ... optional
    COL_END

Usage:
    python3 tools/parse_collision.py <collision.inc.c> <out.npz>
"""

import json
import os
import re
import sys

import numpy as np

REFERENCE = os.path.join(os.path.dirname(__file__), "..", "reference", "Render96ex")

# Surface types that carry a fourth parameter on each triangle.
TYPES_WITH_FORCE = {
    "SURFACE_0004",
    "SURFACE_FLOWING_WATER",
    "SURFACE_DEEP_MOVING_QUICKSAND",
    "SURFACE_SHALLOW_MOVING_QUICKSAND",
    "SURFACE_MOVING_QUICKSAND",
    "SURFACE_HORIZONTAL_WIND",
    "SURFACE_INSTANT_MOVING_QUICKSAND",
}

MACRO_RE = re.compile(r"\b(COL_[A-Z_0-9]+|SPECIAL_OBJECT[A-Z_]*)\s*\(([^)]*)\)")
COMMENT_RE = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)
DEFINE_RE = re.compile(r"^#define\s+(SURFACE_[A-Z_0-9]+)\s+(0x[0-9A-Fa-f]+)", re.M)


def load_surface_types(reference=REFERENCE):
    """Map SURFACE_* names to their numeric values from surface_terrains.h."""
    header = os.path.join(reference, "include", "surface_terrains.h")
    with open(header, "r", encoding="utf-8", errors="replace") as fh:
        return {name: int(value, 16) for name, value in DEFINE_RE.findall(fh.read())}


def _args(text):
    """Split a macro argument list, dropping /* label */ comments and blanks."""
    text = COMMENT_RE.sub("", text)
    return [part.strip() for part in text.split(",") if part.strip()]


def _int(token):
    return int(token, 0)


def parse(path, surface_types):
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        source = fh.read()

    vertices = []
    tri_verts = []
    tri_type = []
    tri_force = []
    water_boxes = []
    special_objects = []

    # Set by COL_VERTEX_INIT / COL_TRI_INIT and consumed by the entries after.
    pending_type = None
    pending_force = False

    for name, raw in MACRO_RE.findall(source):
        args = _args(raw)

        if name == "COL_VERTEX":
            vertices.append([_int(a) for a in args[:3]])

        elif name == "COL_TRI_INIT":
            pending_type = args[0]
            pending_force = pending_type in TYPES_WITH_FORCE

        elif name in ("COL_TRI", "COL_TRI_SPECIAL"):
            tri_verts.append([_int(a) for a in args[:3]])
            tri_type.append(surface_types[pending_type])
            tri_force.append(_int(args[3]) if pending_force and len(args) > 3 else 0)

        elif name == "COL_WATER_BOX":
            # id, x1, z1, x2, z2, y
            water_boxes.append([_int(a) for a in args[:6]])

        elif name.startswith("SPECIAL_OBJECT"):
            entry = {"preset": args[0], "pos": [_int(a) for a in args[1:4]]}
            if len(args) > 4:
                entry["yaw"] = _int(args[4])
            if len(args) > 5:
                entry["param"] = _int(args[5])
            special_objects.append(entry)

    return {
        "vertices": np.array(vertices, dtype=np.int32),
        "tri_verts": np.array(tri_verts, dtype=np.int32),
        "tri_type": np.array(tri_type, dtype=np.int32),
        "tri_force": np.array(tri_force, dtype=np.int32),
        "water_boxes": np.array(water_boxes, dtype=np.int32).reshape(-1, 6),
        "special_objects": special_objects,
    }


def main(argv):
    if len(argv) != 3:
        print(__doc__.strip())
        return 1

    src, dst = argv[1], argv[2]
    data = parse(src, load_surface_types())
    specials = data.pop("special_objects")

    os.makedirs(os.path.dirname(os.path.abspath(dst)), exist_ok=True)
    np.savez_compressed(dst, **data)
    with open(os.path.splitext(dst)[0] + "_objects.json", "w", encoding="utf-8") as fh:
        json.dump(specials, fh, indent=2)

    print(
        f"{len(data['vertices'])} vertices, {len(data['tri_verts'])} triangles, "
        f"{len(data['water_boxes'])} water boxes, {len(specials)} special objects "
        f"-> {dst}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
