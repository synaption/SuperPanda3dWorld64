# Billboards

**Billboards come from two different places.** A whole-object billboard is set
by the *behaviour*, not the geo layout -- `bhvTree` is `BILLBOARD()`/`CYLBOARD()`
even though the tree's geo has no `GEO_BILLBOARD` in it at all. Those are plain
static geometry, so Panda3D's own `set_billboard_axis()` handles them, and the
trees now turn to face the camera instead of standing as flat cards that vanish
edge-on as you walk past. Since nothing in the *asset* says to do this, the
workbench needs `--billboard-axis` to reproduce it: without that flag the tree
draws nothing from 5 of 8 angles, which is the asset being honest rather than a
regression.

**Billboard quads are single-sided, and that was most of the problem.** Once a
quad is turned to face the camera it is invisible from behind, and measured on
the goomba's face in isolation it drew *nothing at all* from 4 of 8 angles
around an orbit. Drawing both faces takes that to 8 of 8. The original never
gets to see the back of one, so this costs nothing.

**Part-level billboards are driven from `sm64py/billboard.py`.** The goomba's
face and most of a scuttlebug's body are `GEO_BILLBOARD` quads that the original
rebuilds every frame to point at the camera. glTF has no billboard concept, so
they export as ordinary geometry and collapse to thin lines edge-on; the
exporter makes each one a joint, and the renderer takes those joints over.

Three separate things were wrong, and the first two produced code that read
perfectly and did nothing:

- Panda3D's billboard *effect* acts on a node's transform, and this geometry is
  skinned to character joints, so it has nothing to act on.
- `Actor.control_joint` returns a NodePath parented to the **model root**, not
  into the joint hierarchy. So `set_hpr(some_other_node, ...)` is not a way to
  escape the joint's parents -- Panda3D solves that against the scene-graph
  parent, which is the model root, and it comes out identical to the plain local
  call. It never sees the joint chain at all.
- The joint chain's rotation is still applied inside the `Character`, on top of
  whatever is set on that node. On the goomba it is `(98.4, 4.9, -90.7)`, and
  that quarter turn of roll means a local *heading* comes out as net *pitch* --
  so heading tipped the quad up and down instead of turning it about vertical.
  No value of it could ever have worked, which is why five rounds of tuning a
  constant all failed.

The fix composes the wanted world rotation against the inverse of the parent
joint's measured net rotation: `net = local * parent`, so `local = world *
parent^-1`. Measured one quad at a time around a 12-point orbit, the width each
holds goes from 0.06 of its widest to 0.84 (goomba face) and from 0.08–0.10 to
0.71–0.80 (scuttlebug). The remainder is perspective -- these quads sit off the
axis they orbit -- and shows as a smooth swell, not a collapse.

`pitch` and `roll` are settings but both sit at zero, and it is worth saying why
they cannot help: a flat quad facing the camera has the same silhouette however
it is spun about its own normal. That they made no difference was read as a
mystery for a long time; it is just geometry.

**The parent rotation only exists once the actor is posed.** In the rest pose
every joint is identity, so it has to be cancelled per frame rather than baked
into a constant -- and an exposed joint reports identity until the character has
been evaluated at least once, which made the first frame of every measurement
quietly wrong until `claim()` started forcing an update.

**Settings live in `assets/billboard_tuning.json`**, read by the game and
written by the workbench, with per-actor overrides. They are a file rather than
constants in source because every previous value here was reasoned out and
wrong, and the only thing that reliably told them apart was measuring.

**A warning about measuring this.** Counting how many pixels the enemy covers
across a camera orbit does *not* verify billboarding: the leg geometry dominates
the count and swings with the viewing angle for unrelated reasons. Nor does
measuring a scuttlebug's three billboards together -- the bounding box then
tracks how far apart they are rather than how wide each one is, and reported the
broken setting as *better* than the fixed one. One quad at a time, isolated.
[`tools/check_billboards.py`](Asset-Workbench) does both halves: that the joints actually move when
the camera does, and that each quad holds its width alone.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
