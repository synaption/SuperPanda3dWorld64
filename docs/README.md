# Documentation wiki

This wiki contains the detailed documentation for Space Crusaders. Start
with the page that matches what you want to do:

## Getting started

- [Project overview and quick start](../README.md) — what the project is and
  how to build and launch it.
- [Building and running](project-guide.md#building-and-running) — the
  toolchain, the optional sound and gamepad features, and the Windows
  cross-build.
- [Controls](project-guide.md#controls) — keyboard, mouse, and gamepad input.
- [Pause menu, levels and render resolution](project-guide.md#the-pause-menu-and-the-internal-render-resolution)
  — Escape, choosing a level, the display settings, and how the world is drawn.
- [Debug console](project-guide.md#the-debug-console) — in-game inspection and
  live tuning.

## Gameplay and design

- [The level](project-guide.md#the-level) — the castle grounds and the planet,
  and how collision is filed on each.
- [Gravity](project-guide.md#gravity) — flat down, down towards the middle of a
  planet, and what had to change to have both.
- [Timing and the port](project-guide.md#timing-and-the-port) — the fixed 30 Hz
  tick, latched input, and the original collision quirks that are deliberately
  *not* kept.
- [Water](project-guide.md#water) — the surface sheets, the underwater view,
  and why the two characters treat deep water differently.
- [The squad](project-guide.md#the-squad) — selection, commands, and filling
  the field with AI Lunas as well as Marios.
- [Machines and the pylon network](project-guide.md#machines-and-the-pylon-network)
  — building stellarators, planting pylons, and the graph search the network
  shares with the crowd's pathing.
- [Warp pipes](project-guide.md#warp-pipes) — what each produces, and the arc
  it is thrown out on.
- [Combat](project-guide.md#combat) — hit resolution and the immunity window.
- [Billboards](project-guide.md#billboards) — turning flat quads to face the
  camera, at object and at joint level.
- [Aiming and attack animation design](aim.md) — planned animation layering,
  aim correction, and weapon behaviour. Design only; the procedural layer is
  not built in this engine yet.

## Development reference

- [Project guide](project-guide.md) — the complete technical reference,
  including project layout, architecture, and porting notes.
- [Asset pipeline](pipeline.md) — what is committed under `assets/`, how to
  regenerate it, and the actor and Luna export workflows.
- [Assets and provenance](pipeline.md#what-is-committed) — what is bundled,
  where it came from, and the notice to read before publishing.
- [Exporting actors](pipeline.md#exporting-actors) and
  [Exporting Luna](pipeline.md#exporting-the-luna) — the two export paths
  and why each step exists.
- [Bevy 0.19 upgrade notes](bevy-0.19-upgrade.md) — what the engine upgrade
  changed and what it broke.
- [Performance](project-guide.md#performance) — collision partitioning, AI
  rates, and crowd LOD.
- [Not done yet](project-guide.md#not-done-yet) — what is still unported.

## Contributing to the wiki

Keep the root README brief. Put detailed design, workflow, and technical
reference material in this directory, then add the new page to this index.
