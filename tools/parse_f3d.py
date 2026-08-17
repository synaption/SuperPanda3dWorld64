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
import shutil
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
GEO_CMD_START_RE = re.compile(r"\b(GEO_[A-Z0-9_]+)\s*\(")
INCLUDE_TEXTURE_RE = re.compile(
    r"const\s+u8\s+(\w+)\s*\[\s*\]\s*=\s*\{\s*#include\s+\"([^\"]+)\"", re.S
)

# Solid-coloured actor parts get their colour from a light group rather than
# from vertex colours or a texture: gdSPDefLights1(ambient_rgb, diffuse_rgb,
# direction).  Without these, Mario's shirt and overalls come out untinted.
LIGHTS_RE = re.compile(
    r"const\s+Lights1\s+(\w+)\s*=\s*gdSPDefLights1\s*\(([^)]*)\)", re.S
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

# Colour combiners that use only the shade colour, never a texel.
SHADE_ONLY_COMBINERS = {"G_CC_SHADE", "G_CC_SHADEFADEA"}

# BLEND lerps the texel over the *shade colour* using the texel's own alpha,
# all within the polygon. The result is opaque: where the texture is
# transparent the shade colour shows, not whatever is behind the surface.
#
# Mario's eyes, mustache, cap logo and overall buttons are all BLEND. Treating
# their alpha as see-through cuts holes in his face; treating them as a plain
# multiply turns a yellow button on blue denim black. Neither is right -- the
# texture has to be composited over the light colour.
BLEND_COMBINERS = {"G_CC_BLENDRGBA", "G_CC_BLENDRGBFADEA",
                   "G_CC_BLENDRGBDECALA"}

# DECAL outputs the texel directly and leans on the blender for transparency.
DECAL_COMBINERS = {"G_CC_DECALRGB", "G_CC_DECALRGBA",
                   "G_CC_DECALFADE", "G_CC_DECALFADEA"}


def combiner_kind(combiner):
    """How a group's texture and shade colour combine."""
    if combiner in BLEND_COMBINERS:
        return "blend"
    if combiner in DECAL_COMBINERS:
        return "decal"
    if combiner in SHADE_ONLY_COMBINERS:
        return "shade"
    return "modulate"


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


def _scan_commands(body, start_re=CMD_START_RE):
    """Yield (name, args) pairs, matching parentheses properly."""
    out = []
    for match in start_re.finditer(body):
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


PROJECT_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


def as_project_relative(path):
    """Store paths relative to the project when possible.

    The converted assets get read back on whatever machine runs the game, and
    this project is likely to be built under WSL and run from Windows against
    the same files. An absolute path baked in at conversion time would not
    resolve there, and the texture would silently go missing.
    """
    path = os.path.abspath(path)
    try:
        relative = os.path.relpath(path, PROJECT_ROOT)
    except ValueError:  # different drive on Windows
        return path
    if relative.startswith(os.pardir):
        return path
    return relative.replace(os.sep, "/")


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
            resolved[symbol] = as_project_relative(candidate)
    return resolved


def collect_textures(groups, out_dir, hd_pack):
    """Copy the textures this level actually uses in beside the mesh.

    Without this the converted level points back into the HD texture pack,
    which is 12 GB of third-party material that cannot be tracked -- so a fresh
    clone parses fine and then draws the whole castle grounds untextured. Only
    the images the level references get copied: 21 of them, 2.8 MB, against the
    thousands in the pack.

    The pack's own directory structure is preserved rather than flattened,
    because two of these are both called `0.rgba16.png`.
    """
    prefix = os.path.join(os.path.abspath(hd_pack), "gfx") + os.sep
    copied = {}
    for group in groups:
        source = group.get("image")
        if not source:
            continue
        if source in copied:
            group["image"] = copied[source]
            continue

        absolute = os.path.abspath(os.path.join(PROJECT_ROOT, source))
        if not absolute.startswith(prefix):
            continue
        relative = absolute[len(prefix):].replace(os.sep, "/")
        destination = os.path.join(out_dir, "textures", relative)
        os.makedirs(os.path.dirname(destination), exist_ok=True)
        shutil.copyfile(absolute, destination)

        copied[source] = as_project_relative(destination)
        group["image"] = copied[source]
    return copied


class Level:
    """All Vtx and Gfx arrays visible to one level, parsed once."""

    def __init__(self):
        self.vertices = {}
        self.display_lists = {}
        self.lights = {}

    def add_source(self, path):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as fh:
                source = fh.read()
        except OSError:
            return

        for name, body in LIGHTS_RE.findall(source):
            values = [_eval(a) & 0xFF for a in _split_args(body)]
            if len(values) >= 6:
                self.lights[name] = {
                    "ambient": tuple(values[0:3]),
                    "diffuse": tuple(values[3:6]),
                    # Signed direction bytes. The RSP normalizes this vector
                    # before taking its per-vertex dot product.
                    "direction": tuple(
                        value - 256 if value > 127 else value
                        for value in values[6:9]
                    ) if len(values) >= 9 else (0, 0, 127),
                }

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

        # Set by the actor exporter before running each part's display list.
        # It joins the dedup key so a vertex shared between two parts becomes
        # two vertices, one per bone -- required for rigid skinning.
        self.bone = 0
        self.vertex_bones = []

        self._texture = None
        self._layer = "OPAQUE"
        self._lighting = False
        self._cull = True
        self._tile = (32, 32)
        self._wrap = ("wrap", "wrap")
        self._light = None
        self._combiner = None
        # Texture state persists across display lists, so a part that draws
        # untextured has to actively say so -- via gsSPTexture(..., G_OFF) or
        # a shade-only combine mode. Without tracking that, the last texture
        # bound leaks onto every solid-coloured part after it.
        self._texture_on = True

    # -- material tracking --------------------------------------------------

    def _state(self):
        texture = self._texture if self._texture_on else None
        # A bound light group means the part is lit, whatever the geometry
        # mode said; actor parts set G_LIGHTING outside the lists walked here.
        lighting = self._lighting or self._light is not None
        return (texture, self._layer, lighting, self._cull,
                self._tile, self._wrap, self._light, self._combiner)

    def _flush_group(self):
        count = len(self.triangles) - self._group_start
        if count > 0:
            (texture, layer, lighting, cull, tile, wrap, light,
             combiner) = self._pending_state
            entry = self.level.lights.get(light) if light else None
            self.groups.append({
                "texture": texture,
                "layer": layer,
                "lighting": lighting,
                "cull": cull,
                "tile_width": tile[0],
                "tile_height": tile[1],
                "wrap_s": wrap[0],
                "wrap_t": wrap[1],
                "light": light,
                "light_diffuse": entry["diffuse"] if entry else None,
                "light_ambient": entry["ambient"] if entry else None,
                "light_direction": entry["direction"] if entry else None,
                "combiner": combiner,
                "combiner_kind": combiner_kind(combiner),
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
        # S10.5 texel coordinates -> normalised UV, left in the N64's own
        # convention: origin top-left, V increasing downward.
        #
        # That matches glTF exactly, so the actor exporter uses these as-is.
        # Panda3D's native texture coordinates run from the bottom-left, so
        # sm64py/level.py flips V when it builds geometry directly. Flipping
        # here instead would silently mirror every actor texture -- the giveaway
        # was Mario's cap logo reading as a W.
        key = (entry, tile_w, tile_h, self.bone)
        index = self._emitted.get(key)
        if index is None:
            index = len(self.positions)
            self.positions.append(entry[0:3])
            self.uvs.append((
                entry[3] / 32.0 / max(tile_w, 1),
                entry[4] / 32.0 / max(tile_h, 1),
            ))
            self.colors.append(entry[5:9])
            self.vertex_bones.append(self.bone)
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
            elif cmd == "gsSPLight":
                self._set_light(args)
            elif cmd == "gsSPTexture":
                # Last argument is G_ON / G_OFF.
                if args:
                    self._texture_on = "G_OFF" not in args[-1]
            elif cmd == "gsDPSetCombineMode":
                self._combiner = args[0].strip() if args else None
                # Shade-only modes sample no texel at all.
                self._texture_on = not any(
                    a.strip() in SHADE_ONLY_COMBINERS for a in args
                )
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

    def _set_light(self, args):
        # gsSPLight(&group.l, 1) binds the diffuse light; slot 2 is the
        # ambient half of the same group, so only slot 1 needs reading.
        if len(args) < 2 or _eval(args[1]) != 1:
            return
        symbol = args[0].lstrip("&").split(".")[0].strip()
        if symbol:
            self._light = symbol

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

    out_dir = os.path.dirname(os.path.abspath(args.out))
    os.makedirs(out_dir, exist_ok=True)
    copied = collect_textures(groups, out_dir, hd_pack) if hd_pack else {}
    np.savez_compressed(args.out, **mesh)
    with open(os.path.splitext(args.out)[0] + "_materials.json", "w",
              encoding="utf-8") as fh:
        json.dump(groups, fh, indent=2)

    textured = sum(1 for g in groups if g["image"])
    print(f"parsed {n_vtx} vertex arrays, {n_dl} display lists, "
          f"{n_tex} textures resolved from the HD pack")
    if copied:
        size = sum(os.path.getsize(os.path.join(PROJECT_ROOT, p))
                   for p in set(copied.values()))
        print(f"copied {len(copied)} used textures ({size / 1e6:.1f} MB) "
              f"into {os.path.join(out_dir, 'textures')}")
    print(f"roots: {len(roots)}")
    print(f"{len(mesh['positions'])} vertices, {len(mesh['triangles'])} triangles, "
          f"{len(groups)} material groups ({textured} textured) -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
