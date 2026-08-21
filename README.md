# Super Bevy World 64

A native Rust/Bevy game and research project that combines a reconstructed
Super Mario 64 castle grounds with two playable movement systems. Play as the
Hero, with original traversal and combat moves, or switch to Mario to compare
against the ported SM64 physics on the same level.

The current build includes the castle grounds, water, enemies, warp pipes, a
Mario squad, sound effects, a third-person aiming camera, keyboard/mouse and
gamepad input, a pause menu with display settings, and a debug console.

Crowds are the thing it is built around: `crowd 2000 mix` in the console puts
two thousand enemies on the lawn, and past `enemy_draw` each one is drawn as a
baked sprite rather than a skeleton, so the whole distant field costs two draw
calls instead of four per goomba and fifteen per scuttlebug. See the
[performance notes](docs/project-guide.md#performance) for how to measure it.

Escape pauses the game and opens the menu; its display page changes the
internal render resolution, which is the world's own resolution rather than the
window's — the setting that buys frame rate, and the one that puts the
console's pixels back.

## Run the project

Bevy 0.19 needs Rust 1.95 or newer, which is usually the rustup toolchain
rather than the distro compiler:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo run
```

`./run_bevy.sh` runs from any working directory, refreshes the packaged Windows
build, and then launches its executable. Pass `--source` to build and run the
native Rust source instead, or `--packaged` to launch the existing Windows
package without rebuilding it. On Linux, sound and gamepad build against ALSA
and udev and are opt-in: `cargo run --features sound,gamepad`. The Windows
build always has both.

Game assets required at runtime are included under `assets/`; the large
reference sources are only needed when regenerating converted assets. The
asset pipeline is Python — it does not ship with the game.

## Documentation wiki

The [documentation home](docs/README.md) is the entry point for project
guides, controls, asset workflows, and design notes.

- [Project guide](docs/project-guide.md) — gameplay systems, controls,
  architecture, and porting notes.
- [Asset pipeline](docs/pipeline.md) — what is bundled under `assets/`, how to
  regenerate it, and the actor and Hero export workflows.
- [Aiming and attack animation design](docs/aim.md) — the proposed
  partial-body animation and procedural aiming approach.

## Asset notice

Some assets are derived from Nintendo game data and community texture work.
Review the asset notice in the [asset pipeline](docs/pipeline.md#what-is-committed)
before publishing or redistributing the repository.



***
## Inspiration

- Demon Chaos - 65k enemies on screen at a time on PS2
- Pseudoregalia - great movement system, retro nastalgia asthetic
- Pikmen/Sons of Liberty - squad control mechanics
- Outer Wilds - seamless interplanetary travel
- EDF, Dynasty Warriors, Armored Core, Souls - combat, jetpack, dead body physics
- Sonic - Shadow Rocket Boots

## Tech

Nearby enemies — maybe 50–300: real gameplay entities with full AI, collision, skeletal animation, attacks, hit reactions, etc.

Mid-distance enemies — perhaps a few thousand: simplified CPU simulation, very cheap steering/collision, lower animation fidelity.

Huge distant army — tens of thousands: effectively GPU particles that happen to look like soldiers. One or a handful of meshes rendered through hardware instancing, with positions/state held in GPU buffers. Panda3D explicitly supports geometry instance counts and shader buffers.

Very distant units: billboards/impostors or extremely low-poly models.
