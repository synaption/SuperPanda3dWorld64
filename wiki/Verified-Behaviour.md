# Verified behaviour

`python3 tools/check_movement.py` measures all of this and prints it. It is a
script rather than prose because these numbers were prose, and one of them had
quietly stopped being true:

```
walking on flat ground
  settles into a sawtooth between 31.01 and 32.35, mean 31.68
  reaches it after 37 ticks

standing jump
  A held      252.0 units
  A released  106.5 units

jump chain (running)
  jump 1      102.6 units
  jump 2      140.6 units
  jump 3      630.0 units

converted level
  490 vertices, 879 triangles, 2 water boxes
  floor under the spawn point: 260.0
```

**Walking does not cap at a flat 32.0**, which is what this said for a long
time. `update_walking_speed` approaches its target and then decrements past it,
so the settled speed oscillates between 31.01 and 32.35 around the 32.0 target.
That sawtooth is the original's behaviour, not drift in the port — but "caps at
exactly 32.0" was wrong, and nothing was checking.

Releasing A during the ascent cuts the rise to well under half. The triple jump
only triggers above forward speed 20, which is why jump 3 clears 630 units while
the two below it stay near 100.

The measurements run on a synthetic flat floor rather than on the level, so
terrain cannot confuse them. Two things about that floor fail quietly if you get
them wrong, and both did while writing the script: wound the wrong way every
triangle is a ceiling and Mario drops through a floor that looks perfectly fine
in the data; made wider than ±32768 it wraps, because collision samples truncate
to `s16` (see [Notes on the port](Porting-Notes)).

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
