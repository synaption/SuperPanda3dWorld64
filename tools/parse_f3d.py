"""Convert F3D display lists from a decomp level into a textured mesh.

The N64 microcode is a stream of commands over a 32-slot vertex cache:
gsSPVertex loads a run of vertices into the cache, and the triangle commands
index into it.  Walking that stream recovers the geometry without emulating
the RSP.

Two pieces of render state have to be tracked alongside the triangles or the
result looks nothing like the game:

  * The bound texture and its tile size.  Vertex UVs are S10.5 fixed point in
    texels, so they only become meaningful once the tile dimensions are known.
  * Whether G_LIGHTING is on.  The same four bytes in a vertex are a colour
    when lighting is off and a normal when it is on, so they cannot be
    interpreted without it.

Triangles are grouped into materials whenever any of that state changes.

Usage:
    python3 tools/parse_f3d.py <level_dir> <area> <out.npz> [--reference DIR]
"""

import argparse
import json
import os
import re
import sys

import numpy as np

# One Vtx entry: {{{x, y, z}, flag, {u, v}, {r, g, b, a}}}
VTX_ENTRY_RE = re.compile(
    r"\{\s*\{\s*\{\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*\}\s*,"
    r"\s*(-?\w+)\s*,"
    r"\s*\{\s*(-?\d+)\s*,\s*(-?\d+)\s*\}\s*,"
    r"\s*\{\s*(-?\w+)\s*,\s*(-?\w+)\s*,\s*(-?\w+)\s*,\s*(-?\w+)\s*\}\s*\}\s*\}"
)

ARRAY_RE = re.compile(
    r"(?:static\s+)?(?:ALIGNED8\s+)?const\s+(Vtx|Gfx)\s+(\w+)\s*\[\s*\]\s*=\s*\{(.*?)\n\}\s*;",
    re.S,
)

COMMENT_RE = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)
CMD_START_RE = re.compile(r"\b(gs[A-Za-z0-9_]+)\s*\(")
INCLUDE_TEXTURE_RE = re.compile(
    r"const\s+u8\s+(\w+)\s*\[\s*\]\s*=\s*\{\s*#include\s+\"([^\"]+)\"", re.S
)

VERTEX_CACHE_SIZE = 32

# Symbols that appear inside tile-size and mode expressions.
F3D_CONSTANTS = {
    "G_TEXTURE_IMAGE_FRAC": 2,
    "G_TX_NOMASK": 0,
    "G_TX_NOLOD": 0,
    "G_TX_WRAP": 0,
    "G_TX_MIRROR": 1,
    "G_TX_CLAMP": 2,
    "G_TX_RENDERTILE": 0,
    "G_TX_LOADTILE": 7,
    "G_ON": 1,
    "G_OFF": 0,
}

# Identifiers only where one can actually start -- the lookbehind keeps the
# "x70" inside a literal like 0x70 from being mistaken for a symbol.
EXPR_TOKEN_RE = re.compile(r"(?<![\w.])[A-Za-z_]\w*")


def _split_args(text):
    """Split a top-level comma list, ignoring commas inside nested parens."""
    text = COMMENT_RE.sub("", text)
    parts, depth, current = [], 0, []
    for ch in text:
        if ch == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
            continue
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        current.append(ch)
    if current:
        parts.append("".join(current).strip())
    return [p for p in parts if p]


def _eval(token, default=0):
    """Evaluate a small integer expression built from F3D constants."""
    token = token.strip()
    if not token:
        return default

    def replace(match):
        name = match.group(0)
        return str(F3D_CONSTANTS.get(name, 0))

    expr = EXPR_TOKEN_RE.sub(replace, token)
    if not re.fullmatch(r"[0-9xXa-fA-F()+\-*/<>|&~ ]*", expr):
        return default
    try:
        return int(eval(expr, {"__builtins__": {}}, {}))  # noqa: S307
    except Exception:
        return default


def _u8(token):
    return _eval(token) & 0xFF


def _scan_commands(body):
    """Yield (name, args) pairs, matching parentheses properly."""
    out = []
    for match in CMD_START_RE.finditer(body):
        name = match.group(1)
        start = match.end()
        depth, i = 1, start
        while i < len(body) and depth:
            if body[i] == "(":
                depth += 1
            elif body[i] == ")":
                depth -= 1
            i += 1
        out.append((name, _split_args(body[start:i - 1])))
    return out


def build_texture_map(reference):
    """Map texture symbol -> the decomp-relative path of its image data."""
    mapping = {}
    for sub in ("bin", "levels", "actors", "textures"):
        root = os.path.join(reference, sub)
        for dirpath, _dirs, files in os.walk(root):
            for name in files:
                if not name.endswith((".c", ".inc.c")):
                    continue
                path = os.path.join(dirpath, name)
                try:
                    with open(path, "r", encoding="utf-8", errors="replace") as fh:
                        source = fh.read()
                except OSError:
                    continue
                for symbol, include in INCLUDE_TEXTURE_RE.findall(source):
                    mapping[symbol] = include
    return mapping


def resolve_textures(symbol_to_include, hd_pack):
    """Point each texture symbol at a PNG in the HD pack, where one exists."""
    resolved = {}
    for symbol, include in symbol_to_include.items():
        png = include
        for suffix in (".inc.c", ".c"):
            if png.endswith(suffix):
                png = png[: -len(suffix)] + ".png"
                break
        candidate = os.path.join(hd_pack, "gfx", png)
        if os.path.exists(candidate):
            resolved[symbol] = candidate
    return resolved


class Level:
    """All Vtx and Gfx arrays visible to one level, parsed once."""

    def __init__(self):
        self.vertices = {}
        self.display_lists = {}

    def add_source(self, path):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                source = fh.read()
        except OSError:
            return

        for kind, name, body in ARRAY_RE.findall(source):
            if kind == "Vtx":
                self.vertices[name] = self._parse_vtx(body)
            else:
                self.display_lists[name] = _scan_commands(body)

    @staticmethod
    def _parse_vtx(body):
        out = []
        for match in VTX_ENTRY_RE.finditer(body):
            x, y, z, _flag, u, v, r, g, b, a = match.groups()
            out.append((
                int(x), int(y), int(z),
                int(u), int(v),
                _u8(r), _u8(g), _u8(b), _u8(a),
            ))
        return out


class MeshBuilder:
    """Walks display lists and accumulates triangles grouped by material."""

    def __init__(self, level):
        self.level = level
        self.positions = []
        self.uvs = []
        self.colors = []
        self.triangles = []
        self.groups = []

        self._cache = [None] * VERTEX_CACHE_SIZE
        self._emitted = {}
        self._group_start = 0

        self._texture = None
        self._layer = "OPAQUE"
        self._lighting = False
        self._cull = True
        self._tile = (32, 32)
        self._wrap = ("wrap", "wrap")

    # -- material tracking --------------------------------------------------

    def _state(self):
        return (self._texture, self._layer, self._lighting, self._cull,
                self._tile, self._wrap)

    def _flush_group(self):
        count = len(self.triangles) - self._group_start
        if count > 0:
            texture, layer, lighting, cull, tile, wrap = self._pending_state
            self.groups.append({
                "texture": texture,
                "layer": layer,
                "lighting": lighting,
                "cull": cull,
                "tile_width": tile[0],
                "tile_height": tile[1],
                "wrap_s": wrap[0],
                "wrap_t": wrap[1],
                "first": self._group_start,
                "count": count,
            })
        self._group_start = len(self.triangles)
        self._pending_state = self._state()

    def begin(self):
        self._pending_state = self._state()

    def _note_state_change(self):
        if self._state() != self._pending_state:
            self._flush_group()

    # -- geometry -----------------------------------------------------------

    def _vertex_index(self, slot):
        entry = self._cache[slot]
        if entry is None:
            return None
        tile_w, tile_h = self._tile
        # S10.5 texel coordinates -> normalised UV. V is flipped because the
        # N64 runs its texture origin from the top.
        key = (entry, tile_w, tile_h)
        index = self._emitted.get(key)
        if index is None:
            index = len(self.positions)
            self.positions.append(entry[0:3])
            self.uvs.append((
                entry[3] / 32.0 / max(tile_w, 1),
                1.0 - entry[4] / 32.0 / max(tile_h, 1),
            ))
            self.colors.append(entry[5:9])
            self._emitted[key] = index
        return index

    def _triangle(self, a, b, c):
        ia, ib, ic = (self._vertex_index(s) for s in (a, b, c))
        if None in (ia, ib, ic):
            return
        self.triangles.append((ia, ib, ic))

    def run(self, name, depth=0):
        commands = self.level.display_lists.get(name)
        if commands is None or depth > 32:
            return

        for cmd, args in commands:
            if cmd == "gsSPVertex":
                self._load_vertices(args)
            elif cmd == "gsSP1Triangle":
                self._note_state_change()
                self._triangle(_eval(args[0]), _eval(args[1]), _eval(args[2]))
            elif cmd == "gsSP2Triangles":
                self._note_state_change()
                self._triangle(_eval(args[0]), _eval(args[1]), _eval(args[2]))
                self._triangle(_eval(args[4]), _eval(args[5]), _eval(args[6]))
            elif cmd in ("gsSPDisplayList", "gsSPBranchList"):
                self.run(args[0], depth + 1)
                if cmd == "gsSPBranchList":
                    return
            elif cmd == "gsDPSetTextureImage":
                self._texture = args[-1]
            elif cmd == "gsDPSetTileSize":
                self._set_tile_size(args)
            elif cmd == "gsDPSetTile":
                self._set_tile(args)
            elif cmd == "gsSPSetGeometryMode":
                self._set_mode(args, True)
            elif cmd == "gsSPClearGeometryMode":
                self._set_mode(args, False)
            elif cmd == "gsSPEndDisplayList":
                return

    def _set_mode(self, args, value):
        flags = " ".join(args)
        if "G_LIGHTING" in flags:
            self._lighting = value
        if "G_CULL_BACK" in flags:
            self._cull = value

    def _set_tile_size(self, args):
        # (tile, uls, ult, lrs, lrt) with lrs/lrt in 10.2 fixed point.
        if len(args) < 5:
            return
        lrs, lrt = _eval(args[3]), _eval(args[4])
        self._tile = ((lrs >> 2) + 1, (lrt >> 2) + 1)

    def _set_tile(self, args):
        # cm_t is arg 6 and cm_s is arg 9 in the gsDPSetTile argument order.
        if len(args) < 10:
            return
        modes = {0: "wrap", 1: "mirror", 2: "clamp"}
        cm_t = _eval(args[6]) & 3
        cm_s = _eval(args[9]) & 3
        self._wrap = (modes.get(cm_s, "wrap"), modes.get(cm_t, "wrap"))

    def _load_vertices(self, args):
        target = args[0]
        offset = 0
        if "+" in target:
            target, _, off = target.partition("+")
            target, offset = target.strip(), _eval(off)

        verts = self.level.vertices.get(target)
        if verts is None:
            return

        count = _eval(args[1])
        dest = _eval(args[2])
        for i in range(count):
            src, slot = offset + i, dest + i
            if src < len(verts) and slot < VERTEX_CACHE_SIZE:
                self._cache[slot] = verts[src]

    def finish(self):
        self._flush_group()
        return {
            "positions": np.array(self.positions, dtype=np.float32).reshape(-1, 3),
            "uvs": np.array(self.uvs, dtype=np.float32).reshape(-1, 2),
            "colors": np.array(self.colors, dtype=np.uint8).reshape(-1, 4),
            "triangles": np.array(self.triangles, dtype=np.int32).reshape(-1, 3),
            "groups": self.groups,
        }


GEO_DL_RE = re.compile(r"GEO_DISPLAY_LIST\s*\(\s*LAYER_(\w+)\s*,\s*(\w+)\s*\)")


def find_root_display_lists(geo_path):
    with open(geo_path, "r", encoding="utf-8", errors="replace") as fh:
        return GEO_DL_RE.findall(fh.read())


def build(level_dir, area, reference, hd_pack):
    area_dir = os.path.join(level_dir, "areas", str(area))

    level = Level()
    for root, _dirs, files in os.walk(level_dir):
        for name in files:
            if name.endswith((".inc.c", ".c")):
                level.add_source(os.path.join(root, name))

    roots = find_root_display_lists(os.path.join(area_dir, "geo.inc.c"))

    builder = MeshBuilder(level)
    builder.begin()
    for layer, dl_name in roots:
        builder._layer = layer
        builder.run(dl_name)

    mesh = builder.finish()

    textures = resolve_textures(build_texture_map(reference), hd_pack) if hd_pack else {}
    for group in mesh["groups"]:
        group["image"] = textures.get(group["texture"])

    mesh["_roots"] = roots
    mesh["_counts"] = (len(level.vertices), len(level.display_lists), len(textures))
    return mesh


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("level_dir")
    parser.add_argument("area")
    parser.add_argument("out")
    parser.add_argument("--reference", default=None,
                        help="decomp root (defaults to three levels above level_dir)")
    parser.add_argument("--hd-pack", default=None,
                        help="root of the HD texture pack")
    args = parser.parse_args(argv[1:])

    reference = args.reference or os.path.abspath(
        os.path.join(args.level_dir, "..", "..")
    )
    hd_pack = args.hd_pack
    if hd_pack is None:
        guess = os.path.join(reference, "..", "RENDER96-HD-TEXTURE-PACK")
        hd_pack = os.path.abspath(guess) if os.path.isdir(guess) else None

    mesh = build(args.level_dir, args.area, reference, hd_pack)
    roots = mesh.pop("_roots")
    n_vtx, n_dl, n_tex = mesh.pop("_counts")
    groups = mesh.pop("groups")

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    np.savez_compressed(args.out, **mesh)
    with open(os.path.splitext(args.out)[0] + "_materials.json", "w",
              encoding="utf-8") as fh:
        json.dump(groups, fh, indent=2)

    textured = sum(1 for g in groups if g["image"])
    print(f"parsed {n_vtx} vertex arrays, {n_dl} display lists, "
          f"{n_tex} textures resolved from the HD pack")
    print(f"roots: {len(roots)}")
    print(f"{len(mesh['positions'])} vertices, {len(mesh['triangles'])} triangles, "
          f"{len(groups)} material groups ({textured} textured) -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
