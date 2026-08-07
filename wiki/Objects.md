# Objects

Trees and enemies run on the same fixed [30 Hz tick](Porting-Notes) as Mario and use the same
surface queries, so they stand on the same floors and stop at the same walls.
They are far simpler than he is -- one velocity, one yaw, gravity, and a small
state machine -- because that is all the originals are. Nothing in
`sm64py/objects.py` touches Panda3D; objects carry a position, a yaw and the
name of the clip they want, so the simulation still runs headless.

**Trees come from the level.** The 26 bubble trees were already being parsed
into `collision_objects.json` and simply going unused. They are instanced from
one loaded model rather than loaded per tree.

**The enemies are placed by hand.** Castle grounds has no goombas or
scuttlebugs in the original, so `ENEMY_SPAWNS` in `app/main.py` puts a few on
open ground near the spawn.

**Sizes come from hitboxes, not from eye.** Mario's hitbox is 160 tall, a
regular goomba's is 50 scaled by 1.5, and a scuttlebug's is 70 -- so they
should stand at about 0.47x and 0.44x his height. Measured in game: 0.46x.

Getting there needed the comparison done against a *posed* Mario. His bind
pose reports 80 units tall with its origin in the middle of him, because SM64
joints point down their own limbs and the rest pose is not a pose at all --
posed to his idle clip he is 149.9, which is the number to size against.

**Rigged objects have to be Actors, not instanced geometry.** Loading one as
plain geometry leaves it in that same meaningless bind pose, straddling the
ground plane rather than standing on it, which reads in game as a half-sunk
enemy lying on its side. Only models with no animations at all -- the trees --
are instanced from a single shared copy.

**Interactions** are resolved after both have moved, so a stomp is judged on
where they ended up rather than where they started. Landing on top while
falling defeats an enemy and bounces Mario at 42; touching one in an attacking
action defeats it outright; anything else knocks him back. A hit sets an
invincibility timer -- without one the knockback leaves him inside the enemy
that hit him and the same touch re-triggers every tick, costing three or four
hits for walking into one goomba once.

**Not every actor wants the quarter scale.** Mario's geo wraps his body in
`GEO_SCALE(0x00, 16384)` and the exporter bakes that in. The tree has no
`GEO_SCALE` at all, and `geo_layout.py` already applies the ones that exist, so
applying the quarter again left the trees and both enemies at a quarter of
their intended size. `ACTOR_SCALE` is now per-actor.

**Animations are per-actor.** Only Mario keeps his in a shared `assets/anims`;
every other actor keeps its own beside its model. Reading the shared directory
for them does not fail cleanly -- the tables are positional, so Mario's
20-joint animations get applied to whatever hierarchy the actor has, which
warned for the goomba and crashed outright on the scuttlebug's 42 joints. The
animation header regex also had to stop requiring array brackets, since only
Mario declares his as `struct Animation anim_00[]`.

---
[Wiki home](Home) · [Repository](https://github.com/synaption/SuperPanda3dWorld64)
