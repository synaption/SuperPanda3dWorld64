# Assets

`assets/` holds the converted game data — about 8.5 MB, and only what is
actually loaded:

```
assets/
  billboard_tuning.json          how billboarded parts aim
  castle_grounds/
    collision.npz                490 vertices, 879 triangles, 2 water boxes
    collision_objects.json       special objects, including the 26 trees
    mesh.npz                     1350 vertices, 785 triangles
    mesh_materials.json          45 material groups, 44 of them textured
    textures/                    the 21 PNGs those groups reference (2.9 MB)
  mario/          mario.glb + mario_clips.json   (209 animations)
  actors/         goomba, scuttlebug, tree, each with a clips sidecar
  sounds/         57 WAVs, plus .source recording where they came from
```

The textures are copied in by [`parse_f3d.py`](Project-Layout) rather than
referenced in place.
They used to point back into `reference/RENDER96-HD-TEXTURE-PACK/`, which is 12
GB of third-party material that cannot be tracked — so a fresh clone parsed
fine and then drew the entire castle grounds untextured. Only the images the
level actually uses get copied: 21 of them, against the thousands in the pack.
Their directory structure is preserved rather than flattened, because two of
them are both called `0.rgba16.png`.

`billboard_tuning.json` is settings rather than converted data — see
[Billboards](Billboards).

> All of this is derived from Nintendo's game data — the geometry and animation
> from the decomp, the audio extracted from a ROM, the textures from a
> community HD pack. It is committed here so the project is runnable and
> reviewable. That is a different thing from being redistributable; consider it
> before publishing this repository anywhere public.

## Regenerating it

Only needed when the source data changes. All of these read from `reference/`
and write into `assets/`.

```bash
python3 tools/parse_collision.py \
    reference/Render96ex/levels/castle_grounds/areas/1/collision.inc.c \
    assets/castle_grounds/collision.npz

python3 tools/parse_f3d.py reference/Render96ex/levels/castle_grounds 1 \
    assets/castle_grounds/mesh.npz

python3 tools/export_actor_gltf.py --actor mario --anims all \
    -o assets/mario/mario.glb

python3 tools/import_sounds.py
```

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
