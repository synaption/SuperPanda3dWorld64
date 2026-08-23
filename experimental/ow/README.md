# Outer Wilds player controller — Panda3D port

A port of the blueprint-only Unreal project in
`reference/OuterWildsPlayerControlle` to Panda3D. Rocky bodies use the
generated terrain mesh from `experimental/planet_gen/out/planet_lod1.glb`;
their collision and gravity surfaces remain spherical, as in the original
controller simulation.

```bash
python3 -m ow.main              # fly the demo system
python3 -m ow.main --selftest   # headless physics checks, no window needed
```

`python3 ow/main.py` works as well, from any directory.

## Controls

| | |
|---|---|
| | flying | on foot |
|---|---|---|
| `W` `A` `S` `D` / arrows | jetpack thrust along the direction arrow | walk across the surface |
| `Space` | thrust up | jump |
| `Left Ctrl` | thrust down | — |
| `Left Shift` | brake — thrust against your velocity | — |
| mouse | look | look (turn about the surface normal) |
| `Q` + mouse | roll | — (the axis is needed for turning) |
| `R` | snap camera to where you are pointing | — |

`F1` HUD · `F2` zero-g · `F3` mutual planet gravity · `F4` gravity sourcing ·
`Esc` release the mouse, then quit. The HUD also shows the current render
frame rate and the slowest frame duration from the preceding ten seconds.

The original binds only a gamepad for movement (`Gamepad_Left2D`); `Space`,
`Left Ctrl`, `Left Shift` and the mouse are its keyboard bindings verbatim.
WASD and `Q` are added here so the port is playable without a pad.

On foot, holding `Space` performs one normal jump; release it before pressing
again to use upward jetpack thrust after leaving the ground.

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

### Planets do not attract each other

`bPlanetsAttractEachOther` is **false** in the Unreal class defaults, with no
override anywhere in the demo level, and its tooltip spells out the intent:

> Disables gravity between planets if False. If False, only player will be
> affected by gravity.

So the solar system is scenery: the planets hold station and only the player
falls. That is not a shortcut, it is load-bearing — nothing in the level is
given an orbital velocity, so switching mutual attraction on makes the whole
system collapse inward (kilometres of drift within a minute, planets reaching
180 m/s). `F3` toggles it if you want to watch that happen; the self-test
covers both states.

Note the flag means *planets feel nothing at all*, not merely nothing from
other planets. Letting a planet still feel the player would have it dragged
around at metres per second, since the player's mass of 1 is the same order as
a planet's 1–8.

### Gravity is deliberately not inverse-square

The Unreal graph carries this comment on the force node:

> Use physics formula: Fg = (G \* m \* M) / r^2. We substitute r^2 for just r
> for gameplay reasons: a delicate balance between feeling gravity from afar
> vs. on-planet.

So the falloff is **linear**. Combined with `GravityConstant = 1e7` and the
demo level's radii, surface gravity across the system lands between 9.5 and 17
m/s² — the constants were tuned around that substitution, and "fixing" the
exponent breaks the level. `--selftest` asserts the linear law so nobody
tidies it away later.

The field stops weakening **1 km above a planet's collision surface** and
remains constant farther away. Adjust `GRAVITY_LINEAR_FALLOFF_DISTANCE` in
`ow/constants.py` to tune that height.

### Walking, and why gravity had to change

The original has no ground mode: you are a sphere with a jetpack, and landing
just means resting against a collider. This port adds walking, which needed one
change to gravity underneath it.

`AC_GravityComponent` sums the pull of **every** body at once. With a linear
falloff the distant ones stay significant, so standing on Hearth you get 13.8
m/s² from Hearth and another ~6 from four other planets and the sun — leaving
the net field **38° off the local vertical**, with a 5.9 m/s² sideways
component. Surfaces are not level, so you slide, and eventually the slide
throws you off. Walking on that is not really possible.

So the default here is **nearest-body gravity**: only the closest surface pulls
you, which puts gravity exactly along the local normal wherever you stand. `F4`
switches back to summing every body, the way the Unreal component does — the
HUD says which is active, and the self-test covers both.

On the ground:

* your feet swing round to the surface over about a third of a second, so
  landing rights you rather than snapping;
* mouse X turns you about the surface normal, mouse Y tilts the view only
  (clamped to ±85°, no roll), and the direction you walk stays in the tangent
  plane regardless of where you are looking;
* one acceleration serves as both drive and friction, so releasing the keys
  skids you to a stop;
* the camera follows tightly on foot (`GROUND_CAMERA_LAG`) — `CameraLag`'s
  0.44 s is right for drifting in space and unusable for mouse-look walking.

Walking a sphere in straight tangent steps lifts you off it very slightly
between contacts, and your facing has to be re-flattened onto the tangent plane
every step as the ground curves away — skip that and a steady walk gently
launches you off the planet. A full lap of Hearth holds within half a
centimetre of the surface.

### Collision

The planets do block you. `AC_GravityComponent` moves its owner with
`K2_AddActorWorldOffset` and the `bSweep` pin set to `true` (its sibling
`K2_SetWorldRotation` leaves the equivalent pin `false`, which is the control
case), against `BlockAll` sphere colliders. Radii, per unit of
`BP_Planet.Scale`:

| | |
|---|---|
| planet collider | 32 — `USphereComponent`'s default `SphereRadius` |
| planet mesh | 32.5 — `/Engine/BasicShapes/Sphere` (radius 50) at `RelativeScale3D` 0.65 |
| player collider | 32, unscaled |

Movement is swept rather than point-sampled, so nothing tunnels through a
planet regardless of speed.

## Layout

| module | ported from |
|---|---|
| `constants.py` | `DA_CharacterVariables`, `AC_GravityComponent` defaults |
| `gravity.py` | `AC_GravityComponent` |
| `movement.py` | `AC_SpaceMovementComponent`, `BFL_ZeroGFunctions` |
| `level.py` | `L_DemoLevel` |
| `world.py` | the tick `BP_PlayerController` drove |
| `app.py` | `BP_Player`, the Unreal scene, Enhanced Input setup |
| `geometry.py` | procedural starfield and emissive sun meshes |

The rocky planet render mesh is the space LOD exported by `planet_gen`. Run
its Blender exporter again after changing its authored face maps to update the
planets in this demo.

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

The first two are features you asked for and are the biggest departures; the
rest are small corrections. All are noted in the code at the point they matter.

1. **Nearest-body gravity, not the sum of every body** (`F4` restores the
   original). Without it surfaces are 38° off level and cannot be walked on.
2. **A walk mode**, which the original does not have at all.

Then:

3. **Fixed 60 Hz physics step.** The original ticks on the render frame, so
   your trajectory through the system depends on your framerate. Same
   arithmetic, stable step.
4. **Double-precision position and velocity.** Panda's default vectors are
   float32; at the demo system's ~5·10⁵ cm scale that loses about a centimetre
   per component and lets momentum visibly drift. Orientation stays float32.
5. **Braking cannot reverse your velocity.** The original applies the braking
   force unclamped, so a hard stop overshoots into a slow drift backwards. The
   final frame is clamped to land exactly on zero.
6. **Mouse look is not scaled by delta-time.** A mouse reports a displacement,
   not a deflection, so scaling it by `dt` — as the original's shared look path
   does — turns you further at low framerates. Stick input still takes the
   dt-scaled path.
7. **Contact cancels the inbound velocity.** Unreal leaves velocity alone on a
   blocking hit, so resting on a planet keeps accumulating speed into it — a
   few seconds of that and you launch when you finally thrust away. Here the
   normal component is removed on contact and the tangential part kept, so you
   settle and can still slide.
8. **Planets are drawn at their collision radius**, not the authored 32.5. The
   mesh scale of 0.65 was reaching for the collider's 32 and overshot by 1.6%;
   at planet scale that is metres of mesh standing proud of the surface you
   stop on, which buries the camera inside the sphere on landing.

## Not ported

The demo level's static geometry and the gym level (`L_GymLevel`, with its
accelerating/braking/spinning mannequin test rigs) are Unreal assets rather
than logic, so the port ships the solar system only.
