# Super Bevy World 64

A native Rust/Bevy port of the playable core of Super Panda3D World 64. It
loads the repository's castle, Hero, Mario, trees, pipes, goombas, and
scuttlebugs and implements a fixed 30 Hz controller, third-person aiming
camera, character switching, jumping, Hero skating/flight, and basic enemies.
Enemies can be attacked or stomped, damage the player on contact, and emerge
from nearby warp pipes. Castle walls occlude the camera and block actors.

## Build and run

This port targets Bevy 0.12 so it works with the repository environment's Rust
1.75 compiler.

```bash
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

The script keeps its matching Rust 1.75 cross-toolchain under the ignored
`target/` directory and produces:

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
control at bottom right. Left/Right adjusts it while the console is open;
brackets (`[`/`]`) adjust it during play; Shift adjusts either 10x faster.
`close <name|all>` removes pinned controls. Up/Down recalls command history and
Tab completes a unique variable name. The panel also reports live FPS, player
state and position, health, and enemy count. The mouse wheel or PageUp/PageDown
scrolls through the bounded command log.

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

## Port status

This is an early playable Bevy milestone, not frame-exact parity with the
Panda3D/SM64 action machine. The castle with its floor, wall and ceiling
collision, core traversal, camera, both avatars, representative actors, sound
events, and keyboard/mouse/gamepad input are ported. Still to port are the
complete SM64 move set, squads, and impostor crowds. Skeletal clip selection,
camera collision, combat hit resolution, health, warp-pipe spawning, water
movement, and the tuning console have playable first-pass implementations.

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
