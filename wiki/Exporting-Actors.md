# Exporting actors for Blender

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
is the ledge grab, which hangs below its position by design, and the submerged
group, which is off the ground by definition.

**Loop points turned out to be a non-issue.** This was listed here as an
outstanding gap. It is not one: `loop_end` in an animation header *is* the frame
count, and all 209 of Mario's clips have `loop_start` 0 and `loop_end` equal to
their length. The exporter was reading both correctly all along and there is
nothing to honour. `start_frame` is the field that actually varies — 18 clips
have a non-zero one.

**The swim stroke is timed to its clip.** Breaststroke runs a 14-frame action
timer and `SWIM_PART1` is exactly 13 frames, so it plays through once per stroke
with no rate scaling — the clip and the action were authored to the same length.
Re-pressing A during the arm sweep rewinds the clip mid-action rather than
changing clip, so a held stroke reads as one continuous cycle instead of visibly
retriggering; that needed a way for an action to ask for a restart without the
clip name changing (`MarioState.anim_reset`). Above forward speed 14 the flutter
kick stops re-asserting its clip altogether and whatever is playing keeps
running, so `animations.resolve` can return `None` meaning "leave it alone".

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

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
