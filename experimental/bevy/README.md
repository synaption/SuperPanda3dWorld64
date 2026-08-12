# Super Bevy World 64

A native Rust/Bevy port of the playable core of Super Panda3D World 64. It
loads the repository's castle, Hero, Mario, trees, pipes, goombas, and
scuttlebugs and implements a fixed 30 Hz controller, third-person aiming
camera, character switching, jumping, Hero skating/flight, and basic enemies.
Enemies can be attacked or stomped, damage the player on contact, and are
thrown out of the warp pipes. Castle walls occlude the camera and block actors.

## Build and run

This port targets Bevy 0.19, which requires Rust 1.95 or newer. The distro
compiler on `PATH` here is 1.75 and will not do; the rustup toolchain alongside
it is current and carries the Windows cross-target as well, so put it first:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd experimental/bevy
cargo run
```

Sound and gamepad support build against ALSA and udev on Linux, which the WSL
environment here does not have headers for, so on Linux they are opt-in and a
plain `cargo run` is silent and keyboard-only. With `libasound2-dev` and
`libudev-dev` installed, ask for them:

```bash
cargo run --features sound,gamepad
```

The Windows build always has both: those backends are part of the OS there, so
`build_windows.sh` needs no extra packages and the packaged game has sound and
pad support out of the box.

Run `./run_bevy.sh` from the repository root to launch the game. It uses the
packaged executable under Git Bash/MSYS or WSL and the current Rust source on a
native Unix host.
Pass `--source` or `--packaged` to select either path explicitly.

### Build the Windows executable from Linux/WSL

Install a MinGW-w64 cross-compiler, then run:

```bash
cd experimental/bevy
./build_windows.sh
```

The script adds the `x86_64-pc-windows-gnu` standard library through rustup if
it is missing and produces:

```text
dist/windows/SuperBevyWorld64.exe
dist/SuperBevyWorld64-windows-x64.zip
```

The ZIP includes the runtime GLB assets and is the file to copy to Windows.

Development builds load GLBs from the root `assets/` directory. Packaged builds
can instead place that directory beside the executable. `assets/castle.bin` and
the root-level `assets/bevy/castle.glb` are
native conversion of the existing NPZ castle mesh and collision data. Regenerate
it after changing the source level with:

```bash
python3 experimental/bevy/tools/convert_level.py
```

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
| Tuning console (pauses the simulation) | `` ` `` | — |
| Debug text | `F1` | — |
| Release/capture the cursor | `Escape` | — |

Landing on an enemy defeats it whichever character is active. A pad plugged in
mid-game is picked up without a restart, and the pad's sticks use the same
circular deadzones as the Panda3D build (0.18 to move, 0.12 to look) so a
gentle diagonal is still holdable.

## Debug console

Backquote opens a command console and pauses gameplay and skeletal animations.
`vars` lists every live value; `<name> <value>` changes one immediately;
`reset <name|all>` restores defaults. Entering a name without a value pins its
control at bottom right; brackets (`[`/`]`) adjust it, open or closed, and Shift
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

Movement, camera, water, enemy, and spawning constants are backed by the tuning
resource rather than copied at startup, so console changes apply on the next
gameplay tick.

## Sound

Sound follows the Panda3D build's split, for the same reason: gameplay never
touches the audio device. The fixed-step systems append typed events to a
queue and a render-rate system drains it, so the simulation runs identically
with no device present and the whole of it stays testable headless.

Each event resolves to a stack of layers that play together, which is what
SM64 itself does for a jump -- a terrain sound from the ground and a voice
from Mario. The Hero speaks with the Zelda voice set and steps with its effect
set; Mario uses the placeholder samples `sm64py/audio.py` synthesises, because
the decomp ships a sound taxonomy and no waveforms. Jump, landing, footfalls,
attacks, taking damage, defeating an enemy, breaking the water surface, and
swim strokes are wired. `sfx_volume` in the console sets the level.

## Water

Water is not part of the level mesh. It is the axis-aligned boxes the
collision data carries, drawn as one flat quad at each box's height: unlit,
half transparent at the original's 0x96 alpha, and two-sided because most of
the time it is looked at from underneath. Each sheet's texture drifts across
the world at a fixed speed, and the two bodies drift in different directions
so they do not read as one sheet.

A box is a plain rectangle laid over a bay of the map, so each also covers dry
ground that rises through the sheet — the moat is the part that reads as
water.

Under the surface the view closes in hard and goes green-blue. That is what
sells being submerged far more than the surface quad does, since the water is
a single flat sheet with nothing behind it. The medium is chosen by where the
*camera* is, not the player: swimming just below the surface leaves the camera
in open air looking down through it, and tinting the world in that case looks
wrong.

`assets/bevy/water.png` is copied out of the reference texture pack by
`tools/convert_level.py` and committed, like the converted castle.

Deep water is not one behaviour, because the two characters do not have the
same clips. Mario swims. The Hero does not: he has no swimming animation, so
rather than drag a walk cycle through the water he is held just under the
surface, slowed to 0.45 of his walk, and drawn upright — `act_wading` in
`sm64py/hero/actions.py`, which is where the numbers come from too. Water
shallower than he floats in is simply slow ground: the bottom is under his
feet and he walks along it. `hero_wade` in the console is the speed.

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
and the lawn fills; set it to 0 and it clears. The Mario pipe's own brood is not
in that count and stays when the lawn is cleared; see Warp pipes.

## Billboards

SM64 draws some things as flat quads it rebuilds every frame to point at the
camera. glTF has no billboard concept, so they arrive as ordinary geometry
drawn from whatever one side they were authored on — and a quad is flat, so
from ninety degrees away there is nothing there at all. The tree mesh measures
exactly zero thick.

There are two cases and they need different machinery, which is the same split
`sm64py/billboard.py` and `sm64py/level.py` arrived at. A **whole object** —
the trees — is plain geometry and turns bodily, about the vertical only, so it
never tips over when the camera looks down at it. **Part of an actor** — a
goomba's face, most of a scuttlebug — is skinned to a joint, where no transform
on the object can reach it; the exporter makes each such quad a joint named
`billboard_*`, and those are driven one at a time.

Driving a joint takes two things that are easy to get wrong. The rotation is
composed against the inverse of the parent joint's world rotation, because the
skeleton leaves the quad rotated a quarter turn and a heading applied on top of
that comes out as pitch. And the aiming runs after the animation player has
posed the skeleton and before transforms are propagated: before the animation
player, every joint written is overwritten a moment later and nothing turns at
all.

Billboarded surfaces are drawn from both sides. Which face was authored toward
the viewer is not something this port can see, and aimed the wrong way round a
single-sided quad would be invisible from *every* angle rather than from half
of them.

## Warp pipes

Three pipes, and each produces one thing: goombas out of the one in the far
west corner, scuttlebugs out of the one in the far east, and Marios out of the
one by the spawn on the castle path. Ported from `PIPE_SPAWNS` in
`app/main.py`, which puts the two enemy pipes where they are on purpose — they
are somewhere to go rather than something to trip over on the way out of the
gate.

Nothing is placed beside a pipe. It is thrown out of the top: spawned down at
the pipe's feet with an upward velocity, so the launch carries it up through
the barrel and out of the mouth, which is what the pop is. It starts hidden
inside the pipe rather than appearing in mid-air above it. The arc is the
original's — 60 units a frame up against 4 a frame of gravity, so it peaks
twice the height of the pipe's own rim and stays up for a full second, carried
outwards throughout and landing about four pipe-widths away.

For that second the thing's own behaviour is suspended and the arc alone moves
it. That suspension is the trick, and `sm64py/objects.py` says why: every
behaviour writes its own speed each tick — a goomba bleeds whatever it has
back toward a walk, a scuttlebug simply overwrites it with its crawl — so a
launch handed to the behaviour is gone within a tick or two and the thing lands
back on the pipe it came out of.

Where it is thrown is chosen rather than taken: headings a golden angle apart
are tried until one has floor at the end of it that is not far below the pipe.
A pipe standing near where the ground drops into the moat otherwise puts a
share of its brood in the water. The golden angle rather than a random number
means five throws spread around the pipe instead of stacking, with a whole run
still reproducible in a test.

The Mario pipe counts its own brood and nothing else. The standing crowd that
`ally_count` answers for is deliberately outside that count, so changing it
does not despawn the Mario the pipe threw. `pipe_brood` is this ally-pipe quota.
The two enemy pipes instead share `enemy_limit`, which counts every live enemy
on the field, including the five placed by hand; `enemy_rate` is their interval.
Enemy pipes ready on the same tick consume the remaining slots in pipe order,
so the cap is exact rather than occasionally ending one over it.

The countdown runs at any distance, so a crowd is waiting when the player
arrives rather than only starting to fill then. It also only runs while there is
room, holding where it stands at the quota — so a kill starts the clock again
instead of restarting it, which is the difference between one along every so
often and a replacement appearing the instant something dies.

## Combat

The player resolves against every enemy once a tick, ported from
`Interactions.resolve` in `sm64py/objects.py`: a swing defeats what is in front
of him, coming down on one stomps it, and touching one any other way throws him
back and costs a heart. Each enemy is an upright cylinder — a radius and a
height — rather than a point, so standing on a roof above one is not touching
it.

The immunity after a hit gates the *whole* resolution and not only the damage.
That is not a detail. A knocked-back player is thrown up and off the enemy that
hit him and comes back down on its head, so without it every enemy that touches
somebody standing perfectly still stomps itself within a couple of seconds —
and a warp pipe whose every goomba destroys itself before you turn round is a
warp pipe that appears to spawn nothing at all.

## Port status

This is an early playable Bevy milestone, not frame-exact parity with the
Panda3D/SM64 action machine. The castle with its floor, wall and ceiling
collision, core traversal, camera, both avatars, representative actors, the
water surface and underwater view, sound events, named animation clips, the
Mario squad, and keyboard/mouse/gamepad input are ported. Still to port are
the complete SM64 move set, the thrown-arc preview the squad aim draws in the
Panda3D build, and impostor crowds. Camera collision, combat hit resolution,
health, warp-pipe spawning, water movement, and the tuning console have
playable first-pass implementations. Billboards — the trees, and the quads
inside the actors — are turned to face the camera the way the original
rebuilds them, and the pipes throw what they produce out of the top on the
original's arc.

All UI text draws with Bevy's embedded default font, which arrives with the
`default_font` feature rather than automatically. Without it every text node
still lays out and still paints its background while every glyph silently goes
missing, so the console renders as a black bar with nothing written on it and
nothing anywhere reports an error. A test asserts the default font handle
resolves.

Animation clips are chosen **by name** rather than by export index — the
Hero's are the Blender action names and Mario's are `anim_XX` after the
decomp's ids — because names are what both exporters guarantee and an index
shifts silently the moment a clip is added. Walks are played at a rate that
tracks ground speed so the feet do not slide, attacks alternate into a combo,
and standing still long enough plays the idle fidget.

Input is latched rather than polled from inside the simulation. Gameplay runs
at a fixed 30 Hz while input arrives at the render rate, so a frame may hold
two fixed steps or none; a `just_pressed` read inside the fixed step would
jump twice on a slow frame and swallow the press on a fast one. Presses are
recorded once per frame and consumed by the step that acts on them.

Static collision is partitioned into a 16×16 X/Z grid built at load time, so
floor, wall, and camera queries only inspect nearby triangles instead of all
879. Enemy AI and floor placement run at the 30 Hz simulation rate rather than
the render rate, and Bevy distributes enemy transforms across its compute pool.
Distance-based crowd LOD lowers far AI to 15 or 7.5 Hz and culls distant
skinned models and their skeletal animation work; all three thresholds are
exposed through the console.
