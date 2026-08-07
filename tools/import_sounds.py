"""Copy the sound samples this port needs out of an extracted asset tree.

The decomp itself ships no audio -- its samples come from a ROM at build time,
exactly as its textures do. sm64pcbuilder2 extracts them, and this pulls the
handful the port actually plays into assets/sounds/, converting AIFF to WAV on
the way because that is what Panda3D's audio loader wants.

Only the files named below are copied. Nothing is redistributed: the source
tree stays where it is and the output lands in assets/, which is gitignored.

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
sys.path.insert(0, os.path.join(HERE, ".."))

from sm64py import audio  # noqa: E402
from sm64py.mario import constants as C  # noqa: E402

DEFAULT_SOURCE = os.path.join(
    HERE, "..", "reference", "sm64pcbuilder2", "assets", "US", "sound", "samples")
DEFAULT_OUTPUT = os.path.join(HERE, "..", "assets", "sounds")

# Terrain code -> sample. Mapped by name rather than by index on purpose: the
# sample bank is not ordered by terrain code. Its file 02 is stone, while
# terrain code 2 is water, and it carries a metal step that no terrain code
# selects. Lining the two up numerically would put the wrong sound underfoot
# on four of the eight surfaces.
TERRAIN_SAMPLES = {
    C.SOUND_TERRAIN_DEFAULT: "sfx_terrain/00_step_default",
    C.SOUND_TERRAIN_GRASS: "sfx_terrain/01_step_grass",
    # SM64 has no water *step* sample; stepping in shallow water splashes.
    C.SOUND_TERRAIN_WATER: "sfx_water/01_splash",
    C.SOUND_TERRAIN_STONE: "sfx_terrain/02_step_stone",
    C.SOUND_TERRAIN_SPOOKY: "sfx_terrain/03_step_spooky",
    C.SOUND_TERRAIN_SNOW: "sfx_terrain/04_step_snow",
    C.SOUND_TERRAIN_ICE: "sfx_terrain/05_step_ice",
    C.SOUND_TERRAIN_SAND: "sfx_terrain/07_step_sand",
}

# Non-terrain sounds. The voice files are named by the same hex byte that
# appears in the sound id, which is how the mario sound bank is indexed.
DIRECT_SAMPLES = {
    C.SOUND_ACTION_SWIM: "sfx_water/02_swim",
    C.SOUND_ACTION_SWIM_FAST: "sfx_water/02_swim",
    C.SOUND_ACTION_WATER_PLUNGE: "sfx_water/00_plunge",
    C.SOUND_ACTION_SURFACE_BREAK: "sfx_water/01_splash",
    C.SOUND_MARIO_YAH_WAH_HOO: "sfx_mario/00",
    C.SOUND_MARIO_HOOHOO: "sfx_mario/03",
    C.SOUND_MARIO_YAHOO: "sfx_mario/04",
    C.SOUND_MARIO_OOOF: "sfx_mario/05",
    C.SOUND_MARIO_HAHA: "sfx_mario/11",
}

# Every terrain-dependent action draws on the same footfall samples; the
# original varies them by envelope and pitch rather than by sample.
TERRAIN_ACTIONS = (
    C.SOUND_ACTION_TERRAIN_JUMP,
    C.SOUND_ACTION_TERRAIN_LANDING,
    C.SOUND_ACTION_TERRAIN_STEP,
    C.SOUND_ACTION_TERRAIN_STEP_TIPTOE,
    C.SOUND_ACTION_TERRAIN_BODY_HIT_GROUND,
    C.SOUND_ACTION_TERRAIN_HEAVY_LANDING,
)


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
    """(resolved sound id -> sample name) for everything the port can play."""
    wanted = {}
    for sound_id in TERRAIN_ACTIONS:
        for terrain, sample in TERRAIN_SAMPLES.items():
            wanted[audio.resolve(sound_id, terrain)] = sample
    for sound_id, sample in DIRECT_SAMPLES.items():
        wanted[audio.resolve(sound_id, C.SOUND_TERRAIN_DEFAULT)] = sample
    return wanted


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
    for resolved, sample in sorted(wanted.items()):
        src = os.path.join(source, sample + ".aiff")
        if not os.path.exists(src):
            missing.append(sample)
            continue
        dst = os.path.join(output, audio.sound_key(resolved) + ".wav")
        seconds += convert(src, dst)
        written += 1

    # Record where these came from, so the game can say whether it is playing
    # real samples or the synthesised stand-ins rather than guessing.
    with open(os.path.join(output, audio.SOURCE_MARKER), "w",
              encoding="utf-8") as fh:
        fh.write(source + "\n")

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
