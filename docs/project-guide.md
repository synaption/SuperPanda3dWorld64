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

The character you play is **the Hero**, a rigged model with twenty
hand-authored clips of his own. He is not Mario in a different costume: rather
than retarget Mario's 209 animations onto him, he has his own action machine
built around the moves he actually has — walk, run, jump, a two-hit sword
chain, a spin kick out of a run, skates on the ground and flight in the air.
What he shares with Mario is everything below the neck: the same level, the
same collision, the same quarter-step movement, and the same 30 Hz tick.

Mario is still here. He wanders the field as an NPC, **F2** hands control back
to him at wherever the Hero is standing, and the field can be filled with more
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
dist/windows/SuperBevyWorld64.exe
dist/SuperBevyWorld64-windows-x64.zip
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
  level.rs       the level blob: geometry, collision, the 16x16 query grid
  player.rs      both characters' controllers, the water medium, combat state
  camera.rs      the third-person aiming camera: look, follow, boom, occlusion
  input.rs       keyboard, mouse and pad merged into one latched snapshot
  animation.rs   which clip each character plays, and how fast
  billboard.rs   turning billboarded objects and actor parts to face the camera
  enemy.rs       goombas and scuttlebugs: behaviour, LOD, hit resolution
  pipe.rs        warp pipes, and the arc they throw their brood out on
  squad.rs       aiming at the ground, and the Marios whistled up and sent out
  water.rs       the water sheets and the underwater view
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
| Attack | Left Shift or left mouse | east (B) |
| Skate on the ground, fly in the air (Hero) | hold `V` | hold north (Y) or left trigger |
| Aim | hold `F` or right mouse | right trigger |
| Recenter camera | `R` | right shoulder |
| Squad: hold to whistle, tap to send | `X` | west (X) |
| Switch Hero/Mario in place | `F2` | Start |
| Pause menu (pauses the simulation) | `Escape` | Select |
| Tuning console (pauses the simulation) | `` ` `` | — |
| Debug text | `F1` | — |
| Window/fullscreen | `F11` | — |

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

**Quirks are kept deliberately.** Collision queries take the *first* triangle
in list order that passes rather than the best one, which reproduces surface
cucking, where a lower triangle shadows a higher one. Wall pushback tests every
wall against the entry position while accumulating the output, so overlapping
walls each push by their full amount. These are not bugs to fix; changing any
of them changes how the game plays.

**Distances are the original's, converted.** The port works at 1/100 of SM64's
scale, so a constant that reads 8 units per frame in the decomp is 2.4 units a
second here. The conversions are written out where they appear rather than
hidden in a helper, because the frame-rate half of them is as easy to get wrong
as the scale half.

## The level

The castle grounds arrive as one blob, `assets/bevy/castle.bin`, embedded in
the binary with `include_bytes!` — so the game needs neither Python nor numpy
to start, and the level cannot go missing. It carries the render mesh, the
collision triangles and their surface types, the water boxes and the tree
placements. `assets/bevy/castle.glb` is the same level as textured geometry.

Static collision is partitioned into a 16×16 X/Z grid built at load time, so
floor, wall and camera queries inspect only nearby triangles instead of all
879.

Castle walls occlude the camera and block actors. The warp pipes are drawn but
not collided with — the level's own collision is all the physics reads, and the
actors' `collision.inc.c` is not loaded, so you can walk through one.

## Animation

Clips are chosen **by name** rather than by export index. The Hero's names are
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

Water is not part of the level mesh. It is the axis-aligned boxes the collision
data carries, drawn as one flat quad at each box's height: unlit, half
transparent at the original's 0x96 alpha, and two-sided because most of the
time it is looked at from underneath. Each sheet's texture drifts across the
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
same clips. Mario swims. The Hero does not: he has no swimming animation, so
rather than drag a walk cycle through the water he is held just under the
surface, slowed to 0.45 of his walk, and drawn upright. Water shallower than he
floats in is simply slow ground: the bottom is under his feet and he walks
along it. `hero_wade` in the console is the speed.

## Sound

Gameplay never touches the audio device. The fixed-step systems append typed
events to a queue and a render-rate system drains it, so the simulation runs
identically with no device present and the whole of it stays testable headless.

Each event resolves to a stack of layers that play together, which is what SM64
itself does for a jump — a terrain sound from the ground and a voice from
Mario. The Hero speaks with the Zelda voice set and steps with its effect set;
Mario uses samples imported from an extracted asset tree, because the decomp
ships a sound taxonomy and no waveforms. Jump, landing, footfalls, attacks,
taking damage, defeating an enemy, breaking the water surface, and swim strokes
are wired. `sfx_volume` in the console sets the level.

The tables name their `.wav` files by hand, and a test walks every name and
asserts the file is in the repository — which is what catches a typo or a
sample renamed out from under them. `build_windows.sh` reads the same names out
of the source to decide what to package.

## The Marios

The field is full of Marios, and they are the squad. One button carries two
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

**Part of an actor** — a goomba's face, most of a scuttlebug — is skinned to a
joint, where no transform on the object can reach it. The exporter makes each
such quad a joint named `billboard_*`, and those are driven one at a time.

Driving a joint takes two things that are easy to get wrong. The rotation is
composed against the inverse of the parent joint's world rotation, because the
skeleton leaves the quad rotated a quarter turn and a heading applied on top of
that comes out as pitch — on the goomba the parent sits at `(98.4, 4.9, -90.7)`,
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

Three pipes, and each produces one thing: goombas out of the one in the far
west corner, scuttlebugs out of the one in the far east, and Marios out of the
one by the spawn on the castle path. The two enemy pipes are where they are on
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
tick — a goomba bleeds whatever it has back toward a walk, a scuttlebug simply
overwrites it with its crawl — so a launch handed to the behaviour is gone
within a tick or two and the thing lands back on the pipe it came out of.

Where it is thrown is chosen rather than taken: headings a golden angle apart
are tried until one has floor at the end of it that is not far below the pipe.
The west pipe stands a few hundred units from where the ground falls into the
moat, and without this a quarter of its goombas came down in the water. The
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

Sizes come from the originals' hitboxes rather than from eye. Mario's is 160
tall, a regular goomba's is 50 scaled by 1.5, and a scuttlebug's is 70, so they
stand at about 0.47x and 0.44x his height.

The immunity after a hit gates the *whole* resolution and not only the damage.
That is not a detail. A knocked-back player is thrown up and off the enemy that
hit him and comes back down on its head, so without it every enemy that touches
somebody standing perfectly still stomps itself within a couple of seconds —
and a warp pipe whose every goomba destroys itself before you turn round is a
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

Escape pauses the game and opens a menu — Resume, Options, Quit — with the
display settings one page in. Escape goes back a page and closes it from the
root, so the key that opened it is the key that gets out of it whatever page
you wandered into. It is keyboard-driven (arrows or `WASD`, Enter, and
left/right to change a value) and the pad drives the same rows with the d-pad,
south and east; Select opens it, because Start already swaps character. Opening
it hands the mouse cursor back and resuming captures it again.

**The world is not drawn to the window.** It is drawn into an image, and a
second camera stretches that image over the window with a nearest-neighbour
filter — which is what makes the render-resolution row possible. The row is a
percentage of the window rather than a fixed height like 240p, so the internal
image always has the window's own aspect ratio and the stretch is uniform;
`src/display.rs` shows what it works out to in pixels beside the percentage.
25% of a 1080p screen is 480×270, which is close to what the console actually
put out.

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

Static collision queries go through the 16×16 grid rather than the full
triangle list. Enemy AI and floor placement run at the 30 Hz simulation rate
rather than the render rate, and Bevy distributes enemy transforms across its
compute pool.

Distance-based crowd LOD lowers far AI to 15 or 7.5 Hz and culls distant
skinned models and their skeletal animation work. All three thresholds are
exposed through the console, which is the point — the right values depend on
the machine, and guessing them from source is how the previous set got chosen.

The other dial is the internal render resolution above, which is the one that
scales the fragment side of the frame rather than the CPU side.

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
- impostor crowds. `assets/impostors/` still holds baked sheets for the goomba
  and the scuttlebug; the tool that baked them was Panda3D-based and went with
  it, so reviving these means a Bevy baker.

See `next.md` for what is wanted next.
