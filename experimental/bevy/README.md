# Super Bevy World 64

A native Rust/Bevy port of the playable core of Super Panda3D World 64. It
loads the repository's castle, Hero, Mario, trees, pipes, goombas, and
scuttlebugs and implements a fixed 30 Hz controller, third-person aiming
camera, character switching, jumping, Hero skating/flight, and basic enemies.

## Build and run

This port targets Bevy 0.12 so it works with the repository environment's Rust
1.75 compiler.

```bash
cd experimental/bevy
cargo run
```

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

- `WASD` / arrows: camera-relative movement
- Mouse or `Q` / `E`: look
- `Space`: jump; while skating, booster take-off
- Hold `V`: skate on the ground and fly in the air (Hero)
- Left Shift: attack
- Hold `F` or right mouse: aim
- `R`: recenter camera
- `F1`: debug text
- `F2`: switch Hero/Mario at the current position
- `Escape`: release/capture the cursor

## Port status

This is the first playable Bevy milestone, not frame-exact parity with the
Panda3D/SM64 action machine. The castle and its floor collision, core traversal,
camera, both avatars, and representative actors are ported. Still to port are
wall/ceiling collision, water/swimming, skeletal clip selection, the complete
SM64 move set, combat hit resolution, squads, audio, gamepad input, impostor
crowds, warp-pipe spawning, and the debug tuning console.
