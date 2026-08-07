# The asset workbench

`tools/workbench.py` draws one asset, alone, against a key-coloured background,
with any part of it hideable. That isolation is the whole point: every wrong
conclusion drawn about the models in this project came from measuring something
that was not isolated. Counting how many pixels a scuttlebug covered to check
its [billboards](Billboards) measured mostly leg geometry, which swings with the viewing
angle for reasons that have nothing to do with billboards -- the numbers looked
fine whether the fix worked or not.

```bash
# look at it
python3 tools/workbench.py assets/actors/scuttlebug.glb

# measure it, with no screen -- for CI, or for an agent
python3 tools/workbench.py assets/actors/goomba.glb --headless --orbit 8 --json

# measure one part of it
python3 tools/workbench.py assets/actors/scuttlebug.glb \
    --headless --isolate 'billboard_' --orbit 8

# gate a build: non-zero exit if any check fails
python3 tools/workbench.py assets/actors/goomba.glb --orbit 8 --frame 0 \
    --billboard --expect
```

Note the `--frame 0`: an SM64 actor unposed is in its bind pose, which is not a
pose at all, and both checks are meaningless there.

Interactive mode orbits, tilts, cycles clips, toggles wireframe, and prints a
measurement on demand. `--list-parts` shows what `--isolate` and `--hide` can
match; `--compare` sizes an asset against another (usually Mario).

**Isolating a skinned actor is not hiding nodes.** All its parts live in one
GeomNode, so hiding by node name hides everything or nothing -- collapsing the
*joint* is what removes its vertices. And it has to keep the ancestors of a
match alive, because collapsing a joint takes its whole subtree with it: the
first version flattened the parents of the very parts it was asked to show and
measured an empty screen.

**Interactive keys.** Arrows orbit and tilt; `space` pauses the spin and
`[` `]` change its speed; `0`-`9` jump straight to a clip and `n`/`p` step
through them; `b` turns on cross-fading so switching clips shows the
*transition* rather than a cut; `w` wireframe, `t` two-sided, `m` prints a
measurement.

**Billboard settings can be adjusted here and saved for the game to read.**
`,` `.` select a setting and `-` `=` change it, with the current values shown on
screen; `g` makes changes an override for this actor alone rather than global;
`k` measures how well the billboards are tracking right now; `y` sweeps the
settings and applies whatever measures best; `s` `l` `r` save, reload and reset;
`d` prints what the geometry says about them. The same things from the command
line:

```bash
# what the geometry says: parents, extents, the rotation that needs cancelling
python3 tools/workbench.py assets/actors/goomba.glb --probe --frame 0

# try a setting without committing to it
python3 tools/workbench.py assets/actors/goomba.glb --billboard \
    --isolate 'billboard_4$' --set cancel_parent=off --orbit 12 --json

# let the measurement choose, and keep it
python3 tools/workbench.py assets/actors/goomba.glb --tune --save-tuning
```

`--tune` is for telling regimes apart, not for micro-optimising: a working
setting measures about fourteen times better than a broken one, so anything
within a tenth of the best counts as the same answer and the plainest of them
wins. Without that it cheerfully picked a 135-degree pitch on two percent of
pixel-quantisation noise. Run against the goomba it now lands on all-zeroes,
which is what ships.

**Two checks ship with it.** `billboard` orbits the asset and compares its
narrowest silhouette against its widest -- something that turns to face you
holds its width, something that only pretends to collapses toward a line.
`grounded` catches an asset whose origin straddles z=0 instead of sitting on
it, which is what a rigged model in its bind pose does, and is exactly why
enemies loaded as plain geometry looked half-sunk and tipped over. Every SM64
actor fails `grounded` unposed; that is the check working, not a false alarm.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
