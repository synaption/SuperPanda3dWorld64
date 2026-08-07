# Project layout

```
sm64py/
  math_util.py     binary-angle trig, Panda3D coordinate bridge
  surfaces.py      collision triangles, spatial partition, floor/ceil/wall queries
  level.py         converted mesh -> Panda3D geometry
  camera.py        following camera
  objects.py       trees and enemies: spawning, behaviour, stepping
  billboard.py     aiming billboarded actor parts at the camera, and its settings
  audio.py         sound events -> Panda3D, plus placeholder sample synthesis
  mario/
    constants.py   action ids, action flags, input bits, surface types
    state.py       per-frame state, controller sampling, geometry queries
    steps.py       quarter-step integration, gravity, ledge grabs
    actions.py     the action state machine
    animations.py  which animation clip each action plays
    water.py       the submerged action group
tools/
  parse_collision.py     collision.inc.c -> npz
  parse_f3d.py           F3D display lists -> textured mesh + its textures
  geo_layout.py          geo layouts -> actor node tree
  sm64_anim.py           animation tables -> per-frame joint rotations
  glb.py                 minimal glTF 2.0 / GLB writer
  export_actor_gltf.py   actor -> rigged, animated .glb
  import_sounds.py       extracted AIFF samples -> assets/sounds/*.wav
  workbench.py           look at / measure one asset, interactively or headless
  check_anim_grounding.py  grounded actions keep their feet on the floor
  check_billboards.py      billboarded parts track the camera and hold width
  check_movement.py        the movement figures quoted under "Verified behaviour"
  check_sound.py           why the game is silent, layer by layer
app/main.py        the runnable game
```

The four `check_*` scripts all run headless and print what they measured.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
