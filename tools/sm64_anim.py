"""Decode SM64 actor animations.

An animation is three pieces: a small header, a `values` table of s16, and an
`indices` table of u16 read as (frame_count, offset) pairs.  Each pair says
where one component of one joint lives:

    frame < frame_count  ->  values[offset + frame]
    otherwise            ->  values[offset + frame_count - 1]   (hold last)

A pair with frame_count == 1 is therefore a constant, which is how the format
stays small.  Pairs are consumed strictly in joint visit order: three for the
root's translation, then three rotations per joint.  Nothing is self
-describing, so a joint skipped while walking the hierarchy desyncs everything
after it.
"""

import os
import re

# Header fields, in order:
# flags, y_trans_divisor, start_frame, loop_start, loop_end, num_parts,
# values symbol, indices symbol, length
ANIM_HEADER_RE = re.compile(
    r"const\s+struct\s+Animation\s+(\w+)\s*\[\s*\]\s*=\s*\{(.*?)\}\s*;", re.S
)
ARRAY_RE = re.compile(
    r"const\s+(u16|s16)\s+(\w+)\s*\[\s*\]\s*=\s*\{(.*?)\}\s*;", re.S
)
COMMENT_RE = re.compile(r"/\*.*?\*/|//[^\n]*", re.S)

ANIM_FLAG_NOLOOP = 1 << 0
ANIM_FLAG_FORWARD = 1 << 1
ANIM_FLAG_HOR_TRANS = 1 << 3
ANIM_FLAG_VERT_TRANS = 1 << 4
ANIM_FLAG_6 = 1 << 6

# How the root joint's three translation pairs are treated.
TRANSLATION_FULL = "full"
TRANSLATION_LATERAL = "lateral"     # x and z only
TRANSLATION_VERTICAL = "vertical"   # y only
TRANSLATION_NONE = "none"


def _numbers(text, signed):
    text = COMMENT_RE.sub("", text)
    out = []
    for token in text.replace("\n", " ").split(","):
        token = token.strip()
        if not token:
            continue
        try:
            value = int(token, 0)
        except ValueError:
            continue
        if signed:
            value = ((value + 0x8000) & 0xFFFF) - 0x8000
        else:
            value &= 0xFFFF
        out.append(value)
    return out


class Animation:
    def __init__(self, name, flags, y_trans_divisor, start_frame, loop_start,
                 loop_end, values, indices):
        self.name = name
        self.flags = flags
        self.y_trans_divisor = y_trans_divisor
        self.start_frame = start_frame
        self.loop_start = loop_start
        self.loop_end = loop_end
        self.values = values
        self.indices = indices

    @property
    def frame_count(self):
        return max(1, self.loop_end)

    @property
    def num_parts(self):
        """Joint count implied by the index table (minus the root translation)."""
        return len(self.indices) // 6 - 1

    @property
    def translation_mode(self):
        if self.flags & ANIM_FLAG_HOR_TRANS:
            return TRANSLATION_VERTICAL
        if self.flags & ANIM_FLAG_VERT_TRANS:
            return TRANSLATION_LATERAL
        if self.flags & ANIM_FLAG_6:
            return TRANSLATION_NONE
        return TRANSLATION_FULL

    def _value(self, frame, pair):
        """Read one component through an (count, offset) index pair."""
        count = self.indices[pair * 2]
        offset = self.indices[pair * 2 + 1]
        index = offset + (frame if frame < count else count - 1)
        return self.values[index] if 0 <= index < len(self.values) else 0

    def sample(self, frame, num_joints):
        """Return (root_translation, [(rx, ry, rz)] per joint) for one frame.

        Rotations are binary angles; translation is in game units.
        """
        mode = self.translation_mode
        translation = [0.0, 0.0, 0.0]

        if mode == TRANSLATION_FULL:
            translation = [float(self._value(frame, i)) for i in range(3)]
        elif mode == TRANSLATION_LATERAL:
            translation[0] = float(self._value(frame, 0))
            translation[2] = float(self._value(frame, 2))
        elif mode == TRANSLATION_VERTICAL:
            translation[1] = float(self._value(frame, 1))

        rotations = []
        for joint in range(num_joints):
            base = 3 + joint * 3
            rotations.append(tuple(self._value(frame, base + i) for i in range(3)))

        return translation, rotations


def parse_animation_file(path):
    """Parse one anim_XX.inc.c into {name: Animation}."""
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        source = fh.read()

    tables = {}
    for kind, name, body in ARRAY_RE.findall(source):
        tables[name] = _numbers(body, signed=(kind == "s16"))

    animations = {}
    for name, body in ANIM_HEADER_RE.findall(source):
        fields = [f.strip() for f in COMMENT_RE.sub("", body).split(",")]
        fields = [f for f in fields if f]
        if len(fields) < 8:
            continue

        def as_int(token, default=0):
            try:
                return int(token, 0)
            except ValueError:
                return default

        values_symbol = fields[6]
        indices_symbol = fields[7]
        if values_symbol not in tables or indices_symbol not in tables:
            continue

        animations[name] = Animation(
            name=name,
            flags=as_int(fields[0]),
            y_trans_divisor=as_int(fields[1]),
            start_frame=as_int(fields[2]),
            loop_start=as_int(fields[3]),
            loop_end=as_int(fields[4]),
            values=tables[values_symbol],
            indices=tables[indices_symbol],
        )

    return animations


def load_animations(anim_dir):
    """Parse every animation in a decomp `assets/anims` directory."""
    animations = {}
    for name in sorted(os.listdir(anim_dir)):
        if not name.endswith(".inc.c") or name.startswith("table"):
            continue
        animations.update(parse_animation_file(os.path.join(anim_dir, name)))
    return animations


ANIM_ID_RE = re.compile(r"#define\s+(MARIO_ANIM_\w+)\s+(\d+)")


def load_animation_names(header_path):
    """Map animation index -> MARIO_ANIM_* name, for readable output."""
    if not os.path.exists(header_path):
        return {}
    with open(header_path, "r", encoding="utf-8", errors="replace") as fh:
        return {int(value): name for name, value in ANIM_ID_RE.findall(fh.read())}
