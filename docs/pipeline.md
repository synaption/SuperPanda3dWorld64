# Asset pipeline

> [Documentation home](README.md) · [Project guide](project-guide.md) ·
> [Aiming and attack animation design](aim.md)

## Source-of-truth policy

Do not read, import, or regenerate anything from the original SM64 reference
files. The committed Blender scenes and repository-native asset data are the
sources of truth from now on. New models, rigs, animations, materials, level
geometry, collision, textures, and audio must be authored directly or derived
from those committed sources.

The tools that parse the decomp, ROM extracts, or `reference/` remain only as
historical migration utilities. They are not part of the supported pipeline
and must not be called by export-all, packaging, or normal development. The
active model path is:

```text
committed .blend source -> Blender 5.2 glTF export -> runtime .glb
```

Everything the game loads is committed under `assets/`. A clone plays and can
rebuild its Blender-authored models without the old reference material.

The tools are Python and stay Python. They do not ship with the game, they run
once and write a file, and numpy plus a Blender install is a far better place
to do mesh and animation surgery than the game's own language.

The pipeline prefers the project-local Blender 5.2 LTS executable at
`blender-5.2.0-linux-x64/blender`. `BLENDER=/path/to/blender` or the existing
`--blender` options can override it. Blender 5.2's glTF exporter is configured
to remove its redundant Armature object wrapper while retaining the standard
skin joints, weights, and animations. The Hero is the deliberate exception:
removing its Rigify object also removes its skin, so its specialized exporter
retains the rig root and the adoption pass normalizes it.

Legacy imported actor sources were normalized with
`tools/convert_legacy_blend.py`: animated actors retain a conventional Blender
Rig object and Armature modifier, while static actors have deformation baked
and contain no armature. This conversion is only for old SM64 imports; new
models should be authored normally in Blender and do not need it.

## What is committed

`assets/` holds the converted game data — about 8.5 MB, and only what is
actually loaded:

```
assets/
  billboard_tuning.json          how billboarded parts aim
  bevy/
    castle.bin                   the level, in the game's own format
    castle.glb                   the same level as renderable geometry
    water.png                    the water sheet's texture
  castle_grounds/
    collision.npz                490 vertices, 879 triangles, 2 water boxes
    collision_objects.json       special objects, including the 26 trees
    mesh.npz                     1350 vertices, 785 triangles
    mesh_materials.json          45 material groups, 44 of them textured
    textures/                    the 21 PNGs those groups reference (2.9 MB)
  mario/          mario.glb + mario_clips.json   (209 animations)
  hero/           hero.glb + hero_clips.json     (20 animations, 53 joints)
  actors/         slime, scuttlebug, tree, warp_pipe; a clips sidecar each
                  where the actor animates. goomba.* is the retired enemy the
                  slime replaced, kept as source rather than loaded.
  sounds/mario64/ full source library plus 57 runtime WAVs and .source marker
  sounds/se_zelda, sounds/vc_zelda   the Hero's effect and voice sets
```

`hero.glb` is built out of Blender rather than out of the decomp, so it goes
through a different pipeline — see [Exporting the Hero](#exporting-the-hero).

Every primary 3D asset also has a committed, self-contained Blender source.
Run `python3 tools/build_blender_sources.py --check` to audit that invariant.
The files under `assets/packs/reference/` are packaged copies, not separate
authoring assets, and intentionally share the primary asset's source. New
models can be adopted once with `python3 tools/build_blender_sources.py`; edit
and export their `.blend` files after that instead of returning to the SM64
decomp exporter. Special pipelines retain their established source names:
`castle.glb` uses `castle_grounds.blend`, and both Hero GLBs use
`TheHero.blend`.

For a rigged actor, export to a temporary GLB and run the adoption pass so
Blender's armature wrapper is normalized and the clip sidecar stays current:

```bash
python3 tools/blend_to_glb.py assets/actors/slime.blend -o /tmp/slime.glb
python3 tools/adopt_blender_export.py /tmp/slime.glb \
    --out assets/actors/slime.glb \
    --sidecar assets/actors/slime_clips.json \
    --skeleton-root Slime_Rig
```

`--skeleton-root` is only needed where the armature is not called `armature`,
which is what the decomp exporter names it. The slime was authored in Blender
and its rig is `Slime_Rig`; `tools/export_all_actors.py` carries the same map so
the batch export does not need it spelled out.

To export every Blender-authored model currently loaded by the game—Mario,
Hero, castle grounds, and the runtime actors—run:

```bash
python3 tools/export_all_blender_assets.py
```

The Hero source needs Blender 5.x. Successful exports replace the GLBs the
game already loads; no Rust changes or asset registration step is required.
`tools/export_all_actors.py` remains available when only the files under
`assets/actors/` should be rebuilt. Weapons are excluded because the game does
not currently load them.

The textures are copied in by `parse_f3d.py` rather than referenced in place.
They used to point back into `reference/RENDER96-HD-TEXTURE-PACK/`, which is 12
GB of third-party material that cannot be tracked — so a fresh clone parsed
fine and then drew the entire castle grounds untextured. Only the images the
level actually uses get copied: 21 of them, against the thousands in the pack.
Their directory structure is preserved rather than flattened, because two of
them are both called `0.rgba16.png`.

> All of this is derived from Nintendo's game data — the geometry and animation
> from the decomp, the audio extracted from a ROM, the textures from a
> community HD pack. It is committed here so the project is runnable and
> reviewable. That is a different thing from being redistributable; consider it
> before publishing this repository anywhere public.

### The `_clips.json` sidecars

glTF has nowhere to put SM64's playback metadata, so each animated actor
carries a sidecar naming, per clip, its frame count, start frame, and loop
range.

The start frame is the one that matters and the one that varies: the
single-jump landing clip starts at frame 22 of 38, and its unplayed lead-in
frames are a deep crouch — play it from zero and Mario sinks knee-deep through
the floor on every landing. Eighteen of Mario's clips have a non-zero one.

Loop points turned out to be a non-issue. `loop_end` in an animation header
*is* the frame count, and all 209 of Mario's clips have `loop_start` 0 and
`loop_end` equal to their length.

The game does not read these sidecars yet — `src/animation.rs` plays whole
clips by name out of the GLB — so they are, for now, the record of what the
decomp said rather than something the runtime consults. They are kept current
anyway: the data cannot be recovered from a `.glb` at all, and the moves still
to port are the ones that will want it.

## The slime

The one enemy that is not a decomp export. It came from a bought model pack —
`Slime.blend` and a GLB, ten authored animations, one dome mesh in two material
groups — and it replaced the goomba, whose decomp-derived rig and billboarded
face made it awkward to edit in Blender at all.

Three things had to be reconciled with the rest of the pipeline, and all three
live in `enemy::Kind` rather than in the asset, because they are facts about
where the model came from:

- **Scale.** The decomp actors are in SM64 units, a hundred to the metre, and
  are drawn at 1/100. The slime is authored at metre scale, one unit tall.
  `Kind::draw_scale` carries the difference. Its 0.7 is not a look: the mesh is
  a unit-radius dome, so 0.7 of it is exactly the 0.70 m collision radius the
  crowd's spacing is measured in, and a model wider than the cylinder that
  spaces it is a crowd that visibly interpenetrates.
- **Which clip is the walk.** A decomp actor has exactly one animation, so
  `#Animation0` was never a choice. The slime has ten, and the game wants
  `Scoot_Move` — index 6. Blender writes actions out alphabetically, so adding
  one animation renumbers the rest; `the_clip_index_is_still_the_walk` pins the
  index to the name so that is a failing test rather than a crowd pulling
  faces on the spot.
- **Materials.** The pack ships the body as `BLEND` at 56% alpha. Faithful to a
  slime and ruinous to a field of two thousand — a translucent surface is a
  sorted draw that cannot join the opaque pass — so it was flattened to opaque
  when the runtime GLB was first made, and `slime.blend` carries that.

The impostor sheets are per-model and had to be rebaked:

```bash
cargo run --release -- bake-impostors slime
```

The pack itself lives in `reference/slime-pack/`, which is not tracked. Its
licence permits integrating it into a project and forbids redistributing the
pack, so the derived `assets/actors/slime.glb` and `slime.blend` are what is
committed. That distinction matters before publishing this repository anywhere,
alongside the note about the Nintendo-derived assets above.

## Legacy migration commands

The commands in this section document how the initial conversion was made.
They read from `reference/` and are no longer an approved regeneration path.
Do not use them for current assets.

```bash
python3 tools/parse_collision.py \
    reference/Render96ex/levels/castle_grounds/areas/1/collision.inc.c \
    assets/castle_grounds/collision.npz

python3 tools/parse_f3d.py reference/Render96ex/levels/castle_grounds 1 \
    assets/castle_grounds/mesh.npz

# The two NPZs above, plus the water texture, into what the game actually
# loads. Run this after either of them.
python3 tools/convert_level.py

python3 tools/export_actor_gltf.py --actor mario --anims all \
    -o assets/mario/mario.glb

# The warp pipe. It needs both flags spelled out: its layout is `warp_pipe_geo`
# rather than the `<actor>_geo_body` the exporter assumes, and like the tree it
# carries no GEO_SCALE, so the default quarter would leave it knee-high.
python3 tools/export_actor_gltf.py --actor warp_pipe \
    --root-layout warp_pipe_geo --scale 1.0 -o assets/actors/warp_pipe.glb

# Clips that are not the decomp's. These append to the .glb the line above
# writes, so they have to run after it, and they have to run *again* whenever
# that line does -- a fresh export has no idea they ever existed.
python3 tools/retarget_anim.py --clip Zombie_Walk:zombie_walk \
    --clip Zombie_Idle:zombie_idle
python3 tools/author_skate.py

python3 tools/import_sounds.py
```

### The level blob

`tools/convert_level.py` writes three files out of the committed NPZs:

- `assets/bevy/castle.bin` — positions, normals, UVs, the collision triangles
  and their surface types, the water boxes, and the tree placements, in a flat
  little-endian format the game reads with no parser. `src/level.rs` embeds it
  with `include_bytes!`, so it is a build-time input rather than a runtime
  load, and playing the game needs neither Python nor numpy.
- `assets/bevy/castle.glb` — the same mesh as renderable, textured geometry.
- `assets/bevy/water.png` — copied out of the reference pack, since water is
  not part of the level mesh and so is not in the GLB.

`build_windows.sh` runs it before packaging, so a Windows build cannot ship a
level blob older than the NPZs it came from.

## Exporting actors

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

**Scale.** Mario's geo wraps the body in `GEO_SCALE(0x00, 16384)`, and
`0x10000` means 1.0 — so he is authored at 4x and shrunk to a quarter at draw
time. The exporter bakes that in by default, giving a ~154-unit Mario that
matches the level and collision units.

Not every actor wants it. The tree has no `GEO_SCALE` at all, and
`geo_layout.py` already applies the ones that exist, so applying the quarter
again left the trees and both enemies at a quarter of their intended size.
`ACTOR_SCALE` is per-actor for that reason.

**Rest pose is meaningless.** SM64 joints point down their own limb, so the
unposed model splays along +X. Every animation supplies rotations for all 20
joints, so it only resolves once posed — that is normal for this data, not a
broken export. In Blender, scrub any action to see him assemble.

Runtime-driven joints (`geo_mario_tilt_torso`, head look, wing flap) export as
identity, since the game drives those rather than the animation data.

**Animations are per-actor.** Only Mario keeps his in a shared `assets/anims`;
every other actor keeps its own beside its model. Reading the shared directory
for them does not fail cleanly — the tables are positional, so Mario's
20-joint animations get applied to whatever hierarchy the actor has, which
warned for the goomba and crashed outright on the scuttlebug's 42 joints. The
animation header regex also had to stop requiring array brackets, since only
Mario declares his as `struct Animation anim_00[]`.

### Materials and texture state

**UV origin.** The N64 puts the texture origin at the top-left with V
increasing downward, and glTF does exactly the same — so actor UVs export
unflipped, and nothing downstream flips them either. Flipping in the converter
silently mirrors every actor texture; the giveaway was Mario's cap logo reading
as a W.

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

## Exporting the Hero

The Hero has no decomp behind him, so `export_actor_gltf.py` has nothing to
work from and a three-step pipeline stands in its place. Run inside Blender,
with the character's .blend open:

```python
exec(open("//wsl.localhost/Ubuntu/home/bob/mario/tools/export_hero_gltf.py").read())
```

then, back on this side:

```bash
python3 tools/adopt_blender_export.py assets/hero/hero_raw.glb \
    --out assets/hero/hero.glb --sidecar assets/hero/hero_clips.json \
    --skeleton-root rig
python3 tools/lock_root_motion.py assets/hero/hero.glb
python3 tools/aim_rig.py assets/hero/hero.glb
```

`tools/build_hero.py` runs all of it headless, which is the one to use after
editing the .blend.

Five things about the source file make each step necessary, and every one of
them fails *silently* — the export succeeds, the game loads it, and the
character is quietly wrong:

**The rig ships in rest position.** `pose_position` is `REST`, which makes the
armature ignore pose evaluation altogether. The keyframes are all still there
and visible in the dope sheet; every exported clip is the bind pose held still.
`export_hero_gltf.py` forces it to `POSE`.

**Rigify carries 240 bones, 53 of which deform.** `export_def_bones` drops the
controls and mechanisms, which is also what gets the exported skeleton down to
something comparable with Mario's 30 joints.

**The material is lit through an Emission node.** Blender writes the texture as
`emissiveTexture` and leaves the base colour black, and since an emissive-only
material carries no metallic/roughness, glTF's defaults of 1.0 apply — a fully
metallic surface, which a PBR renderer draws as a flat white silhouette with
the texture washed out of it. `adopt_blender_export.py` moves the texture to
`baseColorTexture` and pins metallic to 0.

**The clips were authored on a stage, not on the spot.** Measured at the spine,
`Idle` sits at the origin, the run cycles start 0.68 units forward of it, and
`Attack 2` starts 0.83 forward and lunges another 0.85 mid-swing — a couple of
hundred game units. Played one after another they slide the character across
the ground and snap him back on every transition, while the physics position
that walls are tested against never moves. `lock_root_motion.py` shifts every
frame so the spine sits at the horizontal origin, which removes the authored
offset and the in-clip travel together. Height is left alone: the feet already
sit on the origin plane, and the vertical differences between clips are real
crouches and leaps. The attack's lunge is handed back as forward velocity in
`src/player.rs`, where a wall can stop it.

**The exported skeleton is flat.** Rigify's DEF bones in this file are not
parented to each other — they are driven by constraints off the control rig —
so `export_def_bones` has nothing to hang them from and all 53 come out as
children of `rig`. The arms are not under the shoulders and the shoulders are
not under the spine, which means no bone in the file turns the upper body, and
so nothing can aim. `aim_rig.py` inserts an `AIM_TORSO` pivot that does, plus a
`WEAPON_SOCKET` under the right hand. Because the thighs hang off `rig` rather
than off the pelvis, the pelvis can join the upper body without taking the legs
with it, and the whole insert reduces to a constant translation on thirteen
joints — which is why it is exactly lossless where rebuilding an anatomical
hierarchy is not (that moves his fingertips by up to 315 mm; the tool's
docstring has the arithmetic). It runs last: `lock_root_motion.py` works on the
joints that have no parent among the joints, and afterwards the pelvis has one.

See [docs/aim.md](aim.md) for the design the pivot exists to serve.

He is scaled by the game rather than in the export, so it is one number to
change instead of a re-export. The clips come out at Blender's scale — he is
1.18 units tall — and `src/main.rs` spawns him at 0.81 against Mario's 0.00667,
which lands the two within a few percent of each other's height. That is what
lets them share a collision radius and a jump height.

## Borrowing animations from another rig

`tools/retarget_anim.py` puts a clip authored on somebody else's skeleton onto
Mario's. It is what the zombie shamble is: `Zombie_Walk` and `Zombie_Idle` out
of `reference/mesh2motion-app`, which ships a CC0 humanoid library on a rig
with a proper T-pose bind.

The usual way to do this — take each bone's world-space rotation relative to
its rig's rest pose, and apply that delta to the other rig — is unavailable
here, for the reason given above: **Mario's rest pose is meaningless**. The
bind pose is not a pose. Every joint is written with an identity rotation and
its offset along local +X, so unposed Mario is a stack of parts all pointing
the same way, and a delta measured against that means nothing.

What makes it work is that Mario has an A-pose *clip* — `MARIO_ANIM_A_POSE`,
`0x0E` — a real, untwisted, upright pose. Reading each joint's world rotation
there recovers the one fact the bind pose withholds: how that joint's local
axes sit relative to the body. Every SM64 joint puts its bone on local +X, but
the roll about that axis is per-joint and mirrored between the left and right
limbs, and only the A-pose measures it. Both rigs are then reduced to the same
body-relative terms — bone direction, plus a forward reference projected
perpendicular to it — and the transfer is absolute rather than relative, so the
two skeletons' rest postures never have to agree.

Some notes on what that does and does not buy:

**Proportions do not transfer, and cannot.** Bone *directions* are copied;
lengths stay Mario's. His legs are shorter and his thigh/shin ratio is inverted
against the source's, so a bent-knee pose that reaches the floor on the human
does not reach it on Mario. The clip is dropped to sit at the same average
clearance as `MARIO_ANIM_WALKING` rather than pinned so nothing ever
penetrates: the decomp's own walk swings between seven units through the floor
and six above it, so pinning the worst frame is what would look wrong.

**The head reads louder than it does on a human.** The source droops its head
54° forward. On the mannequin that is a slouch; on Mario, whose head is a third
of his height, it is the whole silhouette. This is faithful, not broken —
rendering the source rig beside it is how to tell the difference, and worth
doing before adjusting anything.

**Clips that are not the decomp's have no number.** Their id is their name, so
`anim_zombie_walk` sits in the same tables as `anim_48` and `anim_C5`. The tool
appends to `mario.glb` and updates the `_clips.json` sidecar, replacing a clip
of the same name, so it is safe to re-run. It is not safe to *skip*:
re-exporting Mario from the decomp rewrites the `.glb` and drops every borrowed
clip.

`tools/author_skate.py` is the other way to get a clip Mario does not have —
writing one outright rather than borrowing it. The ice-skating cycle is
authored as joint rotations in code, for a pose there was no source for.

## Frame counts

`rig.frame_count` recovers a clip's length in poses from the time of its last
key, which is the one thing that survives a trip through Blender intact —
channels get split, merged and resampled, so counting keys does not.

Clips are authored as N poses one tick apart at 30 Hz, which puts the last key
at (N-1)/30 rather than N/30, so the count is a round-to-nearest-tick plus the
pose at t=0. This used to subtract that last pose instead, to match a glTF
loader that resampled every channel onto its own grid and read the last key's
*time* as the clip's *duration*. Nothing resamples the clips now — they are
played as written, keys and times both.

## Blender-side tools

These run inside Blender rather than against exported files:

- `tools/build_castle_blend.py` — the castle grounds as a Blender scene.
- `tools/build_quad_planet.py`, `tools/build_valkyrie.py`,
  `tools/render_valkyrie.py` — scene construction and rendering for work in
  progress.
- `tools/blend_to_glb.py` — the generic `.blend` → `.glb` path, and the
  Blender-resolution helper the Hero build uses.
