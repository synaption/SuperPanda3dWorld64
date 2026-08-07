# SuperPanda3dWorld64

Reproducing Super Mario 64's castle grounds and Mario's motion system in Python
and Panda3D, from the Render96ex decomp — as a demonstration that Panda3D can
carry a smooth, playable game of that era's caliber.

The [README](https://github.com/synaption/SuperPanda3dWorld64) covers what this
is and how to run it. These pages are the detail behind it: what was ported,
what was measured, and what turned out to be wrong along the way.

## Start here

- **[Project layout](Project-Layout)** — what each module and tool does.
- **[Assets](Assets)** — what is committed, and how to regenerate it.
- **[Notes on the port](Porting-Notes)** — timing, binary angles, coordinates,
  and the original quirks kept on purpose.

## The port

- **[Verified behaviour](Verified-Behaviour)** — the movement figures, and the
  script that measures them.
- **[Water](Water)** — the submerged action group, and why the surface drifts
  rather than spins.
- **[Sound](Sound)** — action-driven sound events and where the samples come
  from.
- **[Objects](Objects)** — trees, goombas and scuttlebugs.
- **[Billboards](Billboards)** — the parts SM64 turns to face the camera, and
  the three stacked reasons the obvious fixes did nothing.

## Tooling

- **[Exporting actors for Blender](Exporting-Actors)** — decomp actor to rigged,
  animated glTF.
- **[The asset workbench](Asset-Workbench)** — look at, and measure, one asset
  in isolation.
- **[Performance](Performance)** — what was profiled, what was fixed, and what
  was measured and ruled out.

## Status

- **[Not done yet](Roadmap)** — the honest list.

## A note on how these pages are written

Findings here are written up with the number that produced them, and wrong
turns are kept rather than quietly deleted. Several sections exist because a
conclusion that read perfectly turned out to be measuring the wrong thing — the
[billboard](Billboards) and [performance](Performance) pages especially. Leaving
those in is the point: they are the parts most likely to be re-derived, wrongly,
by the next person.

---

*This wiki is kept in the main repository under `wiki/`, so it is reviewed and
versioned alongside the code. To publish a change:*

```bash
git clone git@github.com:synaption/SuperPanda3dWorld64.wiki.git /tmp/wiki
cp wiki/*.md /tmp/wiki/
cd /tmp/wiki && git add -A && git commit -m "sync from main repo" && git push
```
