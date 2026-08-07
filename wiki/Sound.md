# Sound

The decomp names every noise Mario makes -- 467 packed sound IDs, with the
terrain folded into the low bits so one constant covers grass, sand, snow,
stone and water -- but ships no audio. Its samples come out of a ROM at build
time, exactly as the textures do. `sm64pcbuilder2` extracts them, so:

```bash
python3 tools/import_sounds.py
```

pulls the 15 samples the port actually plays out of
`reference/sm64pcbuilder2/assets/US/sound/samples/` and converts them to WAV in
`assets/sounds/` -- 57 files once the terrain variants are expanded, which is
exactly what the action code can raise, with nothing spare. Those are committed
(see [Assets](Assets)), so this step is only needed to regenerate them. Without any
samples at all the game synthesises crude stand-ins and says so at startup, so
the two are never confused:

```
Audio: AudioManager ready, 57 samples
       imported from .../US/sound/samples
```

If the game is silent, `python3 tools/check_sound.py` separates the three
things that can be wrong -- no audio device, samples that failed to load, or
samples that load but never play -- and then plays them all out loud, so a
silent run there points at your audio output rather than at the game.

**Sample paths are converted, not passed raw.** Panda3D's loaders take its own
path syntax rather than the platform's, and the difference only shows on
Windows: a native `C:\...\assets\sounds\x.wav` is read as a *relative* path,
the model path is searched in vain, and the loader hands back a silent sound
instead of failing. On Linux the raw path is already in the right form, so the
bug is invisible there -- which is how it survived being tested. A sound that
loads with zero length is now treated as missing and reported once.

**Actions never play anything.** They append real IDs to
`MarioState.sound_events` and the front end drains it once a tick, so the
simulation runs identically with no audio device attached -- the normal case
under WSL.

**The sample bank is not ordered by terrain code.** Its file `02` is stone
while terrain code 2 is water, and it carries a metal step that no terrain code
selects, so the import maps by name. Lining the two up numerically would have
put the wrong sound underfoot on four of the eight surfaces. SM64 has no water
*step* sample at all -- stepping in shallow water uses the splash.

**Footsteps come from the animation, not from distance travelled.** They fire
on the two frames of each cycle a foot lands on -- 10 and 49 walking, 9 and 45
running, and so on -- which are the original's own numbers. Because the clip is
played back at speed/4, the cadence then follows Mario's speed with no constant
to tune: 3.1 steps/sec walking, 6.7 running, measured against 3.12 and 6.67
predicted from the clip lengths.

Driving them from distance instead was an approximation, and a bad one. It
fired every 52 units, which at a running 960 units/sec is 18 footfalls a second
-- nearly three times too fast. The simulation tracks its own animation frame
for this rather than asking the renderer, so footfalls stay in step whether or
not anything is being drawn.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
