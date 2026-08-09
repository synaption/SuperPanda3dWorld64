# Outer Wilds player controller — Panda3D port

A port of the blueprint-only Unreal project in
`reference/OuterWildsPlayerControlle` to Panda3D.

```bash
python3 -m ow.main              # fly the demo system
python3 -m ow.main --selftest   # headless physics checks, no window needed
```

## Controls

| | |
|---|---|
| `W` `A` `S` `D` / arrows | jetpack thrust, along the direction arrow |
| `Space` / `Left Ctrl` | thrust up / down |
| `Left Shift` | brake — thrust against your current velocity |
| mouse | look |
| `Q` + mouse | roll instead of yaw |
| `R` | snap the camera to where you are actually pointing |
| `F1` / `F2` / `F3` | toggle HUD / zero-g / planets attracting each other |
| `Esc` | release the mouse, then quit |

The original binds only a gamepad for movement (`Gamepad_Left2D`); `Space`,
`Left Ctrl`, `Left Shift` and the mouse are its keyboard bindings verbatim.
WASD and `Q` are added here so the port is playable without a pad.

## How it works

Two orientations, and keeping them apart is the whole trick:

- the **direction arrow** is where you are actually pointing. Look input turns
  it instantly, and every jetpack thrust is applied along *its* axes.
- the **camera** chases the arrow with an exponential lag (`CameraLag = 2.25`).

Because thrust follows the arrow and not the camera, the ship answers the mouse
immediately while the view swims after it. That split is what produces the
floaty, momentum-heavy feel.

Thrust is queued as a *force* onto the player's own `GravityComponent`, into
the same list gravity uses, so one integrator sums jetpack and gravity rather
than having two systems fight over the velocity.

### Gravity is deliberately not inverse-square

The Unreal graph carries this comment on the force node:

> Use physics formula: Fg = (G \* m \* M) / r^2. We substitute r^2 for just r
> for gameplay reasons: a delicate balance between feeling gravity from afar
> vs. on-planet.

So the falloff is **linear**. Combined with `GravityConstant = 1e7` and the
demo level's radii, surface gravity across the system lands between 6 and 11
m/s² — the constants were tuned around that substitution, and "fixing" the
exponent breaks the level. `--selftest` asserts the linear law so nobody
tidies it away later.

## Layout

| module | ported from |
|---|---|
| `constants.py` | `DA_CharacterVariables`, `AC_GravityComponent` defaults |
| `gravity.py` | `AC_GravityComponent` |
| `movement.py` | `AC_SpaceMovementComponent`, `BFL_ZeroGFunctions` |
| `level.py` | `L_DemoLevel` |
| `world.py` | the tick `BP_PlayerController` drove |
| `app.py` | `BP_Player`, the Unreal scene, Enhanced Input setup |
| `geometry.py` | procedural meshes — the port has no asset dependencies |

`gravity`, `movement`, `world` and `level` never touch the scene graph or a
window, which is what lets `--selftest` run the physics headlessly.

## Where the numbers came from

The Unreal project is blueprint-only: no C++, and the logic lives in binary
`.uasset` files. Rather than guess, the assets were parsed directly, and
`ow/tools/` keeps that reader so the values can be re-derived if the Unreal
project changes:

```bash
python3 -m ow.tools.dump_blueprint \
  reference/OuterWildsPlayerControlle/Content/OuterWildsPlayerController/\
OuterWildsPlayerController/Blueprints/Data/DA_CharacterVariables.uasset
```

```
MoveAcceleration   = 3000.0     UpDownAcceleration = 3000.0
BrakeAcceleration  = -4500.0    CameraLag          = 2.25
RotationSpeed      = 130.0      RollSpeed          = 130.0
```

The 14 planets in `level.py` — positions, scales and masses — and the player
spawn came out of `L_DemoLevel.umap` the same way.

Units stay Unreal's (centimetres, degrees, seconds) so every constant
transfers without a conversion factor hiding a bug. Unreal is left-handed and
Panda is right-handed, so level positions have Y negated on import.

## Deliberate differences from the original

Four, all noted in the code at the point they matter:

1. **Fixed 60 Hz physics step.** The original ticks on the render frame, which
   makes the n-body system's trajectories depend on framerate. Same arithmetic,
   stable step.
2. **Double-precision position and velocity.** Panda's default vectors are
   float32; at the demo system's ~5·10⁵ cm scale that loses about a centimetre
   per component and lets momentum visibly drift. Orientation stays float32.
3. **Braking cannot reverse your velocity.** The original applies the braking
   force unclamped, so a hard stop overshoots into a slow drift backwards. The
   final frame is clamped to land exactly on zero.
4. **Mouse look is not scaled by delta-time.** A mouse reports a displacement,
   not a deflection, so scaling it by `dt` — as the original's shared look path
   does — turns you further at low framerates. Stick input still takes the
   dt-scaled path.

## Not ported

The demo level's static geometry and the gym level (`L_GymLevel`, with its
accelerating/braking/spinning mannequin test rigs) are Unreal assets rather
than logic, so the port ships the solar system only. There is no landing,
collision or walk mode in the original either — you pass through planets, and
so do you here.
