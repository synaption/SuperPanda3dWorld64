"""Parse decomp geo layouts into a node tree.

A geo layout is a flat command stream where GEO_OPEN_NODE / GEO_CLOSE_NODE
express nesting: a command creates a node, and an OPEN_NODE that follows makes
subsequent nodes children of it.  GEO_BRANCH splices in another layout, which
is how Mario's hands and face attach to his body.

The tree this produces is what the exporter turns into a skeleton.
"""

import re

from parse_f3d import GEO_CMD_START_RE, _eval, _scan_commands

GEO_LAYOUT_RE = re.compile(
    r"const\s+GeoLayout\s+(\w+)\s*\[\s*\]\s*=\s*\{(.*?)\n\}\s*;", re.S
)

# Commands that place a transform in the hierarchy.
TRANSFORM_COMMANDS = {
    "GEO_ANIMATED_PART",
    "GEO_ROTATION_NODE",
    "GEO_ROTATION_NODE_WITH_DL",
    "GEO_TRANSLATE_NODE",
    "GEO_TRANSLATE_NODE_WITH_DL",
    "GEO_TRANSLATE_ROTATE",
    "GEO_TRANSLATE_ROTATE_WITH_DL",
    "GEO_SCALE",
    "GEO_SCALE_WITH_DL",
    "GEO_BILLBOARD",
    "GEO_BILLBOARD_WITH_PARAMS",
    "GEO_BILLBOARD_WITH_PARAMS_AND_DL",
    "GEO_NODE_START",
    "GEO_SWITCH_CASE",
    "GEO_DISPLAY_LIST",
    "GEO_SHADOW",
    "GEO_RENDER_RANGE",
    "GEO_HELD_OBJECT",
    "GEO_ASM",
}


class GeoNode:
    """One transform in the actor hierarchy."""

    __slots__ = ("kind", "translation", "rotation", "scale", "display_lists",
                 "children", "animated", "switch_cases", "name")

    def __init__(self, kind, name=""):
        self.kind = kind
        self.name = name
        self.translation = (0.0, 0.0, 0.0)
        self.rotation = (0, 0, 0)          # binary angles
        self.scale = 1.0
        self.display_lists = []            # (layer, symbol)
        self.children = []
        # True for GEO_ANIMATED_PART, which consume animation data in the
        # order they are visited.
        self.animated = False
        self.switch_cases = False

    def __repr__(self):
        return (f"<{self.kind} {self.name} t={self.translation} "
                f"anim={self.animated} dls={len(self.display_lists)} "
                f"kids={len(self.children)}>")

    def walk(self):
        yield self
        for child in self.children:
            yield from child.walk()


def parse_geo_layouts(paths):
    """Read every `const GeoLayout name[]` array from the given files."""
    layouts = {}
    for path in paths:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            source = fh.read()
        for name, body in GEO_LAYOUT_RE.findall(source):
            layouts[name] = _scan_commands(body, GEO_CMD_START_RE)
    return layouts


def _make_node(cmd, args):
    node = GeoNode(cmd)

    if cmd == "GEO_ANIMATED_PART":
        # (layer, x, y, z, displayList)
        node.animated = True
        node.translation = tuple(float(_eval(a)) for a in args[1:4])
        if len(args) > 4 and args[4] != "NULL":
            node.display_lists.append((args[0], args[4]))

    elif cmd in ("GEO_TRANSLATE_ROTATE", "GEO_TRANSLATE_ROTATE_WITH_DL"):
        node.translation = tuple(float(_eval(a)) for a in args[1:4])
        node.rotation = tuple(_eval(a) for a in args[4:7])
        if len(args) > 7 and args[7] != "NULL":
            node.display_lists.append((args[0], args[7]))

    elif cmd in ("GEO_TRANSLATE_NODE", "GEO_TRANSLATE_NODE_WITH_DL"):
        node.translation = tuple(float(_eval(a)) for a in args[1:4])
        if len(args) > 4 and args[4] != "NULL":
            node.display_lists.append((args[0], args[4]))

    elif cmd in ("GEO_ROTATION_NODE", "GEO_ROTATION_NODE_WITH_DL"):
        node.rotation = tuple(_eval(a) for a in args[1:4])
        if len(args) > 4 and args[4] != "NULL":
            node.display_lists.append((args[0], args[4]))

    elif cmd in ("GEO_SCALE", "GEO_SCALE_WITH_DL"):
        # Scale is 16.16 fixed point.
        node.scale = _eval(args[1]) / 65536.0
        if len(args) > 2 and args[2] != "NULL":
            node.display_lists.append((args[0], args[2]))

    elif cmd == "GEO_DISPLAY_LIST":
        node.display_lists.append((args[0], args[1]))

    elif cmd == "GEO_SWITCH_CASE":
        node.switch_cases = True
        node.name = args[1] if len(args) > 1 else ""

    elif cmd == "GEO_ASM":
        # Runtime-driven transform (head look, torso tilt, wing flap).
        # Exported as identity; the engine applies these itself.
        node.name = args[1] if len(args) > 1 else ""

    return node


def build_tree(layouts, root_name, switch_case=0, max_depth=64):
    """Expand a layout (following GEO_BRANCH) into a GeoNode tree."""
    root = GeoNode("ROOT", root_name)

    def run(commands, parent, depth):
        if depth > max_depth:
            return
        stack = [parent]
        last = None

        for cmd, args in commands:
            if cmd == "GEO_OPEN_NODE":
                # Children attach to the most recently created node.
                stack.append(last if last is not None else stack[-1])
                last = None
                continue

            if cmd == "GEO_CLOSE_NODE":
                if len(stack) > 1:
                    last = stack.pop()
                continue

            if cmd in ("GEO_RETURN", "GEO_END"):
                break

            if cmd == "GEO_BRANCH":
                # Splice the target layout in at this point.
                target = args[-1]
                if target in layouts:
                    run(layouts[target], stack[-1], depth + 1)
                continue

            if cmd not in TRANSFORM_COMMANDS:
                continue

            node = _make_node(cmd, args)
            stack[-1].children.append(node)
            last = node

    run(layouts.get(root_name, []), root, 0)
    _resolve_switches(root, switch_case)
    return root


def _resolve_switches(node, switch_case):
    """Keep only the selected case under each switch.

    At runtime a switch processes exactly one child -- the sibling walk is
    disabled when the parent is a switch -- so keeping them all would stack
    every eye and hand variant on top of each other.
    """
    for child in node.children:
        _resolve_switches(child, switch_case)

    if node.switch_cases and node.children:
        index = min(switch_case, len(node.children) - 1)
        node.children = [node.children[index]]


def animated_parts(root):
    """Animated parts in visit order -- the order animation data is consumed."""
    return [n for n in root.walk() if n.animated]
