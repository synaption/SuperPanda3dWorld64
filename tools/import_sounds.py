"""Copy the sound samples this game needs out of an extracted asset tree.

The decomp itself ships no audio -- its samples come from a ROM at build time,
exactly as its textures do. sm64pcbuilder2 extracts them, and this pulls the
handful the game actually plays into assets/sounds/mario64/, converting AIFF to
WAV on the way because WAV is what Bevy's audio decoder reads.

Only the files named below are copied. Nothing is redistributed: the source
tree stays where it is and the output lands in assets/, which is tracked -- see
the asset notice in the README before publishing the repository.

The output names are the ones `SAMPLES` in `src/audio.rs` asks for, spelled out
here rather than derived from SM64 sound ids. The ids were how the Panda3D
build addressed its sound bank; the Rust tables name files directly, so the id
arithmetic in between had nothing left to connect and is gone.

Usage:
    python3 tools/import_sounds.py
    python3 tools/import_sounds.py --source <path to .../sound/samples>
"""

import argparse
import os
import struct
import sys
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(HERE, ".."))

DEFAULT_SOURCE = os.path.join(
    PROJECT_ROOT, "reference", "sm64pcbuilder2", "assets", "US", "sound", "samples")
DEFAULT_OUTPUT = os.path.join(PROJECT_ROOT, "assets", "sounds", "mario64")

# Terrain -> source sample. Mapped by name rather than by index on purpose: the
# sample bank is not ordered by SM64's terrain codes. Its file 02 is stone,
# while terrain code 2 is water, and it carries a metal step that no terrain
# code selects. Lining the two up numerically would put the wrong sound
# underfoot on four of the eight surfaces.
TERRAINS = {
    "default": "sfx_terrain/00_step_default",
    "grass": "sfx_terrain/01_step_grass",
    # SM64 has no water *step* sample; stepping in shallow water splashes.
    "water": "sfx_water/01_splash",
    "stone": "sfx_terrain/02_step_stone",
    "spooky": "sfx_terrain/03_step_spooky",
    "snow": "sfx_terrain/04_step_snow",
    "ice": "sfx_terrain/05_step_ice",
    "sand": "sfx_terrain/07_step_sand",
}

# Every terrain-dependent action draws on the same footfall samples; the
# original varies them by envelope and pitch rather than by sample. Each name
# here is written out once per terrain, as `<action>_<terrain>.wav`.
TERRAIN_ACTIONS = (
    "jump",
    "landing",
    "step",
    "tiptoe_step",
    "body_hit_ground",
    "heavy_landing",
)

# Output stem -> source sample, for the sounds that do not vary with terrain.
# The voice files are named by the same hex byte that indexed them in the
# mario sound bank.
DIRECT = {
    "swim_stroke": "sfx_water/02_swim",
    "fast_swim_stroke": "sfx_water/02_swim",
    "water_plunge": "sfx_water/00_plunge",
    "surface_splash": "sfx_water/01_splash",
    "mario_yah_wah_hoo": "sfx_mario/00",
    "mario_hoohoo": "sfx_mario/03",
    "mario_yahoo": "sfx_mario/04",
    "mario_ooof": "sfx_mario/05",
    "mario_haha": "sfx_mario/11",
}

# Where the samples came from, so it is possible to tell an imported set from a
# hand-assembled one without diffing audio.
SOURCE_MARKER = ".source"


def _extended80(raw):
    """Decode the 80-bit IEEE extended float AIFF stores its sample rate in."""
    exponent = struct.unpack(">H", raw[:2])[0]
    mantissa = struct.unpack(">Q", raw[2:10])[0]
    sign = -1 if exponent & 0x8000 else 1
    exponent &= 0x7FFF
    if exponent == 0 and mantissa == 0:
        return 0.0
    return sign * mantissa * 2.0 ** (exponent - 16383 - 63)


def read_aiff(path):
    """Minimal AIFF reader: (channels, width, rate, big-endian frames).

    Written out rather than using the stdlib's aifc, which is deprecated and
    removed in Python 3.13 -- this tool should outlive that.
    """
    with open(path, "rb") as fh:
        data = fh.read()

    if data[:4] != b"FORM" or data[8:12] != b"AIFF":
        raise ValueError(f"{path}: not an AIFF file")

    channels = width = rate = None
    frames = None
    offset = 12
    while offset + 8 <= len(data):
        name = data[offset:offset + 4]
        size = struct.unpack(">I", data[offset + 4:offset + 8])[0]
        body = data[offset + 8:offset + 8 + size]

        if name == b"COMM":
            channels, _, bits = struct.unpack(">HIH", body[:8])
            width = bits // 8
            rate = int(_extended80(body[8:18]))
        elif name == b"SSND":
            start = struct.unpack(">I", body[:4])[0]
            frames = body[8 + start:]

        # Chunks are padded to an even length.
        offset += 8 + size + (size & 1)

    if None in (channels, width, rate) or frames is None:
        raise ValueError(f"{path}: missing COMM or SSND chunk")
    return channels, width, rate, frames


def convert(src, dst):
    """AIFF -> WAV. Sample data is identical apart from byte order."""
    channels, width, rate, frames = read_aiff(src)
    if width != 2:
        raise ValueError(f"{src}: expected 16-bit samples, got {width * 8}-bit")

    # AIFF is big-endian, WAV little-endian.
    count = len(frames) // 2
    swapped = struct.pack(f"<{count}h", *struct.unpack(f">{count}h", frames[:count * 2]))

    with wave.open(dst, "wb") as out:
        out.setnchannels(channels)
        out.setsampwidth(width)
        out.setframerate(rate)
        out.writeframes(swapped)
    return count / float(rate * channels)


def wanted_samples():
    """Output stem -> source sample, for everything the game can play."""
    wanted = dict(DIRECT)
    for action in TERRAIN_ACTIONS:
        for terrain, sample in TERRAINS.items():
            wanted[f"{action}_{terrain}"] = sample
    return wanted


def _as_project_relative(path):
    relative = os.path.relpath(os.path.abspath(path), PROJECT_ROOT)
    return path if relative.startswith(os.pardir) else relative.replace(os.sep, "/")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", default=DEFAULT_SOURCE,
                        help="extracted sound/samples directory")
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    args = parser.parse_args(argv[1:])

    source = os.path.abspath(args.source)
    output = os.path.abspath(args.output)
    if not os.path.isdir(source):
        print(f"No sample directory at {source}")
        print("Point --source at an extracted .../sound/samples directory.")
        return 1

    os.makedirs(output, exist_ok=True)
    wanted = wanted_samples()

    written = 0
    missing = []
    seconds = 0.0
    for stem, sample in sorted(wanted.items()):
        src = os.path.join(source, sample + ".aiff")
        if not os.path.exists(src):
            missing.append(sample)
            continue
        seconds += convert(src, os.path.join(output, stem + ".wav"))
        written += 1

    # Written relative to the project where possible: this file is tracked, and
    # an absolute path would commit one machine's home directory.
    with open(os.path.join(output, SOURCE_MARKER), "w", encoding="utf-8") as fh:
        fh.write(_as_project_relative(source) + "\n")

    unique = sorted(set(wanted.values()))
    print(f"{written} samples written to {output}")
    print(f"  from {len(unique)} distinct source files, {seconds:.1f}s of audio")
    if missing:
        print(f"  {len(set(missing))} not found in the source tree:")
        for name in sorted(set(missing)):
            print(f"    {name}.aiff")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
