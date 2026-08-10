"""Print an uncooked Unreal blueprint as a readable node listing.

This is how every constant in `ow/constants.py` and every body in
`ow/level.py` was derived -- read out of the binary rather than eyeballed, so
they can be re-derived if the Unreal project changes.

    python -m ow.tools.dump_blueprint <file.uasset> [class-or-name filter]

Useful targets, all under
reference/OuterWildsPlayerControlle/Content/OuterWildsPlayerController/
OuterWildsPlayerController/:

    Blueprints/Data/DA_CharacterVariables.uasset   the tuning values
    Blueprints/AC_GravityComponent.uasset          gravity, GravityConstant
    Blueprints/AC_SpaceMovementComponent.uasset    jetpack, camera lag
    Blueprints/BFL_ZeroGFunctions.uasset           shared helper functions
    Levels/L_DemoLevel.umap                        planet placements
"""

import re
import sys

from .uasset import Package, props54


def _render(pkg, tag, depth, out):
    pad = "    " * depth
    base = tag["type"].split("<")[0]
    if base == "StructProperty" and depth < 4:
        nested = props54(pkg, tag["off"], tag["off"] + tag["size"])
        if nested:
            out.append("{}{} ({}):".format(pad, tag["name"], tag["type"]))
            for child in nested:
                _render(pkg, child, depth + 1, out)
            return
    value = tag["val"]
    if isinstance(value, float):
        value = round(value, 6)
    out.append("{}{} = {}   [{}]".format(pad, tag["name"], value, tag["type"]))


def dump(path, keep=None):
    pkg = Package(path)
    print("########## {}".format(path.split("/")[-1]))
    for export in pkg.exports():
        cls = pkg.classof(export) or ""
        if keep and keep not in cls and keep not in export["name"]:
            continue
        print("\n=== [{}] {} :: {}  (@{} {}B)".format(
            export["idx"], cls, export["name"], export["off"], export["size"]))

        tags = props54(pkg, export["off"], export["off"] + export["size"])
        if not tags:
            # Some exports carry a leading byte before the property stream.
            tags = props54(pkg, export["off"] + 1, export["off"] + export["size"])
        lines = []
        for tag in tags:
            _render(pkg, tag, 1, lines)
        print("\n".join(lines))

        # Pins serialise after the tagged properties in a custom blob. It is
        # not worth a full decoder -- the readable names in it are enough to
        # tell what a node is wired to.
        tail_start = max([t["off"] + t["size"] for t in tags], default=export["off"])
        tail = pkg.d[tail_start:export["off"] + export["size"]]
        strings = re.findall(rb"[ -~]{4,}", tail)
        if strings:
            text = b" | ".join(strings[:40]).decode("ascii", "replace")
            print("    strings: " + text[:1200])


def main(argv=None):
    argv = argv if argv is not None else sys.argv[1:]
    if not argv:
        print(__doc__)
        return 1
    dump(argv[0], argv[1] if len(argv) > 1 else None)
    return 0


if __name__ == "__main__":
    sys.exit(main())
