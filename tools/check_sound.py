"""Diagnose audio, one layer at a time, and play the samples out loud.

Run this when the game is silent. It separates the three things that can be
wrong -- no audio device, samples that failed to load, or samples that load but
never get played -- and then actually plays them so you can hear which.

Usage:
    python3 tools/check_sound.py
    python3 tools/check_sound.py --quiet     # report only, play nothing
"""

import argparse
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))

SOUNDS = os.path.join(HERE, "..", "assets", "sounds")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quiet", action="store_true",
                        help="report without playing anything")
    parser.add_argument("--directory", default=SOUNDS)
    args = parser.parse_args(argv[1:])

    directory = os.path.abspath(args.directory)

    from panda3d.core import ConfigVariableString, Filename, loadPrcFileData
    loadPrcFileData("", "window-type none")
    from direct.showbase.ShowBase import ShowBase

    from sm64py import audio
    from sm64py.mario import constants as C

    print("== 1. samples on disk ==")
    if not os.path.isdir(directory):
        print(f"   MISSING: {directory}")
        print("   Run: python3 tools/import_sounds.py")
        return 1
    waves = sorted(f for f in os.listdir(directory) if f.endswith(".wav"))
    source = audio.imported_from(directory)
    print(f"   {len(waves)} .wav files in {directory}")
    print(f"   source: {source or 'synthesised placeholders'}")
    if not waves:
        print("   Run: python3 tools/import_sounds.py")
        return 1

    print("\n== 2. audio device ==")
    base = ShowBase()
    print(f"   audio-library-name: "
          f"{ConfigVariableString('audio-library-name', '').get_value()}")
    print(f"   sfx managers      : {len(base.sfxManagerList or [])}")
    if not base.sfxManagerList:
        print("   No audio manager at all -- Panda3D was built or configured "
              "without audio.")
        return 1
    manager = base.sfxManagerList[0]
    print(f"   valid             : {manager.is_valid()}")
    print(f"   active            : {manager.get_active()}")
    print(f"   volume            : {manager.get_volume()}")
    if not manager.is_valid():
        print("   The device did not open. Under WSL check that PULSE_SERVER "
              "is set and pactl works.")
        return 1

    print("\n== 3. loading through the game's own SoundBank ==")
    bank = audio.SoundBank(directory, manager)
    loaded, failed = [], []
    for sound_id in audio.USED_SOUNDS:
        terrains = range(8) if sound_id in audio._TERRAIN_SOUNDS else (0,)
        for terrain in terrains:
            resolved = audio.resolve(sound_id, terrain)
            sound = bank._sound_for(resolved)
            (loaded if sound is not None else failed).append(resolved)
    print(f"   loaded {len(loaded)}, failed {len(failed)}")
    if failed:
        for resolved in failed[:5]:
            print(f"     missing {audio.sound_key(resolved)}.wav")
        return 1

    print("\n== 4. does the mixer actually advance? ==")
    probe = max((bank._sounds[r] for r in loaded), key=lambda s: s.length())
    probe.set_volume(1.0)
    probe.play()
    start = time.time()
    for _ in range(5):
        base.task_mgr.step()
        time.sleep(0.08)
    elapsed, played = time.time() - start, probe.get_time()
    print(f"   wall clock {elapsed:.2f}s, playback advanced {played:.2f}s")
    if played <= 0.0:
        print("   Playback never advanced: the device accepted the sound but "
              "is not mixing it.")
        return 1
    probe.stop()

    if args.quiet:
        print("\nEverything checks out. Re-run without --quiet to hear them.")
        return 0

    print("\n== 5. playing every sound the game uses ==")
    print("   If you hear nothing here, the problem is your audio output,")
    print("   not the game.\n")
    names = {v: k for k, v in vars(C).items() if k.startswith("SOUND_")}
    for sound_id in audio.USED_SOUNDS:
        terrains = (C.SOUND_TERRAIN_GRASS,) \
            if sound_id in audio._TERRAIN_SOUNDS else (0,)
        for terrain in terrains:
            sound = bank._sounds[audio.resolve(sound_id, terrain)]
            sound.set_volume(1.0)
            print(f"   {names.get(sound_id, hex(sound_id)):38} "
                  f"{sound.length():.2f}s")
            sound.play()
            deadline = time.time() + max(sound.length(), 0.25) + 0.25
            while time.time() < deadline:
                base.task_mgr.step()
                time.sleep(0.01)

    print("\nDone. All samples played.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
