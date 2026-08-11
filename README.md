# Super Panda3D World 64

An experimental Panda3D game and research project that combines a reconstructed
Super Mario 64 castle grounds with two playable movement systems. Play as the
Hero, with original traversal and combat moves, or switch to Mario to compare
against the ported SM64 physics on the same level.

The current build includes the castle grounds, water, enemies, sound effects,
a third-person camera, keyboard/mouse and gamepad input, and a debug console.

## Run the project

Use Python 3. Install the runtime dependencies, then start the game from the
repository root:

```bash
python3 -m pip install panda3d numpy
python3 app/main.py
```

Game assets required at runtime are included under `assets/`; the large
reference sources are only needed when regenerating converted assets.

## Documentation wiki

The [documentation home](docs/README.md) is the entry point for project
guides, controls, asset workflows, and design notes.

- [Project guide](docs/project-guide.md) — gameplay systems, controls, assets,
  architecture, tooling, and verification notes.
- [Aiming and attack animation design](docs/aim.md) — the proposed
  partial-body animation and procedural aiming approach.

## Asset notice

Some assets are derived from Nintendo game data and community texture work.
Review the asset notice in the [project guide](docs/project-guide.md#assets)
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