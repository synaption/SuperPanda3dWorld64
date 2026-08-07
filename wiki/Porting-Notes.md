# Notes on the port

**Timing.** Game logic runs at a fixed 30 Hz, because every movement constant in
the action code is per-frame at that rate. Rendering is decoupled and the
simulation is stepped in whole ticks.

Because of that, the drawn transform is *interpolated* between the last two
ticks using whatever time is left in the accumulator. Without it, on a 144 Hz
display over 90% of rendered frames land between ticks and redraw Mario at the
same spot, so he freezes for four frames and then jumps — the motion is correct
but looks like a stutter. The camera follows the interpolated position for the
same reason, and its smoothing uses exponential decay (`1 - exp(-rate * dt)`)
rather than `rate * dt`, which would otherwise change the settling time with the
frame rate and pass `dt` jitter straight through to the view.

**Angles** are 16-bit binary angles (`0x10000` to a full turn). Sine comes from a
4096-entry table indexed by the top 12 bits, reproducing the original
quantisation rather than calling `math.sin`, because that quantisation is visible
in long slides and slow turns.

**Quirks are kept deliberately.** Collision queries truncate the sample position
to `s16`, and they take the *first* triangle in list order that passes rather
than the best one. Combined with the partition's sort order — by each triangle's
*first* vertex height, which is not necessarily its highest — this reproduces
surface cucking, where a lower triangle shadows a higher one. Wall pushback
likewise tests every wall against the entry position while accumulating the
output, so overlapping walls each push by their full amount. These are not bugs
to fix; changing any of them changes how the game plays.

**Coordinates.** SM64 is Y-up with +Z toward the camera; Panda3D is Z-up with +Y
away from it. `(x, y, z) -> (x, -z, y)` maps one to the other and preserves
handedness, so yaws stay yaws and no winding needs fixing.

**Textures** come from the HD pack. The decomp's own texture arrays are generated
from a ROM at build time and are not present, but each `#include` path maps
one-to-one onto a PNG in `RENDER96-HD-TEXTURE-PACK/gfx/`, so the parser resolves
symbols through that and copies the ones the level uses into `assets/`. A
vertex's four bytes are a colour when `G_LIGHTING` is off and a normal when it
is on, so the parser records the mode per material group and the loader
interprets them accordingly.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
