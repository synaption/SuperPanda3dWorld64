# Project guide

> [Documentation home](README.md) · [Quick start](../README.md) ·
> [Asset pipeline](pipeline.md) · [Aiming and attack animation design](aim.md)

This is the detailed technical and design reference for the game. For a quick
overview and launch instructions, see the [root README](../README.md); for the
tools that produce what it loads, see the [asset pipeline](pipeline.md).

## Overview

The project combines elements of different old N64 games, taken from their
decomps, for research. It starts with Super Mario 64 — the castle grounds, and
Mario's motion system as reconstructed by the Render96 recomp.

The character you play is **Luna**, a rigged model with twenty
hand-authored clips of his own. He is not Mario in a different costume: rather
than retarget Mario's 209 animations onto him, he has his own action machine
built around the moves he actually has — walk, run, jump, a two-hit sword
chain, a spin kick out of a run, skates on the ground and flight in the air.
What he shares with Mario is everything below the neck: the same level, the
same collision, the same quarter-step movement, and the same 30 Hz tick.

Mario is still here. He wanders the field as an NPC, **F2** hands control back
to him at wherever Luna is standing, and the field can be filled with more
of him as a squad. Keeping him is what makes the two movement systems directly
comparable over the same ground.

The game is a Rust/Bevy application. It began as a port of a Panda3D
implementation in Python, which was removed once the port took over — comments
throughout `src/` cite paths under `sm64py/` and `app/` as provenance for a
constant or a rule. Those files are in `git log`, not in the tree.

## Building and running

The game targets Bevy 0.19, which requires Rust 1.95 or newer. A distro
compiler on `PATH` is likely older than that; the rustup toolchain alongside it
is current and carries the Windows cross-target as well, so put it first:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo run
```

`./run_bevy.sh` from the repository root runs from any working directory. By
default it refreshes the packaged Windows build and then launches it;
`--source` builds and runs the native Rust source, while `--packaged` launches
the existing Windows package without rebuilding it.

Sound and gamepad support build against ALSA and udev on Linux, which the WSL
environment here has no headers for, so on Linux they are opt-in and a plain
`cargo run` is silent and keyboard-only. With `libasound2-dev` and
`libudev-dev` installed, ask for them:

```bash
cargo run --features sound,gamepad
```

Nothing else is conditional. The sound *tables* and the input snapshot are
always compiled and tested; only the device backends come and go. The Windows
build always has both — those backends are part of the OS there.

### The Windows executable, from Linux/WSL

Install a MinGW-w64 cross-compiler, then:

```bash
./build_windows.sh
```

The script adds the `x86_64-pc-windows-gnu` standard library through rustup if
it is missing, regenerates the level blob, and produces:

```text
dist/windows/SpaceCrusaders.exe
dist/SpaceCrusaders-windows-x64.zip
```

The ZIP includes the runtime GLB assets and is the file to copy to Windows.
Only the sound samples the tables actually name go in, read out of the tables
themselves so the two cannot drift.

Development builds load assets from the repository's `assets/` directory.
Packaged builds place that directory beside the executable instead, which is
what `asset_path()` in `src/main.rs` chooses between.

## Layout

```
src/
  main.rs        app setup, schedules, the scene, character switching
  world.rs       which level is up: the catalogue, the switch, the planet load
  level.rs       collision: the castle's X/Z grid and the planet's face grid
  furniture.rs   what a level has in it, as placed in Blender: water, pipes,
                 spawns, gravity
  gravity.rs     which way is down -- flat, or towards the middle of a planet
  player.rs      both characters' controllers, the water medium, combat state
  camera.rs      the third-person aiming camera: look, follow, boom, occlusion
  input.rs       keyboard, mouse and pad merged into one latched snapshot
  animation.rs   which clip each character plays, and how fast
  billboard.rs   turning billboarded objects and actor parts to face the camera
  aim.rs         the AIM_TORSO twist: where he is aiming and how far he turns
  weapon.rs      what is in his hand, and what happens when he fires it
  enemy.rs       slimes and ants: behaviour, LOD, hit resolution
  pipe.rs        warp pipes, and the arc they throw their brood out on
  squad.rs       aiming at the ground, and the Marios whistled up and sent out
  water.rs       the water sheets and the underwater view
  sky.rs         the day and night cycle: sun, moon, stars and the sky dome
  audio.rs       gameplay sound events -> layered samples
  console.rs     the debug console: commands, live tuning, pinned controls
  menu.rs        the pause menu: Escape, its pages, and what they change
  display.rs     the internal render target and the stretch onto the window
tools/           the asset pipeline -- see docs/pipeline.md
assets/          everything the game loads -- see docs/pipeline.md
```

Tests live beside the code they cover, in `#[cfg(test)]` modules, and run
headless. That is deliberate and is worth keeping: the simulation never touches
a window, an audio device or a pad, so `cargo test` exercises movement,
collision, combat, spawning, the sound tables and the console without opening
anything on the desktop.

## Controls

Keyboard and pad are read into one snapshot per frame, so both are live at
once and neither has to be selected.

| Action | Keyboard and mouse | Gamepad |
| --- | --- | --- |
| Move | `WASD` / arrows | left stick or d-pad |
| Look | mouse, or `Q` / `E` | right stick |
| Jump (booster take-off while skating) | `Space` | south (A) |
| Attack: swing the sword, or fire the gun | Left Shift or left mouse | east (B) |
| Skate on the ground, fly in the air (Luna) | hold `V` | hold north (Y) or left trigger |
| Aim | hold `F` or right mouse | right trigger |
| Switch weapon (sword / pistol) | `Y` | left shoulder |
| Recenter camera | `R` | right shoulder |
| Squad: hold to whistle, tap to send | `X` | west (X) |
| Build a stellarator: hold to grow it, tap for the smallest | `B` | Select |
| Plant a pylon: hold to open a site, release to plant | `G` | right stick click |
| Switch Luna/Mario in place | `F2` | Start |
| Pause menu (pauses the simulation) | `Escape` | Select |
| Tuning console (pauses the simulation) | `` ` `` | — |
| Debug text | `F1` | — |
| Window/fullscreen | `F11` | — |

The attack control is one button carrying two weapons: with the sword out it
swings, and with the pistol out it fires. Both read the same latched edge and
only the equipped one is allowed to take it, so a press never does both. See
`src/weapon.rs`.

Landing on an enemy defeats it whichever character is active. A pad plugged in
mid-game is picked up without a restart, and the pad's sticks use circular
deadzones — 0.18 to move, 0.12 to look — so a gentle diagonal is still
holdable.

The look stick squares its deflection. That keeps the first half of the stick's
travel slow enough to aim with while the far end still whips the view around,
which a linear stick cannot do at any single sensitivity.

## Timing and the port

**Game logic runs at a fixed 30 Hz**, because every movement constant in the
action code is per-frame at that rate. Rendering is decoupled and the
simulation is stepped in whole ticks. The drawn transform is *interpolated*
between the last two ticks: without it, on a 144 Hz display over 90% of
rendered frames land between ticks and redraw the character at the same spot,
so he freezes for four frames and then jumps — the motion is correct but reads
as a stutter.

**Input is latched, not polled from inside the simulation.** Input arrives at
the render rate while gameplay runs at 30 Hz, so a frame may hold two fixed
steps or none. A `just_pressed` read inside the fixed step would fire a jump
twice on a slow frame and swallow it on a fast one. Presses are recorded once
per frame in `src/input.rs` and consumed by the step that acts on them, so an
edge fires exactly once whenever that step happens to run.

**The original's collision quirks are not kept.** They were, once: floor
queries took the first triangle in list order that passed rather than the best
one, and wall pushback applied each wall's full shove as it was found. Both
made where a body ended up a function of the order the level mesh happened to
be built in. `LevelData::resolve_walls` now measures every overlap in a pass
against the same starting position and combines them deepest first, and the
body it measures is a real capsule tested against the whole of each triangle
rather than three sample spheres strung along a line — so a wall that meets a
body between two of those spheres is a wall it is pushed out of.
`wall_resolution_does_not_depend_on_triangle_order` is the test that says
shuffling the mesh moves nobody, and
`wall_resolution_clears_a_wall_the_middle_of_the_body_meets` is the one that
says the parapet at waist height is solid.

A capsule is not a column, and the difference is what a body is pushed by. Each
contact is asked how far apart the two have to be *horizontally* given how far
apart they already are vertically — `sqrt(radius² - climb²)` — so a face level
with the body pushes by the whole radius, and one a radius or more above its
head or below its feet does not push at all. Leaving that out put invisible
walls on the bridges: a moat face five metres under the deck sits almost
directly beneath the edge you walk along, which is no horizontal distance at
all, and a body clear of it by five metres was being shoved a metre and a half
sideways. `a_face_below_the_deck_is_not_a_wall_on_it` is the test that says it
is not.

**Distances are the original's, converted.** The port works at 1/100 of SM64's
scale, so a constant that reads 8 units per frame in the decomp is 2.4 units a
second here. The conversions are written out where they appear rather than
hidden in a helper, because the frame-rate half of them is as easy to get wrong
as the scale half.

## The level

There are two, and the pause menu's **Level** page chooses between them. What a
level is, in this port, is three things: something to draw, something to
collide with, and a direction for down. `world.rs` owns all three, and
everything it spawns carries a `LevelEntity` marker so that changing level is
one despawn rather than a list somebody has to keep up to date.

### The castle grounds

The castle arrives as one blob, `assets/bevy/castle.bin`, embedded in the
binary with `include_bytes!` — so the game needs neither Python nor numpy to
start, and the level cannot go missing. It carries the render mesh, the
collision triangles and their surface types, and the tree placements.
`assets/bevy/castle.glb` is the same level as textured geometry.

Everything *placed* in the level comes from somewhere else: `assets/levels/
castle.blend`, by way of `assets/bevy/castle_furniture.json`, which is embedded
the same way. Water, warp pipes and what each produces, the enemies standing
about, the spawn point and which way gravity points were all literals in
`world.rs` and `water.rs` until they became empties you can drag. See
[Level furniture](pipeline.md#level-furniture) for what to call them.

Static collision is partitioned into a 64×64 X/Z grid built at load time, so
floor, wall and camera queries inspect only nearby triangles instead of all
879.

Castle walls occlude the camera and block actors. The warp pipes are drawn but
not collided with — the level's own collision is all the physics reads, and the
actors' `collision.inc.c` is not loaded, so you can walk through one.

### The planet

`assets/bevy/planet.glb` is the generated planet from
[`experimental/planet_gen`](../experimental/planet_gen/readme.md): a cube-sphere
about 300 m in radius with ±40 m of terrain on it, 786,432 triangles across 96
tiles. Its collision **is** its render mesh, read back out of the glTF once
Bevy has finished loading it. That is why choosing it is a wait rather than a
frame: the pause menu stays up saying `LOADING PLANET` and, because the menu
being open is what holds the simulation still, the player is not falling
through an empty world meanwhile.

Nothing else is on it yet — no enemies, no pipes, no water, no crowd. Those all
assume a flat level in ways that are described below, and none of that work is
needed to walk around a planet.

#### Filing collision on a sphere

The X/Z grid cannot be reused, and not as a matter of tuning. Project a sphere
onto X/Z and the far side lands on top of the near side: every cell holds two
hemispheres, and the highest surface in the column is on the wrong one. So a
planet is filed by the **cube-sphere face cell** a direction points through —
six faces, 96×96 cells each, about five metres on a side. A cell is a patch of
surface again, and every query is the shape it always was.

Two details in `level.rs` are load-bearing and neither is obvious:

- **Triangles are filed by sampling, not by their corners.** Corners alone are
  enough for this planet, whose terrain triangles are under two metres across
  against a five-metre cell, and that is exactly the sort of thing that stops
  being true when someone exports at a coarser depth or drops in an authored
  tile with one big flat face. The sample spacing comes off the triangle's own
  angular size, so a small triangle still costs three samples.
- **Slivers are thrown away.** A mesh's poles are often several vertices that
  ought to be one point and, in `f32` hundreds of metres from the origin, are
  not. The triangles between them are metres long and microns wide. Their
  normal normalises perfectly well, so a zero-area test does not see them, and
  the determinant in the ray test — which exists to reject exactly this — comes
  out thousands of times its own epsilon. The symptom is a ray straight down
  reporting a hit two thirds of the way to the core, which is to say an
  invisible floor in the middle of the world.

Queries cross a face seam by perturbing the query direction and re-filing it,
rather than by a neighbour table: the perturbed direction lands on whichever
face is actually next door, which is the same answer a table would give and one
fewer thing to keep correct.

## Gravity

`gravity.rs` is one resource with two shapes: `Down`, where up is `+Y`
everywhere, and `Radial`, where up is away from a point. Everything that used
to read `.y` asks it instead — the player's fall and jump, the floor and
ceiling queries, the wall push-out, which way the character faces, and the
camera's orbit.

The strength is unchanged and is the original's: `app/main.py` stepped −1.2
onto a body's speed every frame at 30 Hz, which is 36 m/s². It is now written
as the rate it always was, so that pointing it somewhere else does not also
change it.

Three parts of the generalisation are worth knowing about.

**The player's velocity is split, not rewritten.** Each tick it is separated
into how fast he is climbing away from the ground and how fast he is running
along it; the climb is carried by hand to the bottom of the tick and put back
once. Between the two there is a wall resolution that changes the run and must
not touch the climb.

**The character is stood upright before he is turned.** On a flat level that
step does nothing — up never moves, so easing towards a rotation already held
is the rotation already held. On a planet it is what keeps him perpendicular to
the ground rather than leaning further over the further he walks.

**The camera carries its frame rather than rebuilding it.** `FollowCamera` owns
a quaternion that yaw and pitch are measured in, and each frame it is turned by
the smallest rotation taking its own up onto the local one. Rebuilding it from
scratch — `Quat::from_rotation_arc(Vec3::Y, up)` — has no answer at the antipode
of `+Y` and an arbitrary one near it, so walking towards a planet's south pole
would spin the view faster and faster and then flip it over. Turning by the
step between two consecutive frames never asks that question.

On the castle every one of these is the identity and the arithmetic is the
arithmetic it was before, which is what the collision and movement tests assert.

### Nothing follows the local down exactly

A curved surface felt jerky to walk on long before anything was wrong with it,
and the reason was that every one of the three answers above was taken straight
off the geometry in the frame it changed. There was nothing between the ground
and the pixels. The fix is borrowed wholesale from the Outer Wilds prototype in
`experimental/ow`, which walks a sphere without any of this and does exactly one
thing differently: it never *sets* anything to the surface, it drives towards
it at a rate.

`gravity::settle(rate, delta)` is that rate, and returns how much of the
remaining gap to close this step. It is a first-order ease, so `rate` is a time
constant of `1 / rate` seconds and means the same thing at 30 Hz and at 240 —
unlike a plain fraction-per-frame, which settles in half the wall-clock time at
twice the frame rate. (`camera::blend` exists to undo that for the factors that
were already written the other way; new ones should use `settle`.)

Three things use it, at three deliberately different rates:

| what | rate | why that one |
|---|---|---|
| the body standing upright | 8/s | the prototype's `GROUND_ALIGN_RATE`, unchanged |
| the camera's idea of up | 9/s | between its 20 on foot and 2.25 in flight; there is no flight mode here |
| the feet closing on the floor | 18/s | a position, and the eye calls floating wrong sooner than leaning |

The camera one is the most load-bearing, because it lands on **roll**. Taking
the camera's up off the ground normal puts every wobble in the surface onto the
horizon, and a tilting horizon is the most legible motion on a screen — far more
so than the same wobble in pitch. So `FollowCamera` carries two frames: `frame`
is the ground's answer, transported exactly as above, and `view` chases it.
Every axis the camera is built from is measured in `view`. Yaw and pitch are
**not** eased — they are the player's own input, and lagging them is input
latency rather than smoothing, which is the same split the prototype makes
between its direction arrow and its camera.

Two things are deliberately not eased:

- **A gap that is not walking.** More than `UP_SNAP` between the two ups is a
  respawn, a warp pipe or the far side of the planet, not a step, and easing
  across it is a second of the horizon rolling over for no reason. Walking never
  opens one: running flat out on a planet a few hundred metres across turns up
  by a couple of degrees a second.
- **Sinking.** Being below the floor is a wrong the player can see through, so
  it is corrected in the step it is noticed. Only the gap *above* the floor
  eases shut, and it is capped at `FOOT_SKIN`, because running downhill opens it
  as fast as the filter closes it and the two would otherwise balance several
  times higher.

That last one is the only place the prototype could not simply be copied. Its
planets are analytic spheres, so its ground has no facets and it can afford to
treat contact as a 15 cm proximity band with no placement at all. Ours is a
mesh: the triangles share vertices, so the height under the player is
continuous, but the *slope* is not — crossing an edge changes how fast he is
rising in a single step. That step change is the chatter, and easing the gap
shut is what low-passes it.

## Animation

Clips are chosen **by name** rather than by export index. Luna's names are
the Blender action names — spaces, capitals, trailing space and all; `Idle `
really does end in a space — and Mario's are `anim_XX` after the decomp's
`MARIO_ANIM_*` hex id. Names are what both exporters guarantee, and an index
shifts silently the moment a clip is added.

Three things come across with the clip choice: clips that play once and hold
their last pose rather than cycling, a walk whose playback rate tracks how fast
the ground is going by so the feet do not slide, and an idle fidget that
replaces the idle after standing still for a while. Attacks alternate into a
combo.

All UI text draws with Bevy's embedded default font, which arrives with the
`default_font` feature rather than automatically. Without it every text node
still lays out and still paints its background while every glyph silently goes
missing — so the console renders as a black bar with nothing written on it, and
nothing anywhere reports an error. A test asserts the default font handle
resolves.

## Water

Water is not part of the level mesh. It is axis-aligned boxes — two planes in
`assets/levels/castle.blend`, whose footprints they are — drawn as one flat
quad at each box's height: unlit, half transparent at the original's 0x96
alpha, and two-sided because most of the time it is looked at from underneath.
The castle's waterfall is a mesh in the same file, exported into
`castle_furniture.glb` and adopted when it loads; it was fifteen literal
vertices in `water.rs` before it was something you could reshape. Each sheet's texture drifts across the
world at a fixed speed, and the two bodies drift in different directions so
they do not read as one sheet.

A box is a plain rectangle laid over a bay of the map, so each also covers dry
ground that rises through the sheet — the moat is the part that reads as water.

Under the surface the view closes in hard and goes green-blue. That is what
sells being submerged far more than the surface quad does, since the water is a
single flat sheet with nothing behind it. The medium is chosen by where the
*camera* is, not the player: swimming just below the surface leaves the camera
in open air looking down through it, and tinting the world in that case looks
wrong.

Deep water is not one behaviour, because the two characters do not have the
same clips. Mario swims. Luna does not: he has no swimming animation, so
rather than drag a walk cycle through the water he is held just under the
surface, slowed to 0.45 of his walk, and drawn upright. Water shallower than he
floats in is simply slow ground: the bottom is under his feet and he walks
along it. `luna_wade` in the console is the speed.

## Day and night

The castle grounds run a clock. `sky_hour` on the console is the time in hours,
`day_length` is how many seconds a whole circuit takes, and setting the latter
to zero holds the sky wherever it is — which is how a sunset gets photographed.
Five minutes a day by default.

The sun runs round a circle tilted so noon lands sixty degrees up rather than
overhead, rises due east at six and sets due west at eighteen. The moon is
directly opposite it, so it is always full and always rises as the sun sets.
Four shells ride the camera, drawn beyond where the haze becomes total and
marked unfogged so the fog does not paint them out: an opaque dome whose vertex
colours are rewritten as the light moves, four hundred star quads on a sphere
that turns with the sun, and the two discs.

Everything a person sees is keyed on **the sun's elevation** rather than on the
hour. Twilight is not a time, it is the sun being just under the horizon, so a
table of stops in `sky.rs` runs from deep night to noon and everything —
zenith, horizon, the warm glow along the sun's own bearing, the key light, the
ambient, how many stars show — is read out of it and interpolated. Re-tilt the
orbit or lengthen the day and the look stays right.

Two things join the sky to the world. The camera's fog and the clear colour are
repainted with the dome's own horizon colour, so the far half of the level
dissolves into exactly the sky standing over it — which is what carries a
sunset down to the ground. And the castle is **not lit by this renderer**: its
shading is baked into its vertex colours, and the impostor sheets are
photographs of models lit at noon. Neither takes the key light at all, so
`N64Lighting` carries a fourth term, `daylight` — how much of the light the
bake was made under is left — and the shader multiplies every such surface by
it. Without it the sun sets on the actors alone, walking about on grass that
stays noon-bright at midnight. That term is derived from the same two light
values in the table rather than tabulated separately, so there is one set of
numbers to tune and not two to keep in step.

It is a linear multiplier and the screen is not: midnight comes out near 0.06,
which encodes to a bit over a quarter on the screen. That is a moonlit field
you can still play in, and it is the number to read when the night looks wrong
— picked by eye as linear values, the night stops came out at half brightness,
which is a bright afternoon.

Moving the sun means rewriting the uniform of every material in the game, since
this renderer keeps its light in each material rather than in a bind group of
its own. So the light steps rather than slides: it is rewritten once the sun
has moved a quarter of a degree, about five times a second at the default day
length, which is below what an eight-bit channel can show. The two discs are
transforms rather than uniforms and move every frame.

The castle only. A dome whose horizon is the XZ plane is a claim that up is
`+Y`, and on the planet up is whichever way the core is not — the sun would set
into the ground on one side of it and sit under the floor on the other. Leaving
the castle hides all four shells and puts the light, the fog and the clear
colour back where the rest of the game expects them.

## Sound

Gameplay never touches the audio device. The fixed-step systems append typed
events to a queue and a render-rate system drains it, so the simulation runs
identically with no device present and the whole of it stays testable headless.

Each event resolves to a stack of layers that play together, which is what SM64
itself does for a jump — a terrain sound from the ground and a voice from
Mario. Luna speaks with the Zelda voice set and steps with its effect set;
Mario uses samples imported from an extracted asset tree, because the decomp
ships a sound taxonomy and no waveforms. Jump, landing, footfalls, attacks,
taking damage, defeating an enemy, breaking the water surface, and swim strokes
are wired. `sfx_volume` in the console sets the level.

An event either carries the point in the world it happened at or it does not,
and that is the whole of the spatial half. The player's own noises are
unplaced — he is what the camera is pointed at, and panning his footfalls off to
one side only makes the view feel crooked — while a kill somebody else made
carries the position it happened at: an enemy cut down across the field, a
Mario's punch landing, a hitscan shot hitting at the far end of its beam. Placed
events are heard from where they happened. The ears go on the camera, so what is
on the left of the screen is on the left of the mix, and `sfx_range` in the
console is how far a sound carries at full volume — beyond it, half as loud each
doubling, and once it is under about a twelfth of full it is given no voice at
all — which is what keeps a crowd dying half a field away from being dozens of
decoders nobody can hear.

Direction and distance are carried separately, and by design. The emitter is put
on the *bearing* to the sound at a fixed radius from the listener rather than at
its true position, and how far away it was is carried entirely by volume. That
is a fix for the mixer underneath: rodio gives each channel a gain of
`1/distance²` to that ear, clamped to 1, times a term that rises with that ear's
distance, so a sound at its true distance pans weakly a few metres out and
towards the *wrong* side beyond about a dozen. A fixed radius and the wide ear
gap it is tuned with hold the panning at one steady, correct shape however far
off the sound was, and `audio::attenuation` shapes the falloff instead. Both
halves are plain functions over a transform and a distance, so both are tested
headless like everything else here.

The tables name their `.wav` files by hand, and a test walks every name and
asserts the file is in the repository — which is what catches a typo or a
sample renamed out from under them. `build_windows.sh` reads the same names out
of the source to decide what to package.

## The squad

The field is full of allies, and they are the squad. One button carries two
commands, told apart by how long it is held: **held**, a circle grows on the
ground where the view is pointing and everyone inside it when the button comes
up joins and follows; **tapped**, the squad it already has is sent to the spot
the same aim resolves to, spread around it, and is on its own again once it
arrives. Pikmin's shape rather than an RTS's — there is no cursor to drag a box
with, so the selection is aimed exactly the way a throw would be.

The aim is the ray out of the middle of the screen, marched until it meets
ground: left and right is where the view points, up and down is range. The spot
it returns is on the bearing from the *player* to that hit rather than the hit
itself, so the camera sitting off his shoulder does not skew the order, and it
is walked back toward him until there is floor under it — out over the moat and
off the edge of the map there is none, and an order has to land somewhere.

Followers stand in a golden-angle cluster behind the leader, which is what
keeps a crowd from forming rows with gaps between them. Switching characters
disbands the squad: half of it has just become the player, or stopped being
him.

A Mario with nothing to do ambles: it walks to a spot near where it was last
left, stands about for a few seconds, and then picks another. It walks at the
speed its own walk clip was authored to cover ground at, so its feet are
planted. Both halves matter — a destination that moves while it is being walked
to leaves an ally starting and stopping every few ticks, and since a change of
state restarts the clip, that is a field of Marios stuck on the first frames of
a step rather than a crowd milling around.

`ally_count` in the console is the population, reconciled live — set it to 60
and the lawn fills; set it to 0 and it clears. The Mario pipe's own brood is
not in that count and stays when the lawn is cleared.

**Luna is AI-playable too.** An ally is a character rather than a Mario: the
same two the player switches between with `F2` are the two the squad can be
filled with, and `luna_count` is the second population beside `ally_count`. An
AI Luna is the same model at the same scale as the one the player drives —
both come out of `ActiveCharacter::model`, so there is one place either is
named — animating off Luna's own clip table and fighting under the same rules.
What differs is what she is worth: an ally Luna carries the player's hundred
points of health against a Mario's twenty, which is what makes filling the
field with one or the other a decision rather than a colour choice. Two counts
rather than a share of one, so asking for Lunas never quietly takes Marios
away.

## Machines and the pylon network

Two things you build, on two keys, aimed the same way the squad is ordered —
the ray out of the middle of the screen, marched until it meets ground.

**A stellarator** (`B`) is the thing that makes power. Hold the key and a
footprint ring opens on the ground and the machine grows inside it up to the
size it was authored at; let go and it is built, unless the ring is red, which
means another machine is already standing there. What is drawn inside its coils
is not the plasma mesh in the glTF — that is hidden as it arrives — but sixty-
four streaks riding the same flux surface the mesh describes. See
`src/stellarator.rs`.

**A pylon** (`G`) is how that power gets anywhere. Each mast strings a beam to
every other mast within 42 m that it can see, head to head against the level's
own collision — a beam eight metres up clears a hedge a walker would have to go
round — and power floods outward from whatever machine can reach a mast at all.
The ring under the crosshair says which of three things the site is: red for
blocked by something already standing, amber for legal but joined to nothing,
cyan for legal and wired in. A live mast breathes and its beams are lit; a dark
one stands still. Standing near a live mast fills the jetpack bar four times
faster, which is the reason to push a network outward rather than ring the
machine with masts and stop.

The network shares its algorithms with the pathing rather than having its own.
`src/route.rs` holds both: `flood`, the breadth-first walk that spreads power
from mast to mast and that `flow::rebuild` sweeps the castle with to tell a
crowd of thousands which way the player is, and `tour`, a nearest-neighbour
travelling-salesman walk improved by 2-opt. The tour is what decides the order
the supply packet — the mote of light you can watch crossing the beams — calls
at every live mast in, and each leg between two calls is expanded into the
shortest chain of real beams by the same flood, so the packet never flies
through a hillside on its way to the next stop.

`pylon 5` in the console plants a ring of masts around the player with a
machine in the middle of it, and `pylon clear` takes them away: the same reason
`crowd` exists, which is that the interesting thing about a network is what it
does at a size nobody wants to build by hand. See `src/pylon.rs`.

## Billboards

SM64 draws some things as flat quads it rebuilds every frame to point at the
camera. glTF has no billboard concept, so they arrive as ordinary geometry
drawn from whatever one side they were authored on — and a quad is flat, so
from ninety degrees away there is nothing there at all. The tree mesh measures
exactly zero thick.

There are two cases and they need different machinery. A **whole object** — the
trees — is plain geometry and turns bodily, about the vertical only, so it
never tips over when the camera looks down at it. Note that nothing in the
*asset* says to do this: a whole-object billboard is set by the behaviour in
the decomp, not the geo layout — `bhvTree` is `BILLBOARD()`/`CYLBOARD()` even
though the tree's geo has no `GEO_BILLBOARD` in it at all.

**Part of an actor** — most of a scuttlebug — is skinned to a
joint, where no transform on the object can reach it. The exporter makes each
such quad a joint named `billboard_*`, and those are driven one at a time. No
actor the game loads carries one any more: the goomba and the scuttlebug both
went, and the slime and the ant that replaced them are authored art with real
skinned meshes and nothing flat on them. The machinery stays because it is what
any decomp actor coming back would need, and the tests for it now measure the
unedited decomp export kept under `assets/packs/reference`.

Driving a joint takes two things that are easy to get wrong. The rotation is
composed against the inverse of the parent joint's world rotation, because the
skeleton leaves the quad rotated a quarter turn and a heading applied on top of
that comes out as pitch — on the scuttlebug the parent sits at `(98.4, 4.9, -90.7)`,
so no value of a plain heading could ever have worked. And the aiming runs
after the animation player has posed the skeleton and before transforms are
propagated: before the animation player, every joint written is overwritten a
moment later and nothing turns at all.

Billboarded surfaces are drawn from both sides. Which face was authored toward
the viewer is not something this port can see, and aimed the wrong way round a
single-sided quad would be invisible from *every* angle rather than from half
of them. The original never gets to see the back of one, so this costs nothing.

Pitch and roll are settings and both sit at zero. It is worth saying why they
cannot help: a flat quad facing the camera has the same silhouette however it
is spun about its own normal.

**A warning about measuring this.** Counting how many pixels an enemy covers
across a camera orbit does *not* verify billboarding — the leg geometry
dominates the count and swings with the viewing angle for unrelated reasons.
Nor does measuring a scuttlebug's three billboards together: the bounding box
then tracks how far apart they are rather than how wide each one is, and
reports the broken setting as *better* than the fixed one. One quad at a time,
isolated.

## Warp pipes

Three pipes, and each produces one thing: slimes out of the one in the far
west corner, ants out of the one in the far east, and Marios out of the
one by the spawn on the castle path. Which is which, where they stand and how
long each waits are properties of an empty in `assets/levels/castle.blend`
rather than numbers in the source. The two enemy pipes are where they are on
purpose — they are somewhere to go rather than something to trip over on the
way out of the gate.

Nothing is placed beside a pipe. It is thrown out of the top: spawned down at
the pipe's feet with an upward velocity, so the launch carries it up through
the barrel and out of the mouth, which is what the pop is. It starts hidden
inside the pipe rather than appearing in mid-air above it. The arc is the
original's — 60 units a frame up against 4 a frame of gravity, so it peaks
twice the height of the pipe's own rim and stays up for a full second, carried
outwards throughout and landing about four pipe-widths away.

For that second the thing's own behaviour is suspended and the arc alone moves
it. That suspension is the trick: every behaviour writes its own speed each
tick — a slime bleeds whatever it has back toward a walk, a crawling ant
simply overwrites it with its crawl — so a launch handed to the behaviour is gone
within a tick or two and the thing lands back on the pipe it came out of.

Where it is thrown is chosen rather than taken: headings a golden angle apart
are tried until one has floor at the end of it that is not far below the pipe.
The west pipe stands a few hundred units from where the ground falls into the
moat, and without this a quarter of its slimes came down in the water. The
golden angle rather than a random number means five throws spread around the
pipe instead of stacking, with a whole run still reproducible in a test.

The Mario pipe counts its own brood and nothing else. The standing crowd that
`ally_count` answers for is deliberately outside that count, so changing it does
not despawn the Mario the pipe threw. `pipe_brood` is this ally-pipe quota. The
two enemy pipes instead share `enemy_limit`, which counts every live enemy on
the field, including the five placed by hand; `enemy_rate` is their interval.
Enemy pipes ready on the same tick consume the remaining slots in pipe order,
so the cap is exact rather than occasionally ending one over it.

The countdown runs at any distance, so a crowd is waiting when the player
arrives rather than only starting to fill then. It also only runs while there is
room, holding where it stands at the quota — so a kill starts the clock again
instead of restarting it, which is the difference between one along every so
often and a replacement appearing the instant something dies.

## Combat

The player resolves against every enemy once a tick: a swing defeats what is in
front of him, coming down on one stomps it, and touching one any other way
throws him back and costs a heart. Each enemy is an upright cylinder — a radius
and a height — rather than a point, so standing on a roof above one is not
touching it.

**That cylinder is measured, not written down.** `Kind::body` reads the
model's own position bounds out of the glTF the renderer loads: the radius is
how far the mesh reaches from its origin in the horizontal plane, the height is
how tall it is. There is no scale factor anywhere in the code, so an actor is
drawn at the size it was authored at and spaced, shadowed, stomped and punched
at the size it is drawn. **Size is set in Blender** — `tools/resize_actor.py`,
or by hand — and everything downstream follows, because the crowd's spacing, its
flow-field arithmetic and its shove distances are all expressed in body radii.

The decomp actors these replaced were drawn at a hundred units to the metre and
had a `draw_scale` constant each to undo it; the sizes were then written out a
second time as hitboxes, and the pair drifted the first time an actor was
re-exported. What guards the arrangement now is a band rather than a number:
`an_actor_is_the_size_it_was_authored_at` fails an actor outside the size this
game's crowd arithmetic was built for, which is what a four-metre ant tripped.

The immunity after a hit gates the *whole* resolution and not only the damage.
That is not a detail. A knocked-back player is thrown up and off the enemy that
hit him and comes back down on its head, so without it every enemy that touches
somebody standing perfectly still stomps itself within a couple of seconds —
and a warp pipe whose every slime destroys itself before you turn round is a
warp pipe that appears to spawn nothing at all.

## The debug console

Backquote opens a command console and pauses gameplay and skeletal animations.
`vars` lists every live value; `<name> <value>` changes one immediately; `reset
<name|all>` restores defaults. Entering a name without a value pins its control
at bottom right; brackets (`[`/`]`) adjust it, open or closed, and Shift
adjusts 10x faster. `close <name|all>` removes pinned controls. The panel also
reports live FPS, player state and position, health, and enemy count. The mouse
wheel or PageUp/PageDown scrolls through the bounded command log.

The prompt is a real line editor: Left/Right move the caret, Home/End jump to
either end, Backspace and Delete take the character on either side of it, and
typing goes in where the caret is. Up/Down recall command history and Tab
completes the word the caret is in. The keys that move or delete repeat while
held, which is what makes getting back to the start of a long line something
other than tapping — and a slow frame delivers every repeat it covered rather
than one. The caret is drawn where it actually is, since that is the only way
moving it is visible at all.

Adjusting a pinned control used to be Left/Right while the console was open,
which is why it is the brackets in both modes now: the arrow keys are the
caret's.

Movement, camera, water, enemy and spawning constants are backed by the tuning
resource rather than copied at startup, so console changes apply on the next
gameplay tick.

## The pause menu and the internal render resolution

Escape pauses the game and opens a menu — Resume, Level, Options, Quit — with
the level list and the display settings one page in each. Escape goes back a page and closes it from the
root, so the key that opened it is the key that gets out of it whatever page
you wandered into. It is keyboard-driven (arrows or `WASD`, Enter, and
left/right to change a value) and the pad drives the same rows with the d-pad,
south and east; Select opens it, because Start already swaps character. Opening
it hands the mouse cursor back and resuming captures it again.

The **Level** page lists every level in `world::LevelId`, with the one being
played marked, and choosing one takes the world down and puts that one up. It
is the one page that does more than set a number, and the one whose rows are
generated rather than written out: a level added to the catalogue appears in
the menu without anyone having to remember the page exists. A level that cannot
arrive in one frame — the planet, whose collision is 14 MB of glTF — keeps the
menu up while it loads, and the menu swallows every key until it is done. That
is not politeness: the menu being open is what holds the simulation still, and
closing it early is the player falling through a world with no ground in it
yet.

**The world is not drawn to the window.** It is drawn into an image, and a
second camera stretches that image over the window with a nearest-neighbour
filter — which is what makes the render-resolution row possible. The row is a
pixel multiplier from 1×1 through 8×8. At 2×2 the world target is half the
window width and height; at 3×3 it is one third of each, and so on. Both axes
always use the same divisor, so this works at any aspect ratio.

Everything expensive in a frame here — the water's overdraw, the vertex-lit
surfaces, the billboards — scales with the pixel count and nothing else, so the
setting is the one dial that reliably buys frame rate. The UI is deliberately
outside it: the HUD, the console and the menu are drawn by the second camera,
*after* the stretch, so they stay at the window's own resolution however low
the world is rendered.

The two cameras have to stay the way they are, and the failure is silent both
ways. If the world camera goes back to targeting the window the setting does
nothing; if it ends up the highest-order camera on the window, Bevy hands it
the UI as well and the HUD comes out blurred along with the world. A test in
`main.rs` asserts both.

## Performance

The thing that costs a frame here is **draw calls, not pixels and not
triangles**. Bevy marks every skinned mesh `NoAutomaticBatching`, because each
one needs its own joint matrices, so a skinned actor costs one draw call per
mesh primitive and no two of them ever merge. The decomp's scuttlebug was
fifteen of them for seventy-six triangles, and two thousand enemies drawn that
way was around nineteen thousand draw calls a frame. The authored actors that
replaced it are far better behaved — two primitives for a slime, one for an ant
— so the same field is now about three thousand. Still most of a frame. The
castle, for comparison, is 785 triangles and 45 draws.

So the crowd work is all about doing less per enemy, in three ways: drawing
fewer things, keeping fewer entities, and properly simulating fewer of them.

### Drawing fewer things

- **Impostors.** Past `enemy_draw` an enemy is drawn as a camera-facing sprite
  from a baked atlas rather than as a skeleton, and the whole distant crowd of
  one kind is rebuilt every frame into a *single* mesh — one draw call for a
  thousand slimes. See `impostor.rs`, and `impostor/bake.rs` for the baker.

  A sprite is a photograph, so it is only right from the height it was taken
  at. The atlas therefore holds two: bearings round the actor from fifteen
  degrees up, and the same bearings again from fifty-five. The runtime picks by
  the angle the camera makes with *that* enemy and lays the quad back to match,
  which is what keeps a crowd seen from a wall or a ledge from being a field of
  flanks foreshortened into slivers. Two rather than three because a tier
  doubles the atlas — 32 MB a kind — and two already keeps every sprite within
  twenty degrees of a baked picture.

  Both the angle and the lean are measured in the **model's own frame**, which
  is what makes the same two tiers serve the crawlers: an ant on a wall is
  standing on the wall, so a player facing that wall is looking down on the ant
  and gets the steep pictures, drawn on a quad lying in the wall rather than
  standing out of it. Every slope between flat and vertical comes out of the
  same arithmetic, and there is no threshold anywhere that says what counts as
  steep.

  The baker runs `drawing()` — the game's own post-update chain — rather than a
  copy of it, and that is not tidiness. It was a copy once, missing the
  billboard half, so the scuttlebug's three billboard
  joints baked at the quarter scale the exporter puts on the skeleton instead of
  having it put back by `billboard::aim`, and single-sided, so they were culled
  from half the angles too. Sprites covered **52% of the pixels the models did**
  and enemies visibly shrank as they crossed the swap distance. Sharing the
  chain took that to 95%. Nothing about it moves the silhouette — the face sits
  inside the head, and the survey extents are identical to four decimals — so no
  test on the sheets can catch it; only running the same code can.

  The bake is two passes: a survey at a fixed camera size to find how big the
  actor gets, then the sheet, sized to what the survey saw. An actor bigger than
  the survey camera does not fail that first pass, it *saturates* it — every cell
  comes back full, the crop is read as the measurement, and `world_size` becomes
  a lie that the runtime then sizes its quads from. `survey_fits` notices the
  bounding box touching the cell edge and looks again from twice as far back.
- **MSAA is off** on the world camera. It is not off by default: Bevy registers
  `Msaa` as a required component of every `Camera` and the default is
  `Sample4`, so a camera that says nothing runs four-times multisampling. On
  this renderer that measured 18% of the frame, and it buys nothing on vertex-lit
  geometry that is then resampled nearest-neighbour onto the window.
- **Culled enemies have their animation stopped, not paused.** Pausing saves
  nothing at all: `bevy_animation` checks the paused flag only to skip the
  clock and the event triggering, and still samples every curve and writes
  every joint every frame — which also keeps the transform propagator's
  dirty-tree optimisation permanently defeated.

### Keeping fewer entities

An enemy is not one entity. It is a glTF scene — about 9 for a slime, 33 for
an ant, and 63 for the scuttlebug the ant replaced — and every one of those is a
transform to propagate and an archetype row to walk past. A mixed field of two
thousand used to be 85,000 entities and eight thousand was a third of a million,
at which point the entity
count, not the draws and not the AI, is what a frame is made of. That was
measured: at eight thousand the simulation budget below saved nothing at all,
because simulation was no longer the expensive part.

So past `enemy_draw` an enemy sheds its `WorldAssetRoot` and the whole scene
goes with it, leaving one entity holding a transform — which is all its
impostor needs. Eight thousand enemies is now 51,000 entities rather than
336,000, and 39 ms a frame rather than 58. Crossing back rebuilds the model;
the boundary has hysteresis so something ambling along it cannot build and
destroy an actor on alternate frames.

### Simulating fewer of them

`sim_budget` (default 200) is how many enemies are simulated properly, and it
is a **count rather than a distance** — a fixed amount of CPU whether the field
holds fifty or five thousand. The nearest that many get level collision, the
jostling in `enemy::spread`, the aggro chain in `enemy::alert` and the ability
to be hit. Everything else is a *crowd* tier moved by `flow.rs`.

The flow field is where the cheap tier's believability comes from. The castle
is surveyed once into a 96×96 grid — each cell asked what the ground under it
is, whether anything can stand there, and which of its eight neighbours have a
wall between them — and a few times a second a breadth-first sweep runs out
from the player's cell recording how far every cell is from him and which way
to walk to get closer. A crowd enemy then costs
four array lookups and no level queries at all, and gets *better* behaviour for
it: a route swept over connected ground flows round the moat and up the ramps
instead of marching into the water.

Four things about that tier are worth knowing, because all four were got wrong
first and none of the failures looked like what it was:

- **Ground on both sides of a step is not the same question as a step being
  takeable.** The first version checked only that the far side had ground —
  which is true on both sides of a fence, and true at the top of a wall as well
  as the foot of it. On the castle that is 2,261 cliff edges out of 23,223, and
  134 more with a wall across them: nearly a tenth of the map was somewhere a
  crowd enemy could walk up at `CLIMB_SPEED`. The worst of a field of ninety-six
  climbed **30 metres in thirty seconds**, straight up the castle. The survey
  now casts between neighbouring cell centres at knee height, with the same
  probe and the same steepness threshold `enemy::walk` uses, so the two tiers
  share one idea of what a wall is. 3.7 ms once at startup.
- **The rule lives in one place.** The sweep, the pass that turns step counts
  into directions, and the enemy taking an actual step all go through one
  `passable`. Three copies of it is a field that routes a crowd somewhere its
  own members then refuse to walk, and a stream that jams against an invisible
  line.
- **Height is interpolated between cells, not taken from the cell.** Taking the
  cell's own height stands every enemy in it at the height of that cell's
  centre, so on a slope half are buried and half float — 0.46 m of average
  error against a slime 0.7 m tall, which reads as the crowd flickering in and
  out of the hillsides.
- **The alarm spreads as a wave.** `enemy::alert`'s shouting chain cascades
  through a dense field within a tick or two, which is why a fully simulated
  crowd ends up converging on the player almost entirely. The cheap tier cannot
  afford the chain, so the flow field carries a single expanding radius
  instead. Without it the far crowd ambles about while the near crowd charges,
  and the field looks like it has lost most of its enemies.

The rule underneath all of them: **a cheaper tier may look worse, but it must
not behave differently.** An enemy must not change its mind about chasing you
because it crossed a boundary it cannot see.

### One answer to how bodies avoid each other

`enemy::spread` holds every creature out of every other one: every enemy in the
field, the Marios and the player, in one pass over one list. It did not always —
the Marios were held out of *nothing*, because `squad::move_allies` walks each
one to its slot and asks nothing about what is standing there, so a squad
following the player was a heap of Marios in the same place walking through each
other and through him.

They share a pass rather than having one each because a Mario, a slime and an
ant are all bodies of some radius standing on some ground, and two
answers to how bodies avoid each other is two answers that disagree at the
boundary between them.

Three things it does that are not obvious:

- **The player is in the list and is never pushed.** Everything else is held
  out of him; he is driven by the controller. Being nudged about by the crowd
  you are walking through is a worse bug than the one being fixed. A pair
  against something immovable takes the whole share of the overlap rather than
  half, or it is walked through.
- **The shove resolves its own result against walls.** A press leaning on the
  one at the front is enough to post it through a fence, and neither the walk
  step nor the crowd step gets a say in where a shove puts it. Not for
  crawlers: an ant is held out of walls by being *on* one.
- **Crawlers are pushed within the surface they are stuck to.** Shoving a bug
  off its wall is the one thing this must not do.

### Every reach and approach is measured from the body, not the centre

`spread` holding two bodies apart is only half a rule. The other half is that
**nothing may be told to walk somewhere the shove will not let it stand.** A
chaser aimed at a body's centre is aimed inside it: the AI walks it in every
tick, the shove pushes it out every tick, and what that looks like is a Mario
standing inside an ant, vibrating. Two systems, two answers, and the argument
runs for as long as the fight does.

Every distance of this kind is therefore measured between *surfaces*:

| | was | now |
|---|---|---|
| `enemy::STAND_OFF` — where an enemy waits by its quarry | 1.0 from its centre | both radii + 0.6 |
| `squad::STRIKE_RANGE` — how near a Mario walks to punch | 1.2 from its centre | its radius + a Mario + 0.5 |
| `enemy::MARIO_REACH` — the punch | 1.6 from its centre | its radius + 1.3 |
| `enemy::ATTACK_REACH` — the player's sword | 2.2 from its centre | its radius + 1.9 |
| `enemy::PLAYER_REACH` — the player's touch | already relative | unchanged |

`Aggro::room` carries the target's radius so a chaser knows how wide the thing
it is chasing is; `alert` fills it as it fills `Aggro::at`, because
`enemy::update` walks the field in parallel and must not be reading other
entities while it does.

This was invisible for as long as the actors were decomp goombas 0.6 m across,
which every one of those constants comfortably cleared. The authored ant is
**5 m across**, and at that size the absolute numbers are all *inside* it: a
Mario was ordered to stand 1.3 m within an ant's body, and the player's sword —
2.2 m — could not reach an ant that `spread` correctly held 3.27 m away, so the
Luna could not hit one at all. Measured on a settled field of 200, worst
overlap: Mario/enemy 1.72 m and Mario/Mario 0.52 m before, **0.00 m** for both
after, with three pairs of the two hundred in light contact.

`enemy::tests::nothing_walks_to_a_spot_it_would_be_shoved_out_of` checks each
approach distance against the room the shove will insist on, for every pair of
kinds, and checks that a weapon still reaches what its owner is allowed to stand
next to. It measures against the widest actor in `KINDS` rather than a named
one, because the next re-export is allowed to change which that is.

The geometry that follows is real and not a bug: sixty creatures 5 m wide
cannot all stand next to one player, so they form a ring several deep and the
back of it waits some way out. That is the authored size talking, not the
spacing.

### Spacing the whole field rather than the near tier

For a while this was the near tier only — two hundred creatures standing
politely apart while eighteen hundred stacked into each other — on the argument
that two distant slimes in the same spot are two pixels in the same spot. They
are, and a thousand of them are a crowd that has visibly collapsed into a heap,
with bare lawn around it. Photographed from above at `crowd 800`: a tangle in
one corner of the courtyard before, an evenly settled field after. An enemy
promoted out of a stack also arrives already inside its neighbours.

What made spacing all of them affordable is three techniques from
`reference/potatoe.md`, and it is worth knowing what each one was actually worth
here, at 2,000 enemies, measured on the same machine state:

| `enemy::spread`, per fixed tick | ms |
|---|---|
| whole field, hashed cells and a `Vec` per bucket, no stride, `sqrt` per pair | 0.438 |
| + flat spatial hash (counting sort into two arrays) | 0.223 |
| + `CROWD_SPREAD_STRIDE`, a quarter of the cheap tier per tick | 0.183 |
| + squared-distance reject before the `sqrt` | **0.178** |

So: the striding is worth about as much as everything else put together, the
grid rewrite is worth 0.045 ms, and **the squared-distance trick is worth
nothing measurable** — one `sqrtps` is about a dozen pipelined cycles on this
CPU, and the pairs that reach it are the small minority that overlap. It is kept
because it is free and reads no worse, not because it bought anything. Somebody
reading `potatoe.md` and expecting item 4 to be the big one should know it was
item 3.

The frame cost of the whole change at 2,000 is **26.9 → 28.0 ms**, of which only
0.18 ms is the system: the rest is that a crowd occupying its real footprint puts
more of itself inside `enemy_draw` (8,661 → 8,995 entities). At 8,000 the system
costs 0.7–1.0 ms of a 48 ms frame.

The hash has one hazard that direct cell addressing does not, and it is worth
stating because nothing about the code makes it visible: **two of the nine cells
a query reads can share a bucket**, and reading that bucket twice returns
everything in it twice. A shove counting one neighbour double would push half
again as hard, on some ticks, for some pairs — a twitch, and near-impossible to
trace back. `Neighbourhood::near` deduplicates the nine bucket indices before it
reads them, and
`enemy::tests::the_spatial_hash_never_hands_back_the_same_body_twice` uses two
thousand points because a nine-cell query collides about once in a hundred at
that size: a smaller test would pass on a broken grid.

A cheap-tier shove resolves against `flow::FlowField::clear` rather than
`LevelData::resolve_walls`, for the reason the tier does everything else that
way — one idea of what a wall is, shared with the step that has to live with the
result.

### Where an enemy's feet are

**A transform is not where the model is.** The scuttlebug's rig root sat up
inside its body, so its geometry hung 31 cm below its own origin. Seat that
origin on the ground — which is what every placement in the game does — and a
third of the bug was underground on flat stone. The goomba hung 6.5 cm, and the
first ant 22 cm.

**Both actors that ship now are the case this wants**, and their lifts are zero:
they are authored with their meshes on `y = 0`, so an origin already is its
feet. That is the fix this always wanted — one origin moved in Blender beats a
correction carried through every placement in the game — and the mechanism stays
for the next actor that arrives without it.

`Kind::lift` is measured off the model, like the cylinder above it, and for the
same reason: the constant it replaced outlived the model it was measured from. A
re-exported ant that had *stopped* hanging spent a session floating a third of
its own height above the ground on a stale 0.216 nobody had re-measured.

`tools/measure_actor_hang.py` is the authority when the two disagree. `lift`
reads the bind pose, which is what a glTF's POSITION accessors hold; the tool
evaluates the *skinned* mesh on every frame of every clip, which is what tells a
permanent rig offset from a squash that dips through the floor plane for a few
frames of a walk cycle.

**The baked sheets are not that authority, and it is worth knowing why.** A
sheet cell is a silhouette drawn by a camera tilted 15 degrees down, so the near
rim of a wide body projects below where its own origin projects even with
nothing at all below it in world space — half a metre of body reaching toward
the camera buys 13 cm of that. It was the right instrument for the scuttlebug,
which was tall and narrow, and it overstates both actors that ship now. So
`impostor::tests::the_lift_matches_what_the_baked_sheets_show` asserts only what
survives that: a lift never exceeds what the silhouette reaches, because a model
held further up than its own picture extends is a model hovering.

`walk`, `settle` and `crawl` all keep answering in **contact points** — where the
feet go — and `enemy::update` converts at the boundary. Keeping them pure means
a test can walk an enemy across a level without knowing what it looks like.

**The sheets and the models are checked against each other**, by
`the_sheets_agree_with_the_models_they_were_baked_from`. It is the only thing
that puts the two ways of measuring an actor in the same room: `Kind::body`
reads a glTF header and never renders anything, while a sheet's `world_size` is
what the bake camera had to cover to fit the actor on screen — the renderer's
answer, through skinning, billboards and every node transform in the file. It
catches the ordinary mistake (resize an actor, re-export, forget to re-bake:
drawn at the new size up close and the old size past `enemy_draw`) and the
strange one (something in the file scaling what is drawn but not what the
bounds say).

Two related things are worth knowing about a crawler's `up`. Hanging under a
ceiling and being buried under a floor are the *same state* — surface overhead,
body against its underside, `up` pointing down — so nothing local can tell them
apart and nothing local can undo the second. A bug that has been moved without
being told which way is up clings to the underside of the lawn forever. The
crowd tier stands crawlers upright, which is the only thing in the game that
rescues one, and makes it self-healing: walk far enough away and the bug is put
back on its feet.

And `up` is **rolled, not snapped**: `ROLL_RATE` caps it at a degree a
millisecond, so a right-angle corner takes three ticks rather than none. The
probe's normal used to become the bug's `up` the same tick it was found, which
reads as the model glitching rather than as an animal climbing. Note that the
lift has to be applied and removed along the same axis — the bug's own `up`, not
the surface's — or the mismatch leaves a residue every tick: a bug going round
the castle's corners drifted 1.2 m in one step that way.

`squad::move_allies` still has no wall collision of its own, so a Mario walks
through the castle unless the shove happens to catch it.

### The rest

Collision queries go through a 64×64 grid rather than the full triangle list,
enemy AI and floor placement run at the 30 Hz simulation rate rather than the
render rate, distance LOD drops far AI to 15 or 7.5 Hz, and Bevy spreads the
enemy step across its compute pool.

Every threshold is a console tunable, which is the point — the right values
depend on the machine, and guessing them from source is how the previous set
got chosen.

The remaining dial is the internal render resolution above, which scales the
fragment side of the frame rather than the draw-call side, so it is the one
that helps when the crowd is small and the window is large.

### The packaging trap

`build_windows.sh` copies a **named list** of assets rather than the whole tree,
deliberately — the sound directories hold thousands of files and the game plays
a couple of dozen. The cost of that is a failure mode with no symptoms at the
point it happens: an asset the game loads but the script does not copy produces
a packaged build that starts, runs, and is quietly missing something, saying so
only on a stderr that a `windows_subsystem = "windows"` build has nobody
attached to.

It has now happened three times. `display.rs` guards the UI render plugin
against it with a compile-time trick; the impostor sheets were caught only after
they had shipped, so the packaged game drew *no distant enemies at all* and
enemies appeared out of nothing as you walked towards them; and the planet was
caught by the person playing it — the packaged build had the level in its menu
and no glTF for it, so choosing it loaded nothing, put the castle back and shut
the menu, which from the outside is a menu row that does nothing. Every test
passed each time, because tests read the source tree.

Two guards came out of that, and anything new under `assets/` wants both:

- a test that reads `build_windows.sh` and asserts the asset will be copied
  (`impostor::tests::the_windows_package_ships_the_sheets`,
  `world::tests::the_windows_package_ships_every_level`)
- a count on the corner readout of enemies drawn by *neither* path, so a missing
  atlas shows up in the game as `… / 704 UNDRAWN` rather than as a mystery

Where the script can derive the list from the source rather than repeating it,
it does — the weapons, the sound samples and now the levels are all grepped out
of the module that names them, because a list that exists once cannot drift. A
level that fails to load also says so on the pause menu now, which is the same
lesson applied to the runtime: the report used to go only to the console, and
nobody has the console open at the moment they choose a level.

### Measuring it

`crowd 2000 mix` in the console puts a whole reproducible field down at once
rather than waiting for the pipes to fill it; `crowd clear` takes it away. The
corner readout carries the enemy and entity counts beside the frame time,
because an enemy is not one entity but a whole scene of them — a mixed field of
two thousand is about 85,000 entities, and that multiplier is most of what the
crowd work is fighting.

For a repeatable number with no window in the way:

```bash
cargo test --release -- --ignored --nocapture crowd_benchmark
```

which runs the real game headless against a real GPU and prints a row per field
size. `CROWD_BENCH=2000` runs one size instead of the sweep, `CROWD_DRAW=60`
overrides the impostor swap distance and `CROWD_SIM=2000` the simulation
budget — between them that is enough to A/B a single change. `--features perf` turns on Bevy's own per-system tracing and
writes a Chrome trace; the systems that matter are mostly Bevy's own
(`animate_targets`, `propagate_parent_transforms`, the render-phase queues) and
cannot be timed from this crate at all.

Comparing a sprite against the model it replaces is a two-shot job, because
`enemy_draw` changes only what is drawn and never what is simulated: render the
same field at `enemy_draw 5` and `enemy_draw 900` with the same `SHOT_SETTLE`
and the enemy positions are identical, so the frames can be differenced
directly. Counting enemy-coloured pixels in each is what turned "the pop-in is
bad" into "sprites cover 52% of what models do", and then into a fix.

`cargo run --release -- screenshot out.png [crowd] [x,y,z] [look x,y,z]` draws
the real game into a PNG without a window, which is the only way to see it at
all on a machine with no display. `SHOT_SETUP="enemy_draw 12; sim_budget 50"`
runs console commands before the shutter and `SHOT_SETTLE=300` chooses how many
frames the world runs first — which is what catches behaviour that only
diverges over time, as the crowd tier's did.

## Not done yet

This is an early playable milestone, not frame-exact parity with the SM64
action machine. Ported: the castle with its floor, wall and ceiling collision,
core traversal, the camera, both avatars, representative actors, the water
surface and underwater view, sound events, named animation clips, the Mario
squad, and keyboard/mouse/gamepad input. Camera collision, combat hit
resolution, health, warp-pipe spawning, water movement and the tuning console
have playable first-pass implementations.

Still to port:

- the complete SM64 move set — dives, slides, ledge grabs, wall bonks and the
  rest of the action machine
- the thrown-arc preview the squad aim used to draw
- the crowd tier's remaining behaviour gap. A tiered field converges on the
  player less hard than a fully simulated one does — measured at roughly 60% of
  the on-screen crowd after five seconds — because a spreading alarm radius is
  an approximation of a chain whose reach depends on how densely packed the
  crowd happens to be. `SHOT_SETTLE` and the pixel-counting comparison in the
  performance notes are how that was measured and how a fix would be judged.
- atlasing the actor exports. This was worth a seven-fold cut in draw calls
  when the scuttlebug was fifteen mesh primitives and nine materials for
  seventy-six triangles. Replacing both decomp enemies with authored art took
  most of it already — a slime is two primitives and an ant is one — so what is
  left of the idea belongs to Mario and Luna rather than to the enemies.

See `next.md` for what is wanted next.
