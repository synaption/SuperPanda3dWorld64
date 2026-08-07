# Performance

Measured with an offscreen buffer, so the numbers below are CPU-side. The
machine this was profiled on falls back to Mesa's `llvmpipe` software
rasteriser, which means GPU-side timing could not be measured at all — the
large frame spikes that remain in the profile are software-rendering
artifacts, not something the port is doing.

The game logic is not the bottleneck and never was: simulation, camera,
animation and HUD together run at a 0.29 ms median, 1.50 ms at p99.

Four things were found and fixed.

**The camera moved in steps, not sweeps.** Its yaw was stored as a whole
16-bit binary angle and converted to a direction through SM64's 4096-entry
sine table. That table resolves 0.088°, worth 2.3 units at the camera's
1500-unit orbit radius, and `s16()` truncated any easing step below one unit
to zero. Easing a 30° pan at 60 fps moved the camera on **53 of 239 frames**
and then stopped 8 units short of the target permanently — it stalled and
jumped rather than sweeping. The camera now carries its yaw as a float and
uses real trig (`sins_f`/`coss_f`); stalled frames drop to 50 of 239, matching
an ideal float reference, and the pan arrives at 29.998° of 30.

This is the one fix that is *not* Panda3D-specific — `FollowCamera` is shared,
so the ModernGL front end had the same stepping.

Gameplay still sees a quantised angle: `mario_yaw` rounds to a whole binary
unit before the movement code reads it. The sine table stays in use everywhere
it affects simulation, which is the whole point of the port. It is only
bypassed for placing the render camera.

**Ten of 26 textures were uploaded mid-gameplay.** Panda3D prepares a texture
the first frame it is actually drawn, so scenery that came into view as the
camera swung paid its upload as a dropped frame. `preload()` now calls
`prepare_scene` once at startup; all 26 are resident before frame 1, for 0.2 ms.

**The HUD rebuilt its glyph geometry every frame.** Assigning to an
`OnscreenText` only marks it dirty — the text is regenerated during the
following cull traversal, so the cost lands in the render, not in the call.
It measured 0.58 ms per frame with spikes to 25 ms. It now refreshes at 10 Hz,
which is 1.03 → 0.45 ms median and 12.81 → 2.92 ms worst case.

**The level drew as 45 nodes.** One node per material group is how the loader
keeps render state separate, but groups sharing a state can be merged, and the
level never moves. `flatten_strong()` takes it to 24. Small at this scale (785
triangles) — worth doing, not worth expecting much from.

Mario's textures also arrived from the `.glb` without mipmaps, at 512×512 drawn
over a few dozen pixels, so the detail crawled as he moved. `use_mipmaps()`
gives them the filtering the level geometry already had. That is image quality
rather than frame time.

Two things were suspected, measured, and ruled out — recorded so they don't get
re-investigated:

- **Skinning is not a cost.** SM64 actors are rigidly segmented, so each geom
  binds to exactly one joint with no blend weights. The actor costs 0.13 ms.
  `hardware-animated-vertices` changes nothing, because there is nothing to
  blend.
- **Clips are not bound lazily.** The glTF loader binds all 209 up front; a
  clip's first play costs 0.002 ms. No pre-binding pass is needed.

## Vsync is off by default

This is what actually cleared up the microstutters, and it turned out not to be
a Panda3D overhead problem at all — the two front ends were simply not
configured the same way. `modernGL/main.py` free-runs with `clock.tick(240)` and
never requests vsync, while Panda3D defaults it on. Under vsync a frame that
overruns its refresh interval waits for the whole next one, which is a visible
hitch; free-running just shows that frame late and carries on.

So both Panda3D front ends now set `sync-video 0`. To put it back:

```sh
MARIO_VSYNC=1 python3 app/main.py
```

The tradeoff is tearing, and an uncapped frame rate that will spin the GPU as
fast as it can. If that becomes a problem, cap it rather than re-enabling vsync:

```sh
python3 app/main.py    # with: clock-mode limited / clock-frame-rate 120
```

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
