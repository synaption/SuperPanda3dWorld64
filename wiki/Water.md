# Water

Water is not collision. It is a set of axis-aligned boxes the collision data
carries — castle grounds has two, the moat and the lake, both with their
surface at y = -81 — so "underwater" is a comparison against a height looked up
by (x, z), not anything the surface engine reports. `find_water_level` does that
lookup; the boxes were already being parsed into the `.npz` and were simply
going unused.

Below the surface Mario runs on the submerged action group, which steps
differently from the ground: no quarter-stepping, no gravity, walls tested
higher up his body, and a hard floor-to-ceiling headroom requirement. Buoyancy
pulls him toward the surface when he is near it and lets him sink when he is
not. Both bodies of water are genuinely swimmable — the moat runs to a median
430 units deep, the lake to 1067.

**Swimming is the only place pitch and roll are drawn.** On land Mario stays
upright however steep the slope, and the port threw both angles away. Swimming
aims his whole body along his heading, so `sync_graphics` now carries all three
and the front end applies `set_hpr` instead of `set_h`, interpolating each the
short way round.

**The surface drifts, it does not spin.** Rotating the UVs was the obvious way
to animate it and the wrong one: rotation moves every point by its distance from
the centre of rotation, so one corner of a 15000-unit water box crawls while the
opposite corner races. Worse, the centre is wherever UV (0.5, 0.5) lands, which
for these boxes is off in a corner rather than the middle. Measured, that drove
the moat surface at 1531-2429 world units/sec against Mario's 960-unit/sec
sprint -- the water outran him by up to 2.5x. It now translates instead, at a
flat 25 units/sec that is uniform across the sheet and expressed in units the
rest of the game uses.

**Underwater needs fog, not just a surface.** The water is a single flat sheet
with nothing behind it, so a camera below the waterline renders identically to
one above it. Dropping the fog range from 9000-20000 down to 200-4200 and
recolouring it green-blue is what actually sells being submerged. The test is on
the camera, not on Mario: swimming just under the surface leaves the camera in
open air looking down through it, and tinting the whole world in that case looks
wrong.

**The stick is mirrored, and only the yaw cancels it.** This port deliberately
feeds `stick_y` with the opposite sign to the original, and the heading formula
plus the camera rotation undo that. Anything reading `stick_y` as a *scalar*
has nothing to undo it: the swim pitch and the water-jump test both had to flip.
Measured rather than reasoned about — holding forward gave +39° of pitch and
floated Mario upward, when pushing forward should dive.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
