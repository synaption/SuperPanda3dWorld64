# Not done yet

- **Animation blending.** Clips are swapped on action change with no crossfade.
  Playback rate and start frame follow the original. Actions whose animation
  depends on finer state (second punch, ledge climbs) fall back to a
  near-enough clip.
- **Tiptoe and walk cycles are unreachable on a keyboard.** The clip is chosen
  from whichever is larger, Mario's speed or how far the stick is pushed, and a
  key is always full deflection — so it selects the run cycle immediately. That
  is faithful; it just needs an analog stick to show.
- **The camera** is a following camera, not a port. The original is a large state
  machine with per-area modes and hand-authored triggers. Mario's control feel
  depends on the camera's yaw, which is wired up correctly, but the camera's own
  behaviour is an approximation.
- **Objects are a thin slice.** Trees, goombas and scuttlebugs run; there are no
  coins, no warps, and no other enemies. `collision_objects.json` carries more
  presets than anything consumes.
- **Scuttlebug legs render as thin wire quads.** Fifteen of its animated parts
  carry flat `LAYER_ALPHA` display lists that the geo does not billboard, so
  unlike its body they have nothing to turn toward the camera.
- **Most cutscene and automatic actions** (poles, hanging, cannons), and the
  parts of swimming that need systems this port does not have: drowning and the
  breath meter (no health), metal-cap water walking, and carrying an object
  while swimming.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
