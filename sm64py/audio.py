"""Sound events, and playing them through Panda3D.

The decomp carries a complete sound *taxonomy* -- 467 packed IDs naming every
noise Mario makes -- but no audio. Its samples are extracted from a ROM at
build time, exactly as the textures are, and none of the sequence or bank
descriptors under ``sound/`` contain waveform data. So the event system here
is faithful and the samples are not: ``generate_placeholders`` synthesises
simple stand-ins so the plumbing is audible and testable. Point ``SoundBank``
at real files to replace them.

Actions never call into this module. They append IDs to
``MarioState.sound_events`` and the front end drains that once a frame, so the
simulation runs identically with no audio device present -- which matters,
because it often is absent under WSL.
"""

import math
import os
import struct
import wave

from .mario import constants as C

# Terrain-dependent IDs have their terrain written into the low bits of the
# sound id byte, so one constant covers grass, sand, snow, stone and water.
_TERRAIN_SOUNDS = {
    C.SOUND_ACTION_TERRAIN_JUMP,
    C.SOUND_ACTION_TERRAIN_LANDING,
    C.SOUND_ACTION_TERRAIN_STEP,
    C.SOUND_ACTION_TERRAIN_BODY_HIT_GROUND,
    C.SOUND_ACTION_TERRAIN_STEP_TIPTOE,
    C.SOUND_ACTION_TERRAIN_HEAVY_LANDING,
}

# Surface types that sound like something other than the default ground.
_TERRAIN_BY_SURFACE = {
    0x0013: C.SOUND_TERRAIN_SAND,      # SURFACE_SAND
    0x0014: C.SOUND_TERRAIN_SNOW,      # SURFACE_SNOW
    0x0015: C.SOUND_TERRAIN_ICE,       # SURFACE_ICE
    0x0007: C.SOUND_TERRAIN_WATER,     # SURFACE_SHALLOW_WATER
    0x000A: C.SOUND_TERRAIN_STONE,     # SURFACE_HARD
}


def terrain_for(mario):
    """Terrain code for whatever Mario is standing on."""
    if mario.floor is None:
        return C.SOUND_TERRAIN_DEFAULT
    return _TERRAIN_BY_SURFACE.get(int(mario.floor.type), C.SOUND_TERRAIN_GRASS)


def resolve(sound_id, terrain):
    """Fold the terrain into a terrain-dependent id; others pass through."""
    if sound_id in _TERRAIN_SOUNDS:
        return sound_id | (terrain << 16)
    return sound_id


def sound_key(sound_id):
    """Readable, stable filename stem for a resolved sound id."""
    bank = (sound_id >> 28) & 0xF
    ident = (sound_id >> 16) & 0xFF

    terrain_names = (
        "default", "grass", "water", "stone",
        "spooky", "snow", "ice", "sand",
    )
    terrain_families = (
        (0x00, "jump"),
        (0x08, "landing"),
        (0x10, "step"),
        (0x18, "body_hit_ground"),
        (0x20, "tiptoe_step"),
        (0x60, "heavy_landing"),
    )
    if bank == C.SOUND_BANK_ACTION:
        for base, action_name in terrain_families:
            terrain = ident - base
            if 0 <= terrain < len(terrain_names):
                return f"{action_name}_{terrain_names[terrain]}"

        action_names = {
            0x30: "water_plunge",
            0x31: "surface_splash",
            0x33: "swim_stroke",
            0x47: "fast_swim_stroke",
        }
        if ident in action_names:
            return action_names[ident]

    if bank == C.SOUND_BANK_VOICE:
        voice_names = {
            0x00: "mario_yah_wah_hoo",
            0x03: "mario_hoohoo",
            0x04: "mario_yahoo",
            0x05: "mario_ooof",
            0x0F: "mario_attacked",
            0x11: "mario_haha",
            0x18: "mario_panting",
        }
        if ident in voice_names:
            return voice_names[ident]

    # Keep unknown IDs usable without pretending to know what they contain.
    return f"unknown_bank_{bank}_sound_{ident:02x}"


# -- placeholder synthesis --------------------------------------------------
#
# Deliberately crude: short shaped noise for footfalls and impacts, short
# shaped tones for voice. Enough to hear that the right event fired at the
# right moment, and obviously not the real thing.

SAMPLE_RATE = 22050


def _write_wav(path, samples):
    with wave.open(path, "wb") as fh:
        fh.setnchannels(1)
        fh.setsampwidth(2)
        fh.setframerate(SAMPLE_RATE)
        fh.writeframes(b"".join(
            struct.pack("<h", max(-32767, min(32767, int(s * 32767))))
            for s in samples
        ))


def _noise(duration, decay, seed, low_pass=0.5):
    """Shaped noise, for footsteps and splashes."""
    state = seed & 0xFFFFFFFF
    n = int(SAMPLE_RATE * duration)
    out = []
    smoothed = 0.0
    for i in range(n):
        # A small xorshift keeps this dependency-free and reproducible.
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        value = (state / 0x100000000) * 2.0 - 1.0
        smoothed += (value - smoothed) * low_pass
        out.append(smoothed * math.exp(-decay * i / SAMPLE_RATE))
    return out


def _tone(duration, start_hz, end_hz, decay=6.0):
    """Swept tone, standing in for a vocal."""
    n = int(SAMPLE_RATE * duration)
    out = []
    phase = 0.0
    for i in range(n):
        t = i / n
        hz = start_hz + (end_hz - start_hz) * t
        phase += 2.0 * math.pi * hz / SAMPLE_RATE
        envelope = math.exp(-decay * i / SAMPLE_RATE)
        # A little second harmonic so it is not a pure sine.
        out.append((math.sin(phase) + 0.3 * math.sin(2 * phase)) * 0.5 * envelope)
    return out


def _placeholder_for(sound_id, terrain):
    bank = (sound_id >> 28) & 0xF
    if bank == C.SOUND_BANK_VOICE:
        return _tone(0.30, 480.0, 700.0)
    if sound_id in (C.SOUND_ACTION_SWIM, C.SOUND_ACTION_SWIM_FAST,
                    C.SOUND_ACTION_WATER_PLUNGE, C.SOUND_ACTION_SURFACE_BREAK):
        return _noise(0.35, 9.0, 0x51F0 + sound_id, low_pass=0.25)
    # Footfalls: pitched a little by terrain so they are distinguishable.
    return _noise(0.12, 34.0, 0x1234 + sound_id + terrain * 7,
                  low_pass=0.35 + 0.1 * terrain)


# Written by tools/import_sounds.py next to the samples it imports, naming the
# tree they came from. Its presence is how the game knows it is playing real
# audio rather than the synthesised stand-ins.
SOURCE_MARKER = ".source"


def imported_from(directory):
    """Where the samples in a directory came from, or None if synthesised."""
    try:
        with open(os.path.join(directory, SOURCE_MARKER), encoding="utf-8") as fh:
            return fh.read().strip() or None
    except OSError:
        return None


def generate_placeholders(directory, ids):
    """Write a stand-in .wav for each (sound id, terrain) that can occur."""
    os.makedirs(directory, exist_ok=True)
    written = []
    for sound_id in ids:
        terrains = range(8) if sound_id in _TERRAIN_SOUNDS else (0,)
        for terrain in terrains:
            resolved = resolve(sound_id, terrain)
            path = os.path.join(directory, sound_key(resolved) + ".wav")
            if not os.path.exists(path):
                _write_wav(path, _placeholder_for(sound_id, terrain))
                written.append(path)
    return written


# Every id the action code can raise, which is what needs a sample.
USED_SOUNDS = (
    C.SOUND_ACTION_TERRAIN_JUMP,
    C.SOUND_ACTION_TERRAIN_LANDING,
    C.SOUND_ACTION_TERRAIN_STEP,
    C.SOUND_ACTION_TERRAIN_STEP_TIPTOE,
    C.SOUND_ACTION_TERRAIN_BODY_HIT_GROUND,
    C.SOUND_ACTION_TERRAIN_HEAVY_LANDING,
    C.SOUND_ACTION_SWIM,
    C.SOUND_ACTION_SWIM_FAST,
    C.SOUND_ACTION_WATER_PLUNGE,
    C.SOUND_ACTION_SURFACE_BREAK,
    C.SOUND_MARIO_YAH_WAH_HOO,
    C.SOUND_MARIO_HOOHOO,
    C.SOUND_MARIO_YAHOO,
    C.SOUND_MARIO_HAHA,
    C.SOUND_MARIO_OOOF,
)


class SoundBank:
    """Loads samples and plays the events an action raised.

    Degrades quietly: with no audio device, or no samples on disk, every call
    is a no-op. That is the normal case under WSL, where Panda3D is often
    started with the null audio backend on purpose.
    """

    def __init__(self, directory, audio_manager=None, volume=0.7):
        self.directory = directory
        self.manager = audio_manager
        self.volume = volume
        self._sounds = {}
        self._missing = set()
        self.enabled = audio_manager is not None and audio_manager.is_valid()

    def _sound_for(self, resolved_id):
        if resolved_id in self._sounds:
            return self._sounds[resolved_id]
        if resolved_id in self._missing:
            return None

        path = os.path.join(self.directory, sound_key(resolved_id) + ".wav")
        sound = None
        if os.path.exists(path):
            # Converted, not passed raw. Panda3D loaders take its own path
            # syntax rather than the platform's, and the difference only shows
            # on Windows: a native path like C:\...\assets\sounds\x.wav is read
            # as a relative one, the model path is searched in vain, and the
            # loader hands back a silent sound rather than failing. On Linux
            # the raw path happens to already be in the right form, so this is
            # invisible there -- which is exactly why it survived testing.
            from panda3d.core import Filename
            sound = self.manager.get_sound(
                Filename.from_os_specific(os.path.abspath(path)))

        # A sound with no length loaded in name only; treat it as missing so
        # the failure is reported once instead of playing silence forever.
        if sound is None or sound.length() <= 0.0:
            self._missing.add(resolved_id)
            if len(self._missing) == 1:
                print(f"Audio: failed to load {path} -- running silent for "
                      f"that sound.")
            return None

        sound.set_volume(self.volume)
        self._sounds[resolved_id] = sound
        return sound

    def play_events(self, mario):
        """Drain and play whatever the actions raised this frame."""
        if not self.enabled or not mario.sound_events:
            return
        terrain = terrain_for(mario)
        for sound_id in mario.sound_events:
            sound = self._sound_for(resolve(sound_id, terrain))
            if sound is not None:
                sound.play()
