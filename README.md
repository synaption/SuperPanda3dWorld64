# mario64 in panda3d

The goal of this project is to combine different elements from different old N64
games based on their decomps in python with panda3d for research. We are going to
start with mario 64, just the castle outside and mario with his motion system
based on the render96 recomp.

Current state: the castle grounds load and render from the decomp data, and
Mario's movement — walking, slopes, jumps, the jump chain, dives, slides, ledge
grabs, wall bonks — runs on a port of the original physics.

## Running

```bash
python3 tools/parse_collision.py \
    reference/Render96ex/levels/castle_grounds/areas/1/collision.inc.c \
    assets/castle_grounds/collision.npz

python3 tools/parse_f3d.py reference/Render96ex/levels/castle_grounds 1 \
    assets/castle_grounds/mesh.npz

python3 tools/export_actor_gltf.py --actor mario --anims all \
    -o assets/mario/mario.glb

python3 app/main.py
```

The converters read from `reference/` and write to `assets/`. They only need to
be re-run when the level data changes.

Requires `panda3d` and `numpy`.

### Ursina version

An Ursina front end is also available. It shares the exact movement, collision,
level rendering, actor, fixed-timestep, and camera code with the Panda3D app:

```bash
python3 -m pip install ursina
python3 ursina/main.py
```

The converted assets and controls are the same as for `app/main.py`.

### ModernGL version

The standalone ModernGL front end uses pygame for its window and input, reads
the converted level directly, and GPU-skins Mario without Panda3D or Ursina:

```bash
python3 -m pip install moderngl pygame pillow numpy
python3 modernGL/main.py
```

The controls and converted assets are shared with the other front ends.

Building under WSL and running from Windows against the same files works: the
converters record texture paths relative to the project rather than absolute,
and paths handed to Panda3D loaders go through `Filename.from_os_specific`,
which is what turns a UNC share into the `/hosts/server/share/...` form Panda3D
expects. Passing a native path string straight to a loader makes it search the
model path and fail with "not found on model path".

### Controls

| | |
|---|---|
| `W` `A` `S` `D` / arrows | analog stick (camera-relative) |
| `Space` | A — jump |
| `Left Shift` | B — punch, dive |
| `Left Ctrl` | Z — crouch, ground pound, long jump |
| `Q` `E` / mouse drag | swing the camera |
| `R` | re-centre the camera behind Mario |
| `F3` | toggle the collision overlay |
| `F1` | toggle the debug readout |

## Layout

```
sm64py/
  math_util.py     binary-angle trig, Panda3D coordinate bridge
  surfaces.py      collision triangles, spatial partition, floor/ceil/wall queries
  level.py         converted mesh -> Panda3D geometry
  camera.py        following camera
  mario/
    constants.py   action ids, action flags, input bits, surface types
    state.py       per-frame state, controller sampling, geometry queries
    steps.py       quarter-step integration, gravity, ledge grabs
    actions.py     the action state machine
    animations.py  which animation clip each action plays
tools/
  check_anim_grounding.py  verify grounded actions keep their feet on the floor
  parse_collision.py     collision.inc.c -> npz
  parse_f3d.py           F3D display lists -> textured mesh
  geo_layout.py          geo layouts -> actor node tree
  sm64_anim.py           animation tables -> per-frame joint rotations
  glb.py                 minimal glTF 2.0 / GLB writer
  export_actor_gltf.py   actor -> rigged, animated .glb
app/main.py        the runnable game
ursina/main.py     the Ursina front end
modernGL/main.py   the ModernGL + pygame front end
```

## Notes on the port

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
symbols through that. A vertex's four bytes are a colour when `G_LIGHTING` is off
and a normal when it is on, so the parser records the mode per material group and
the loader interprets them accordingly.

## Exporting actors for Blender

`tools/export_actor_gltf.py` turns a decomp actor into a rigged, animated
`.glb`. It is dependency-free apart from numpy — the glTF is written directly.

```bash
# every animation, game units, HD textures embedded
python3 tools/export_actor_gltf.py --actor mario --anims all -o mario.glb

# metres, for a Blender scene that works at human scale
python3 tools/export_actor_gltf.py --actor mario --anims all \
    --scale 0.0025 -o mario_metres.glb

# keep everything, including the wing-cap wings
python3 tools/export_actor_gltf.py --actor mario --exclude-dl '' -o mario.glb
```

Parts the game only draws conditionally are left out by default. Mario's wings
sit under a `GEO_ASM` hook that only emits them while the wing cap is active,
so exporting them unconditionally leaves them stuck to his head at all times.

Mario comes out as 30 joints (20 of them animated), 514 vertices, 760
triangles, and 209 animations.

**Why the conversion is exact.** These actors are rigidly segmented, not
smooth-skinned: each body part is its own display list authored in its joint's
local space. Every vertex therefore binds to exactly one joint with weight 1.0,
which is precisely what the hardware did when it multiplied each part by that
joint's matrix. Nothing is approximated or re-rigged.

**Joint order is implicit.** It is not stored anywhere — it is the order
animated parts are visited walking the geo layout depth-first, and the
animation index table is read in lockstep with that walk. The exporter
cross-checks the two and warns if they disagree. They agree here: the
hierarchy yields 20 animated joints and all 209 animation tables independently
report 20 parts.

**Scale.** The actor's geo wraps the body in `GEO_SCALE(0x00, 16384)`, and
`0x10000` means 1.0 — so Mario is authored at 4x and shrunk to a quarter at
draw time. The exporter bakes that in by default, giving a ~154-unit Mario that
matches the level and collision units.

**Clips need a start frame.** Animation headers carry a start frame, and the
exporter writes it to a `*_clips.json` sidecar because glTF has nowhere to put
it. It is not cosmetic: the single-jump landing clip starts at frame 22 of 38,
and its unplayed lead-in frames are a deep crouch — play it from zero and Mario
sinks knee-deep through the floor on every landing.

`tools/check_anim_grounding.py` guards this. It poses every grounded action and
reports any whose lowest vertex sits below the floor, which is what a grounded
action pointing at an airborne clip looks like. Airborne actions are exempt, as
is the ledge grab, which hangs below its position by design.

**Rest pose is meaningless.** SM64 joints point down their own limb, so the
unposed model splays along +X. Every animation supplies rotations for all 20
joints, so it only resolves once posed — that is normal for this data, not a
broken export. In Blender, scrub any action to see him assemble.

Runtime-driven joints (`geo_mario_tilt_torso`, head look, wing flap) export as
identity, since the engine drives those rather than the animation data.

**UV origin differs by target.** The N64 puts the texture origin at the
top-left with V increasing downward, and glTF does exactly the same — so actor
UVs export unflipped. Panda3D's own texture coordinates start at the
bottom-left, so `sm64py/level.py` flips V when it builds geometry directly.
Flipping in the converter instead silently mirrors every actor texture; the
giveaway was Mario's cap logo reading as a W.

**The combiner decides how texture and shade meet.** Getting this wrong on
Mario's face produced three different failures in turn:

- `MODULATE*` multiplies texel by shade. Applying that to the others
  multiplies a yellow button by blue denim and comes out black.
- `BLEND*` lerps the texel over the *shade colour*, within the polygon, using
  the texel's own alpha. The result is **opaque**: where the texture is
  transparent the shade colour shows through, not whatever is behind the
  surface. Treating that alpha as see-through cuts holes through his face.
- `SHADE*` samples no texel at all.

Mario's eyes, mustache, cap logo and overall buttons are all `BLEND`. glTF's
`baseColorFactor` can only multiply, so "texture over a colour" cannot be
expressed directly — the exporter composites each such texture onto its light
colour and emits the flattened result as an opaque texture. That is what the
hardware produced, just baked ahead of time.

Genuine cut-out alpha still occurs elsewhere and is emitted as `MASK` with a
0.5 cutoff; RGBA5551 carries a single alpha bit, so a mask is exact there.

**Solid colours come from lights, not vertices.** Mario's shirt and overalls
carry no texture and no useful vertex colour — the colour is on the light group
bound by `gsSPLight`, so the parser reads `gdSPDefLights1` and uses its diffuse
value as the material's base colour. Texture state also persists across display
lists, so a part that draws untextured has to actively say so via
`gsSPTexture(..., G_OFF)` or a shade-only combiner; without tracking that, the
last texture bound leaks onto every solid part after it.

**Textures must not be sRGB-decoded.** Panda3D's glTF loader flags
`baseColorTexture` as sRGB, which the spec asks for — but nothing here
re-encodes, and every other texture in the project is loaded raw, so the decode
lands once and is never undone. `sm64py.level.use_linear_textures` swaps those
formats back before the Actor is parented.

The symptom is a colour *split*, not an overall shift: a material's
`baseColorFactor` is used as written while its texture is darkened. Mario's
composited face rendered orange at (253, 136, 49) beside untextured parts at
the intended (254, 193, 121) — which is exactly sRGB-to-linear applied once,
and how the bug was identified.

**Panda3D needs the mesh under the skeleton.** The skinned mesh node has to be
a *child* of the skeleton root, not a sibling. Panda3D's glTF loader builds its
Character from the joint hierarchy and only adopts geometry sitting underneath
it — as a sibling the mesh loads and renders but never binds, so the Actor
animates nothing.

The app maps 48 actions onto 33 clips (`sm64py/mario/animations.py`), including
the speed-dependent tiptoe/walk/run split and the rise/fall halves of a double
jump.

## Verified behaviour

Checked against the reference numbers, not just eyeballed:

- Walking accelerates to exactly 32.0 and caps there.
- A standing jump starts at 42.0 vertical velocity; releasing A during the ascent
  cuts the rise (242 units held vs 96 released).
- The jump chain runs single -> double -> triple with the right heights
  (68 / 88 / 561 units), and the triple only triggers above speed 20.
- Spawn floor height at the level's `MARIO_POS` resolves to exactly 260.0.
- Parsed collision matches the level header counts (490 vertices, 879 triangles).

## Performance

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

### Comparing against the ModernGL front end

The two are not configured the same way, so "ModernGL feels smoother" is partly
not a Panda3D result. `modernGL/main.py` free-runs with `clock.tick(240)` and
never requests vsync; `app/main.py` sets `sync-video 1`. Under vsync a frame
that overruns its refresh interval waits for the next one, which reads as a
stutter; free-running just shows the frame late. Set `MARIO_VSYNC=0` to make
the comparison fair:

```sh
MARIO_VSYNC=0 python3 app/main.py
```

## Not done yet

- **Animation blending.** Clips are swapped on action change with no crossfade.
  Playback rate and start frame now follow the original; the loop points in the
  headers are still ignored. Actions whose original animation depends on finer
  state (second punch, ledge climbs) fall back to a near-enough clip.
- **Tiptoe and walk cycles are unreachable on a keyboard.** The clip is chosen
  from whichever is larger, Mario's speed or how far the stick is pushed, and a
  key is always full deflection — so it selects the run cycle immediately. That
  is faithful; it just needs an analog stick to show.
- **The camera** is a following camera, not a port. The original is a large state
  machine with per-area modes and hand-authored triggers. Mario's control feel
  depends on the camera's yaw, which is wired up correctly, but the camera's own
  behaviour is an approximation.
- **Water, objects, and the moving-texture system.** No water surfaces, no coins,
  trees, or warps; the level's special objects are parsed out to
  `collision_objects.json` but nothing consumes them yet.
- **Swimming, and most cutscene/automatic actions** (poles, hanging, cannons).
- **Sound.**
