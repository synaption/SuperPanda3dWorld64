# SuperPanda3dWorld64

Super Mario 64's castle grounds and Mario's motion system, rebuilt in Python and
Panda3D from the [Render96ex](https://github.com/Render96/Render96ex) decomp.

The point is not the remake. It is to find out whether **Panda3D can carry a
smooth, enjoyable 3D game of the N64 era's caliber** — and SM64 is the fairest
possible test, because the original is exactly documented, so "does it feel
right" has a real answer instead of an opinion.

Walking, slopes, the jump chain, dives, slides, ledge grabs and wall bonks all
run on a port of the original physics. The moat and lake are swimmable. Actions
raise the original sound events and play real samples. The level's trees stand
alongside a few goombas and scuttlebugs you can stomp, punch, and run away from.

## Running

```bash
python3 app/main.py
```

Requires `panda3d` and `numpy`. Everything the game loads is committed under
`assets/`, so a clone runs as-is — no ROM, no decomp checkout, no 12 GB of
reference material.

| | |
|---|---|
| `W` `A` `S` `D` / arrows | analog stick (camera-relative) |
| `Space` | A — jump |
| `Left Shift` | B — punch, dive |
| `Left Ctrl` | Z — crouch, ground pound, long jump |
| `Q` `E` / mouse drag | swing the camera |
| `R` | re-centre the camera behind Mario |
| `F1` | toggle the debug readout |
| `F3` | toggle the collision overlay |
| `Esc` | quit |

## Goals

**Show that Panda3D is a viable engine for this.** It has a lot going for it
that is specifically useful to games — an actor and animation system, a scene
graph built for it, a real collision framework — and the open question was
whether it can hold a steady frame without the microstutter that is easy to get
stuck with. It can. That took finding the causes rather than guessing at them:
vsync was the big one, and a camera that was stepping rather than sweeping was
the one that felt worst. See [Performance](https://github.com/synaption/SuperPanda3dWorld64/wiki/Performance).

**Be faithful enough that "it feels right" is checkable.** Movement runs at a
fixed 30 Hz because every constant in the action code is per-frame at that rate;
angles are 16-bit binary angles through SM64's own 4096-entry sine table; the
original's collision quirks are reproduced rather than fixed. Where a number can
be measured it is measured, and the scripts that do it are in `tools/`.

**Keep the conversion honest.** Nothing is hand-modelled or re-rigged —
geometry, rigs, animations, collision and audio all come out of the decomp data
through converters in this repository.

**Stay a research base, not just one level.** The medium-term aim is to combine
elements from several N64-era decomps in one Panda3D project. Castle grounds and
Mario are the first slice.

## Documentation

The detail lives in the **[wiki](https://github.com/synaption/SuperPanda3dWorld64/wiki)**
— what was ported, what was measured, and what turned out to be wrong on the way
there:

| | |
|---|---|
| [Project layout](https://github.com/synaption/SuperPanda3dWorld64/wiki/Project-Layout) | what each module and tool does |
| [Assets](https://github.com/synaption/SuperPanda3dWorld64/wiki/Assets) | what is committed, and how to regenerate it |
| [Notes on the port](https://github.com/synaption/SuperPanda3dWorld64/wiki/Porting-Notes) | timing, angles, coordinates, deliberate quirks |
| [Verified behaviour](https://github.com/synaption/SuperPanda3dWorld64/wiki/Verified-Behaviour) | the movement numbers, and what measures them |
| [Performance](https://github.com/synaption/SuperPanda3dWorld64/wiki/Performance) | what was profiled, fixed, and ruled out |
| [Asset workbench](https://github.com/synaption/SuperPanda3dWorld64/wiki/Asset-Workbench) | looking at and measuring one asset in isolation |
| [Not done yet](https://github.com/synaption/SuperPanda3dWorld64/wiki/Roadmap) | the honest list |

The wiki's source is kept in this repository under `wiki/`, so it is versioned
and reviewed alongside the code.

## Checks

Four scripts measure the things that are easy to get wrong, and all run
headless:

```bash
python3 tools/check_movement.py         # walking, jumps, the jump chain
python3 tools/check_billboards.py       # billboarded parts track the camera
python3 tools/check_anim_grounding.py   # grounded actions keep their feet down
python3 tools/check_sound.py            # why the game is silent, layer by layer
```

## Credit and licence

The game data is Nintendo's. Geometry, animation and collision come from the
[Render96ex](https://github.com/Render96/Render96ex) decomp, textures from the
Render96 HD texture pack, and audio from samples extracted out of a ROM. It is
committed here so the project is runnable and reviewable, which is a different
thing from being redistributable — worth thinking about before forking this
somewhere public.

The code in `sm64py/`, `tools/` and `app/` is an original port and is free to
use.
