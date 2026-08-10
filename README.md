# mario64 in panda3d

The goal of this project is to combine different elements from different old N64
games based on their decomps in python with panda3d for research. We are going to
start with mario 64, just the castle outside and mario with his motion system
based on the render96 recomp.

Current state: the castle grounds load and render from the decomp data, and
Mario's movement — walking, slopes, jumps, the jump chain, dives, slides, ledge
grabs, wall bonks — runs on a port of the original physics. The moat and lake
are swimmable, actions raise the original sound events and play real samples,
and the level's trees stand alongside a few goombas and scuttlebugs that can be
stomped, punched, and run away from.

The character you play is now **the Hero**, a rigged model with twenty
hand-authored clips of his own. He is not Mario in a different costume: rather
than retarget Mario's 209 animations onto him, he has his own action machine
(`sm64py/hero/`) built around the moves he actually has — walk, run, jump, a
two-hit sword chain, a spin kick out of a run. What he shares with Mario is
everything below the neck: the same level, the same collision, the same
quarter-step movement, and the same 30 Hz tick.

Mario is still here, and still runs the full decomp port. He wanders the field
as an NPC, and **F2** hands control back to him at wherever the Hero is
standing — which is the point of keeping him, since it makes the two movement
systems directly comparable over the same ground.

## Running

```bash
python3 app/main.py
```

Requires `panda3d` and `numpy`. Everything the game loads is committed under
`assets/`, so a clone runs without the 12 GB of reference material.

## Assets

`assets/` holds the converted game data — about 8.5 MB, and only what is
actually loaded:

```
assets/
  billboard_tuning.json          how billboarded parts aim (see "Objects")
  castle_grounds/
    collision.npz                490 vertices, 879 triangles, 2 water boxes
    collision_objects.json       special objects, including the 26 trees
    mesh.npz                     1350 vertices, 785 triangles
    mesh_materials.json          45 material groups, 44 of them textured
    textures/                    the 21 PNGs those groups reference (2.9 MB)
  mario/          mario.glb + mario_clips.json   (209 animations)
  hero/           hero.glb + hero_clips.json     (20 animations, 53 joints)
  actors/         goomba, scuttlebug, tree, warp_pipe; a clips sidecar each
                  where the actor animates
  sounds/mario64/ full source library plus 57 runtime WAVs and .source marker
```

`hero.glb` is built out of Blender rather than out of the decomp, so it goes
through a different pipeline — see "Exporting the Hero" below.

The textures are copied in by `parse_f3d.py` rather than referenced in place.
They used to point back into `reference/RENDER96-HD-TEXTURE-PACK/`, which is 12
GB of third-party material that cannot be tracked — so a fresh clone parsed
fine and then drew the entire castle grounds untextured. Only the images the
level actually uses get copied: 21 of them, against the thousands in the pack.
Their directory structure is preserved rather than flattened, because two of
them are both called `0.rgba16.png`.

> All of this is derived from Nintendo's game data — the geometry and animation
> from the decomp, the audio extracted from a ROM, the textures from a
> community HD pack. It is committed here so the project is runnable and
> reviewable. That is a different thing from being redistributable; consider it
> before publishing this repository anywhere public.

### Regenerating it

Only needed when the source data changes. All of these read from `reference/`
and write into `assets/`.

```bash
python3 tools/parse_collision.py \
    reference/Render96ex/levels/castle_grounds/areas/1/collision.inc.c \
    assets/castle_grounds/collision.npz

python3 tools/parse_f3d.py reference/Render96ex/levels/castle_grounds 1 \
    assets/castle_grounds/mesh.npz

python3 tools/export_actor_gltf.py --actor mario --anims all \
    -o assets/mario/mario.glb

# The warp pipe. It needs both flags spelled out: its layout is `warp_pipe_geo`
# rather than the `<actor>_geo_body` the exporter assumes, and like the tree it
# carries no GEO_SCALE, so the default quarter would leave it knee-high.
python3 tools/export_actor_gltf.py --actor warp_pipe \
    --root-layout warp_pipe_geo --scale 1.0 -o assets/actors/warp_pipe.glb

# Clips that are not the decomp's. These append to the .glb the line above
# writes, so they have to run after it, and they have to run *again* whenever
# that line does -- a fresh export has no idea they ever existed.
python3 tools/retarget_anim.py --clip Zombie_Walk:zombie_walk \
    --clip Zombie_Idle:zombie_idle
python3 tools/author_skate.py

python3 tools/import_sounds.py
```

### Controls

As the Hero:

| | |
|---|---|
| `W` `A` `S` `D` / arrows | analog stick (camera-relative) |
| mouse | look — the window takes the pointer at startup |
| right mouse / `F` (held) | aim: the camera comes in over his shoulder, the view narrows, the crosshair closes up |
| `Space` | jump — and, off the skates, the take-off; see below |
| `V` (held) | the jetpack. On the ground it puts him on his skates; in the air it flies him |
| `Left Shift` | attack; again mid-swing to chain the second, or while running for the spin kick |
| `Left Ctrl` | draw or sheathe the sword |

As Mario, which is the original's set, unchanged:

| | |
|---|---|
| `Space` | A — jump |
| `Left Shift` | B — punch, dive |
| `Left Ctrl` | Z — crouch, ground pound, long jump |
| `Z` (held) | shamble like a zombie |
| `C` | put the skates on, and take them off again |

Either way:

| | |
|---|---|
| `X` / left mouse (held) | a circle grows where the crosshair points, and the allies inside it when you let go follow you — see below |
| `X` / left mouse (tapped) | send the ones following to the spot being aimed at |
| `Q` `E` | swing the camera without the mouse |
| `R` | re-centre the camera behind him |
| `` ` `` (backquote / tilde) | open the debug console, pausing the game |
| `F1` | toggle the debug readout |
| `F2` | swap between the Hero and Mario |
| `F3` | toggle the collision overlay |
| `Esc` | close the console, then release the mouse, then quit |

The mouse is captured at startup, which is what makes it the look control
rather than a pointer. `Esc` gives it back — the console needs it for its
sliders and takes it automatically — and clicking the window takes it again.
While it is loose, dragging with a button held still swings the view, so the
game is playable without the capture at all.

Where the platform has a relative mouse mode the pointer free-runs and there is
nothing more to say. Where it does not — WSL over X11 is the case at hand, and
Panda3D says so in the log — the pointer has to be shoved back to the middle of
the window before it reaches an edge and stops reporting. The delta is taken
against the last *observed* position rather than against the centre, and the
reason is worth recording: `move_pointer` does not land synchronously, reading
the pointer straight back after a warp still returns the old position, and a
delta measured as `pointer - centre` is therefore wrong by the width of the
window on any frame the previous warp had not arrived. That is a single frame
turning the view ninety degrees and pinning the pitch at its limit, at random,
a few times a minute. Measured against the last reading instead, warp timing
cannot enter into it; the one frame a warp lands on is dropped, since it is not
motion, and one frame in a few hundred is not detectable.

A crosshair marks the centre of the view, which is the same thing as the line
the aim is taken along. It is drawn twice, a thick dark pass under a thin light
one, because a single-colour reticle vanishes against the castle grounds —
white sky above the hill, dark green on it. It does not hold still: it opens
with his speed and with being in the air, and closes up when he stops and again
when the sights come up, where a dot fades in at the middle of it. It hides
itself while the console is open, which is drawn in the same layer.

### A gamepad

Plug one in — before starting or while playing, either works — and it drives
the same set. Nothing has to be configured and the keyboard keeps working
alongside it; the two are added together rather than switched between, so a key
and the stick pushed the opposite way simply cancel out.

| | |
|---|---|
| left stick | analog stick, with a walk at the bottom of its travel |
| d-pad | the same, at full deflection, when the stick is centred |
| right stick | look, with a response curve and a turn ramp — see the camera below |
| right stick, clicked | aim. A latch, not a hold: the thumb aiming with the stick cannot also hold it in |
| left stick, held in | the right stick sets how far out the camera sits, rather than where it looks |
| left trigger | the jetpack — on the ground he skates on it, in the air he flies on it |
| `A` | jump — or, off the skates, take off |
| `B` | attack — Mario's B |
| `X` | the squad, held or tapped as above |
| right trigger | Z — crouch, ground pound, long jump |
| right shoulder | re-centre the camera |
| left shoulder (held) | shamble like a zombie |
| `Y` | put the skates on, and take them off again |
| `Start` | swap between the Hero and Mario |

The button names are Panda3D's, which are an Xbox pad's; a DualShock reports
its own layout through the same names, so `A` is cross and `X` is square. The
console still needs the keyboard, and holds the pad neutral for as long as it
is open.

`X` used to be a second attack button alongside `B`, and is the squad's now:
one button cannot both swing a sword and hold a whistle open, and the squad is
the one of the two that needs the hold. It is also the only control whose
release is a command in its own right, so `Gamepad` publishes a falling edge
(`released`) beside the rising one.

`app/gamepad.py` is the whole of it: it polls the first pad the system reports
once a rendered frame and hands the game a snapshot, rather than throwing
button events, because the rest of the input is polled too — the analog stick
has no event to throw and the tick loop reads held state. `python3
tools/check_gamepad.py` exercises the mapping against a stub device, so the
directions, the deadzone and the latching buttons can be checked with no pad
plugged in.

## The camera

`sm64py/camera.py`. It used to be a follow camera in Lakitu's spirit — it
trailed Mario, you could swing it around him, and its yaw was what the analog
stick was measured against. That is the right camera for a platformer and the
wrong one for a game you aim in, for one reason ahead of all the others: the
player's own look input was *eased*. Pushing the stick set a target yaw and the
camera crept toward it over a couple of hundred milliseconds, so the view was
always somewhere you had asked to be a moment ago. On a platformer that reads
as weight. On anything with a crosshair it reads as latency, because the
crosshair is the thing being steered and it never arrives.

So the rule the current one is built on:

> **The player's look input is never smoothed. Everything else is.**

A mouse delta or a stick push moves the view on the frame it arrives, in full,
with no spring between the hand and the angle. What *is* smoothed is everything
you did not ask for — the character walking around underneath, the ground
rising into stairs, a wall sliding in behind the boom, the move into and out of
the sights.

**The pivot follows him rather than tracking him.** The boom hangs off a point
at chest height that chases him with a critically damped spring — Unity's
`SmoothDamp`, the same algebra, and critically damped rather than exponential
for a reason worth stating: exponential smoothing is fastest at the instant the
target moves and slows from there, so a target that changes velocity puts a
corner in the output. A spring carries its own velocity across the change. That
is the difference between a camera that follows and one that glides, and it
costs four multiplies.

Horizontally it closes in 55 ms, which is enough to take the stair-step off the
30 Hz simulation without feeling loose. Vertically it is slower and has a **dead
band**: his height has to change by more than a step before the camera answers
at all, so kerbs, slopes and the ordinary bob of a run leave the horizon alone,
and past the band it chases the *edge* of the band rather than him, so the
recovery has nothing to rebound from. The air gets a wider band and a slower
spring — a jump you can see the top of reads better than one the camera rides —
and a hard leash at 520 units, because nothing above outruns a jetpack.

**The boom pulls in hard and pushes out soft.** The line back from his shoulder
to where the camera wants to be is marched against the collision, and anything
in the way — walls, ceilings and the ground alike — shortens it. Coming in is
instantaneous: a camera that eases into a wall spends those milliseconds inside
it. Going back out takes a third of a second, because a camera that snaps out
the moment a pillar clears is a jolt. That asymmetry is the whole of the
occlusion behaviour.

Shortening is the only answer it has, and that is deliberate, because it is the
only answer that is free. **The camera sits on its own aim ray**, so sliding in
and out along that ray leaves the ray — and so the point under the crosshair —
exactly where it was. Lifting the camera over a hillside instead would keep the
distance and drag the aim across the world with the terrain: a hundred units of
lift with the view angled eight degrees down moves the point under the
crosshair seven hundred units further out, and at five degrees, eleven hundred.
A camera that crowds your shoulder on a slope is doing its job. One that walks
your aim off target every time you cross a hill is not. The march therefore
runs backwards along the view direction, not outwards from the pivot, so that
the length can change by any amount at all and cost the aim nothing.

**The shoulder offset** puts him to one side so he is not standing on top of
what you are aiming at. It is the one part of the rig that sits *off* the ray,
so it is left alone unless it has to give: only when the offset itself is
buried in a wall — him flat against it, shoulder side in — does it fold away,
smoothly and in both directions, since that move is one the aim can feel.

**The sights** blend the whole rig — boom length, shoulder, field of view, look
sensitivity, pitch limits — toward a tight over-the-shoulder framing over about
a tenth of a second in and twice that out. `set_aim` takes an amount rather
than a flag, so a partial aim is expressible — an analog control could hold the
camera half way in. Nothing is bound to one at the moment: the pad aims on a
right-stick click, which latches, because the thumb aiming with that stick
cannot also hold it in.

**Sticks get a curve and a ramp.** The magnitude is squared and the direction
kept, so the middle of the stick's travel is fine control and the rim is the
full rate; and holding it near the rim ramps the rate up by 85% over about half
a second, which is what lets one thumb both flick around and track something
slowly. The mouse gets neither — it needs neither — beyond 20 ms of smoothing
to take the stair-step off a 125 Hz mouse read at 200 fps, which is a slider
(`mouse_smooth`) and can be set to zero for a raw 1:1 pointer.

**The boom length is the player's too.** Hold the left stick in and the right
one stops being the view and becomes the boom: push it forward and the camera
comes in over his shoulder, pull it back and it stands off, between 250 and
4000 units. It costs the aim nothing to move, for the same reason occlusion
does not — the camera is travelling along its own ray — so the distance can be
changed mid-fight without the crosshair drifting off what it was on. The hip
and the sights keep their own lengths and the stick sets whichever is in
effect, since how far back you want to stand and how close you want to be down
the sights are two different decisions.

**Nothing re-points the view but the player.** No drift back behind him while
he runs, no framing assist, no correction of any kind. `R` is the only thing in
here that turns the view on its own, and `R` is a button.

The rest is small: a landing kicks the camera in proportion to the fall, a
sprint widens the view by four degrees and lengthens the boom, and the whole
lot is on console sliders (`cam_distance`, `cam_shoulder`, `cam_fov`,
`cam_follow`, `mouse_sens`, `stick_sens` and a dozen more).

`python3 tools/check_camera.py` runs the half of this that has a number
attached, headless: that a look input lands on the frame it arrives and in
full, that the same hand movement turns the same angle at 30 fps and at 240 for
both the mouse and the stick, that everything which *is* smoothed settles
rather than ringing, that the boom comes in on the frame it must and leaves
slowly, that the boom's stops hold and the two lengths stay independent, and
that the aim ray really is the middle of the screen.

## The jetpack

**The left trigger is the jetpack, and `A` is the jump. Neither is the other.**
That separation is the whole design, and it is not where this started: `A` used
to become the boosters once it had been held six frames, on the grounds that
six frames is long enough to tell a tap from a hold. It is, but a jump that
turns into a flight when you are slow off the button is a jump you cannot
trust, and it left the trigger and the button meaning overlapping things. Now
`A` is a jump, every time, and the trigger is the boosters, everywhere.

The keyboard has the trigger on `V`, since `A`'s key no longer doubles as it
and a keyboard with no boosters would be a keyboard playing a different game.

### On the ground it skates

Holding the trigger while he is standing does not lift him. It puts him on
`ACT_HERO_SKATING`, riding the jets at ground level, and the physics is the ice
the game already had: `update_sliding` with the floor class forced to
very-slippery is momentum that keeps going, a friction that barely bites, and
steering that rotates the velocity vector rather than the body, so turning
costs distance instead of speed. What ice has no answer for is where the speed
comes from — nothing in SM64 makes a sliding character go faster on the flat —
and here the jets supply it, `SKATE_PUSH` per tick at full stick up to
`SKATE_TOP_SPEED` of 56, against a running top speed of 38. It is the fastest
way he has of crossing the grounds, which is the point of pressing it.

One thing is deliberately *not* the ice's answer. A slide is at the mercy of a
hill: `update_sliding` pulls 10 units a frame downhill at full steepness and
nothing in a slide answers it, which is why Mario on skates cannot climb. A
character being pushed by an engine is not sliding, and being dragged backwards
down the rise to the castle door is not what holding the trigger should feel
like, so `SKATE_GRIP` cancels all but 1.5 of that pull before `update_sliding`
adds it. A steep face still costs him speed; it does not reverse him.

`A` out of a skate is the take-off, and it is not a jump: no jump action, no
jump clip, and no clip restart either. The skate and the flight draw the same
held pose, so `hold_pose()` suppresses the animation reset every other
transition in the machine wants — otherwise the take-off would visibly replay
on the spot, which is the jump animation the take-off is explicitly not.

### In the air it flies

Press the trigger in mid-air — off a ledge, or halfway down from a jump — and
`ACT_HERO_JETPACK` lights from wherever he is, arresting the fall over a few
frames and then climbing. Let go and he falls.

He steers under thrust with the running controls rather than a jump's weak air
control: the stick turns him at the same `TURN_RATE` and accelerates him to the
same top speed, so he flies a circle around the camera facing where he is going
instead of drifting sideways, and letting go of it coasts him to a stop in the
air the way it does on the ground.

There is no flying clip among his twenty, so he keeps the pose he is already
in: `jump up` while he is rising, `jump down` while he is still coming down.

The one number worth knowing is that **the thrust has to beat gravity to be
thrust at all**. `apply_gravity` takes 4 units a frame back after every air
step, and the thrust is applied as an approach *before* the step, so anything
at or under 4 hovers rather than climbs — which is why `jetpack_thrust` has a
floor of 4 on its slider. Running the approach before the step is also what
makes the climb honest: the step moves him by the velocity it is given and only
then hands 4 of it to gravity, so the approach re-covers that loss every frame
and he settles at exactly `JETPACK_RISE_SPEED` rather than somewhere under it.
Measured over a long hold, he gains 20 units a frame at the default 20.

### Why the ground is a skate and not a slow take-off

Because taking off from the ground did not work, and could not be made to.

The boosters climb 8 units on the frame they light. A hill lifts the floor
under him faster than that the moment he is running: 38 units a frame forward
up a 25 degree slope raises the ground 16. So the air step landed him on the
frame it started, the landing pose played, the trigger was still down, the
landing handed him back to the boosters, and it went round again. Holding the
trigger while running uphill was a landing animation on a loop and no flight at
all — and the castle grounds are a hill.

There is no take-off to fail now. Ground under a burning jetpack is a skate,
whether he started there or flew into it, so `act_jetpack` answers
`AIR_STEP_LANDED` with `ACT_HERO_SKATING` rather than with a landing, and
hugging a rising slope on the way up puts him on his skates and he carries on
up the hill. `A` is what leaves the ground, and it leaves it at
`JETPACK_LAUNCH_SPEED` — 30 rather than 8, which clears anything up to about 28
degrees, and steeper than that simply puts him back on his skates.
`tools/check_hero.py` runs that hill and fails if the landing plays even once.

It rides on the `Controller` as its own field rather than as a button bit, the
way the skates do: the button mask is the N64's, every bit in it already means
something, and Mario shares the same controller.

`jetpack_thrust`, `jetpack_rise`, `jetpack_launch`, `skate_push` and
`skate_top` are all on the console, so the feel is a slider away.

## The squad

Hold `X` and a circle opens on the ground where you are looking, growing to
1100 units over about a second, with the arc of a throw drawn from your hands
to the middle of it. Let go and every ally standing inside it falls in behind
you. Tap `X` — the same button, under a fifth of a second — and the ones
following are sent to the spot the same aim resolves to, where they spread out
and hold it. Pikmin's shape, in other words, and `sm64py/squad.py` is all of
it.

**The allies are the Marios.** They are the only thing in the field that is on
your side, and they already hunt goombas, so an ally posted somewhere is an
ally fighting there. There are up to six about: the one the level places, and
the five the castle-path pipe produces.

**Aiming is the camera, not a cursor.** The crosshair is the middle of the
screen and the aim is the ray out of it, which the camera hands over directly
— `FollowCamera.aim_ray()`, built from the same yaw and pitch the view itself
was built from, so the two cannot disagree. Tilting the view down brings the
far end of that ray in; tilting it up throws it out to the cap. Left and right
is where the view is pointed. That is the whole aim, and it is why the reticle
never leaves the middle of the screen.

What comes back is not the ray's own hit but a point in front of the player, on
the bearing from *him* to that hit — so the camera sitting off his shoulder
does not skew where the throw goes — at the distance the ray chose: clamped
into 250–2600 units, and walked back toward him 200 at a time until there is
floor under it, which is what happens when the view is pointed out over the
moat or off the edge of the map. A throw does not have to land exactly where it
was pointed. It does have to land somewhere.

**The circle is traced over the ground it covers**, one collision query per
point, so a whistle across the slope up to the castle follows the slope instead
of cutting into it. That is 28 queries a frame, and the ray march is another 32
— coarse 150-unit steps and then six halvings, rather than fine steps
throughout — but none of it runs unless the button is down.

**The arc is a real arc**, in that its height comes from the same object
gravity a goomba out of a pipe falls under: a lob rising `a` over its own chord
is in the air `sqrt(8a / -g)` ticks, so the flight time follows from the shape
rather than being picked. Nothing is actually thrown — the allies walk — but
the preview is of a throw that would work.

It is drawn with rungs across it, and that is not decoration. The aim is always
straight away from the camera, so the arc, and anything else in the vertical
plane it flies through — a shadow under it, a line along the ground — projects
to a single vertical line on screen and reads as a pole planted in the dirt.
The rungs are the one part that is not in that plane. They come out horizontal
and crowd together toward the top, which is what the height of the lob looks
like from behind it.

**An order is held, not obeyed once.** An ally sent somewhere keeps the goal
after he arrives and stands on it until he is whistled up again, because the
alternative — handing him back to the wandering behaviour on arrival — has him
a thousand units away inside ten seconds, and then sending a squad somewhere
means nothing. He will still leave the spot to hit something, and walks back to
it afterwards.

**And his leash is held from the spot rather than from his own feet.** A loose
Mario hunts anything inside 3500 units; one under orders is cut to 1000, which
alone is not enough — measured from where he happens to be standing, each kill
puts the next enemy inside range of the last and the squad leapfrogs across the
level with the order a mile behind it. Measured from the goal, it stays put.

`squad_range`, `squad_circle`, `squad_grow` and `squad_follow` are on the
console. `python3 tools/check_squad.py` runs the whole of it against a flat
plane with no window: that the aim lands in front of you and not behind, that
tilting up throws it further, that the circle catches what it is drawn around,
that a whistled ally arrives and a sent one stays.

## The debug console

`` ` `` drops down a console over the game, and **pauses it** for as long as it
is open. It carries the readout `F1` draws, everything the game has printed
since it started, and a command line:

```
> run_speed
run_speed = 38.00  -- slider added, top speed, units per 30 Hz frame
> run_speed 12
run_speed = 12.00  (was 38.00)
```

Typing the name of a variable puts a **slider** for it on screen, bottom right.
The slider stays there when the console is dismissed, which is the point of it:
a movement constant is not worth tuning from a menu, it is worth tuning while
you are running around, and the game reads these values on the frame it uses
them, so a drag lands on the next 30 Hz tick.

| | |
|---|---|
| `<name>` | put a slider for that variable on screen |
| `<name> <value>` | set it outright |
| `vars` | every variable, with its value, range and what it does |
| `close <name>` / `close all` | take a slider away |
| `reset <name>` / `reset all` | back to the value it started at |
| `clear` | empty the log |
| `help` | the same list, in the console |

`Tab` completes a name, the arrow keys recall previous commands, the **scroll
wheel** goes back through the log — a marker on the divider says how far back
you are, and new output does not slide the lines you are reading — and the
keyboard belongs to the console while it is open, so typing `run_speed` does
not walk the Hero across the field.

The pause is the simulation simply not being stepped: no ticks, no accumulated
time to replay on the way out, and the clips are held on their current frame
rather than left walking on the spot. Nothing restarts them by hand —
`ObjectRenderer.sync` and `_update_animation` set every play rate from scratch
on each frame the game actually runs.

The variables themselves are declared in `Game._register_tunables`
(`app/main.py`), which is a dozen lines to extend: give a name, the module or
object holding the value, the attribute or attributes to write, and a range.
Most of them are the Hero's movement constants — `run_speed`, `walk_accel`,
`turn_rate`, `jump_velocity`, the sword and spin kick speeds — plus the two the
camera keeps for itself.

`run_speed` defaults to 38 — the Hero outruns Mario, whose own top speed is 32 —
and the slider runs to 120 and means it the whole way. Two things had to change for that to
be true, both in `update_ground_speed`:

- The stick's magnitude tops out at 32 — `intended_mag` is that magnitude
  squared into 0..64 and halved — and the speed he accelerated toward was the
  *smaller* of that and the cap, so every cap above 32 was the same cap. The
  target is now the stick as a fraction of its own ceiling, times the cap. At
  the default 30 the two agree at a full press and differ by 6% at a half one.
- The acceleration taper (`WALK_ACCEL - forward_vel / ACCEL_TAPER`) falls to
  nothing at 60, so nothing above that could be reached however long you held
  the stick. It now scales with the cap, keeping its shape. The ramp takes
  proportionally longer — a second to reach 30, four to reach 120 — so raise
  `walk_accel` alongside it if you want the top end to arrive sooner.

120 is where the movement stops being trustworthy rather than an arbitrary
stop: the ground step splits a frame into four, and at 120 units a frame each
quarter is 30 units, still inside the 50-unit wall check, so he is stopped by
walls instead of passing through them.

`run_speed` writes both `MAX_WALK_SPEED` and `MAX_RUN_SPEED`, since a ceiling
below the target is a Hero accelerating into a wall he can never cross.

Standard output is captured by wrapping `sys.stdout` and `sys.stderr`, so the
terminal still gets everything as well. Panda3D's own notify output is written
from C++, never passes through Python, and so appears only in the terminal.

## Layout

```
sm64py/
  math_util.py     binary-angle trig, Panda3D coordinate bridge
  surfaces.py      collision triangles, spatial partition, floor/ceil/wall queries
  level.py         converted mesh -> Panda3D geometry
  camera.py        the third-person shooter camera: look, follow, boom, sights
  console.py       the debug console: captured output, commands, live sliders
  objects.py       trees, enemies and warp pipes: spawning, behaviour, stepping
  squad.py         aiming at the ground, and the allies whistled up and sent out
  billboard.py     aiming billboarded actor parts at the camera, and its settings
  audio.py         sound events -> Panda3D, plus placeholder sample synthesis
  mario/
    constants.py   action ids, action flags, input bits, surface types
    state.py       per-frame state, controller sampling, geometry queries
    steps.py       quarter-step integration, gravity, ledge grabs
    actions.py     the action state machine
    animations.py  which animation clip each action plays
    water.py       the submerged action group
  hero/
    constants.py   his actions, and the numbers his movement is tuned with
    state.py       extends MarioState, so the same steps.py moves him
    actions.py     his action state machine -- walk, jump, sword chain, spin kick
    animations.py  which of his twenty clips each action plays
tools/
  parse_collision.py     collision.inc.c -> npz
  parse_f3d.py           F3D display lists -> textured mesh + its textures
  geo_layout.py          geo layouts -> actor node tree
  sm64_anim.py           animation tables -> per-frame joint rotations
  glb.py                 minimal glTF 2.0 / GLB writer
  export_actor_gltf.py   actor -> rigged, animated .glb
  export_hero_gltf.py    the Hero -> .glb, run inside Blender
  adopt_blender_export.py  make a Blender .glb loadable by Panda3D
  lock_root_motion.py    take the authored travel out of a clip
  rig.py                 posing Mario's skeleton from outside the decomp data
  retarget_anim.py       another rig's animation -> a clip on Mario's skeleton
  author_skate.py        the ice-skating cycle, written rather than borrowed
  import_sounds.py       extracted AIFF samples -> assets/sounds/mario64/*.wav
  workbench.py           look at / measure one asset, interactively or headless
  check_anim_grounding.py  grounded actions keep their feet on the floor
  check_billboards.py      billboarded parts track the camera and hold width
  check_movement.py        the movement figures quoted under "Verified behaviour"
  check_hero.py            the Hero's action machine, start to finish
  check_sound.py           why the game is silent, layer by layer
  check_gamepad.py         the pad mapping, against a stub device
  check_squad.py           aiming, whistling and sending, on a flat plane
  check_camera.py          the camera: look latency, following, occlusion, sights
app/
  main.py          the runnable game
  gamepad.py       the pad, polled a frame at a time
```

The eight `check_*` scripts all run headless and print what they measured.

## Notes on the port

**Timing.** Game logic runs at a fixed 30 Hz, because every movement constant in
the action code is per-frame at that rate. Rendering is decoupled and the
simulation is stepped in whole ticks.

Because of that, the drawn transform is *interpolated* between the last two
ticks using whatever time is left in the accumulator. Without it, on a 144 Hz
display over 90% of rendered frames land between ticks and redraw Mario at the
same spot, so he freezes for four frames and then jumps — the motion is correct
but looks like a stutter. The camera follows the interpolated position for the
same reason, and its smoothing uses exponential decay (`1 - exp(-rate * dt)`)
rather than `rate * dt`, which would otherwise change the settling time with the
frame rate and pass `dt` jitter straight through to the view.

**Angles** are 16-bit binary angles (`0x10000` to a full turn). Sine comes from a
4096-entry table indexed by the top 12 bits, reproducing the original
quantisation rather than calling `math.sin`, because that quantisation is visible
in long slides and slow turns.

**Quirks are kept deliberately.** Collision queries truncate the sample position
to `s16`, and they take the *first* triangle in list order that passes rather
than the best one. Combined with the partition's sort order — by each triangle's
*first* vertex height, which is not necessarily its highest — this reproduces
surface cucking, where a lower triangle shadows a higher one. Wall pushback
likewise tests every wall against the entry position while accumulating the
output, so overlapping walls each push by their full amount. These are not bugs
to fix; changing any of them changes how the game plays.

**Gait is chosen on speed, not on the stick.** This is a deliberate departure,
and the one place a keyboard forced one. `anim_and_audio_for_walk` picks between
tiptoe, walk and run — and scales the cycle — on `val04`, whichever is larger of
Mario's speed and how far the stick is pushed. That reads a range on a stick.
A key has no range: a press is always a full deflection, so `intended_mag` is
pinned at its ceiling of 32 and the comparison picks 32 on every frame. The run
clip therefore came out on the first frame of movement and played at a full
sprint's cadence — eight times its authored speed, 240 clip frames a second —
while Mario was still creeping forward at a quarter of that speed. The walk clip
never appeared at all. Choosing on his actual speed gives back what the original
shows on a stick: tiptoe, then walk, then run, as he gets up to it.

The thresholds and the divisors are still the decomp's. What is not the
decomp's is a two-unit hysteresis on the gait thresholds, and it earns its keep:
speed sawtooths by about a unit a frame, so where Mario settles near 5 or 22 a
bare threshold flips on alternate frames, and every flip restarts the clip from
frame zero. That reads as the animation stuttering rather than as him changing
gait.

**Coordinates.** SM64 is Y-up with +Z toward the camera; Panda3D is Z-up with +Y
away from it. `(x, y, z) -> (x, -z, y)` maps one to the other and preserves
handedness, so yaws stay yaws and no winding needs fixing.

**Textures** come from the HD pack. The decomp's own texture arrays are generated
from a ROM at build time and are not present, but each `#include` path maps
one-to-one onto a PNG in `RENDER96-HD-TEXTURE-PACK/gfx/`, so the parser resolves
symbols through that and copies the ones the level uses into `assets/`. A
vertex's four bytes are a colour when `G_LIGHTING` is off and a normal when it
is on, so the parser records the mode per material group and the loader
interprets them accordingly.

## Exporting actors for Blender

`tools/export_actor_gltf.py` turns a decomp actor into a rigged, animated
`.glb`. It is dependency-free apart from numpy — the glTF is written directly.

```bash
# every animation, game units, HD textures embedded
python3 tools/export_actor_gltf.py --actor mario --anims all -o mario.glb

# metres, for a Blender scene that works at human scale
python3 tools/export_actor_gltf.py --actor mario --anims all \
    --scale 0.0025 -o mario_metres.glb

# keep everything, including the wing-cap wings
python3 tools/export_actor_gltf.py --actor mario --exclude-dl '' -o mario.glb
```

Parts the game only draws conditionally are left out by default. Mario's wings
sit under a `GEO_ASM` hook that only emits them while the wing cap is active,
so exporting them unconditionally leaves them stuck to his head at all times.

Mario comes out as 30 joints (20 of them animated), 514 vertices, 760
triangles, and 209 animations.

**Why the conversion is exact.** These actors are rigidly segmented, not
smooth-skinned: each body part is its own display list authored in its joint's
local space. Every vertex therefore binds to exactly one joint with weight 1.0,
which is precisely what the hardware did when it multiplied each part by that
joint's matrix. Nothing is approximated or re-rigged.

**Joint order is implicit.** It is not stored anywhere — it is the order
animated parts are visited walking the geo layout depth-first, and the
animation index table is read in lockstep with that walk. The exporter
cross-checks the two and warns if they disagree. They agree here: the
hierarchy yields 20 animated joints and all 209 animation tables independently
report 20 parts.

**Scale.** The actor's geo wraps the body in `GEO_SCALE(0x00, 16384)`, and
`0x10000` means 1.0 — so Mario is authored at 4x and shrunk to a quarter at
draw time. The exporter bakes that in by default, giving a ~154-unit Mario that
matches the level and collision units.

**Clips need a start frame.** Animation headers carry a start frame, and the
exporter writes it to a `*_clips.json` sidecar because glTF has nowhere to put
it. It is not cosmetic: the single-jump landing clip starts at frame 22 of 38,
and its unplayed lead-in frames are a deep crouch — play it from zero and Mario
sinks knee-deep through the floor on every landing.

`tools/check_anim_grounding.py` guards this. It poses every grounded action and
reports any whose lowest vertex sits below the floor, which is what a grounded
action pointing at an airborne clip looks like. Airborne actions are exempt, as
is the ledge grab, which hangs below its position by design, and the submerged
group, which is off the ground by definition.

**Loop points turned out to be a non-issue.** This was listed here as an
outstanding gap. It is not one: `loop_end` in an animation header *is* the frame
count, and all 209 of Mario's clips have `loop_start` 0 and `loop_end` equal to
their length. The exporter was reading both correctly all along and there is
nothing to honour. `start_frame` is the field that actually varies — 18 clips
have a non-zero one.

**The swim stroke is timed to its clip.** Breaststroke runs a 14-frame action
timer and `SWIM_PART1` is exactly 13 frames, so it plays through once per stroke
with no rate scaling — the clip and the action were authored to the same length.
Re-pressing A during the arm sweep rewinds the clip mid-action rather than
changing clip, so a held stroke reads as one continuous cycle instead of visibly
retriggering; that needed a way for an action to ask for a restart without the
clip name changing (`MarioState.anim_reset`). Above forward speed 14 the flutter
kick stops re-asserting its clip altogether and whatever is playing keeps
running, so `animations.resolve` can return `None` meaning "leave it alone".

**Rest pose is meaningless.** SM64 joints point down their own limb, so the
unposed model splays along +X. Every animation supplies rotations for all 20
joints, so it only resolves once posed — that is normal for this data, not a
broken export. In Blender, scrub any action to see him assemble.

Runtime-driven joints (`geo_mario_tilt_torso`, head look, wing flap) export as
identity, since the engine drives those rather than the animation data.

**UV origin differs by target.** The N64 puts the texture origin at the
top-left with V increasing downward, and glTF does exactly the same — so actor
UVs export unflipped. Panda3D's own texture coordinates start at the
bottom-left, so `sm64py/level.py` flips V when it builds geometry directly.
Flipping in the converter instead silently mirrors every actor texture; the
giveaway was Mario's cap logo reading as a W.

**The combiner decides how texture and shade meet.** Getting this wrong on
Mario's face produced three different failures in turn:

- `MODULATE*` multiplies texel by shade. Applying that to the others
  multiplies a yellow button by blue denim and comes out black.
- `BLEND*` lerps the texel over the *shade colour*, within the polygon, using
  the texel's own alpha. The result is **opaque**: where the texture is
  transparent the shade colour shows through, not whatever is behind the
  surface. Treating that alpha as see-through cuts holes through his face.
- `SHADE*` samples no texel at all.

Mario's eyes, mustache, cap logo and overall buttons are all `BLEND`. glTF's
`baseColorFactor` can only multiply, so "texture over a colour" cannot be
expressed directly — the exporter composites each such texture onto its light
colour and emits the flattened result as an opaque texture. That is what the
hardware produced, just baked ahead of time.

Genuine cut-out alpha still occurs elsewhere and is emitted as `MASK` with a
0.5 cutoff; RGBA5551 carries a single alpha bit, so a mask is exact there.

**Solid colours come from lights, not vertices.** Mario's shirt and overalls
carry no texture and no useful vertex colour — the colour is on the light group
bound by `gsSPLight`, so the parser reads `gdSPDefLights1` and uses its diffuse
value as the material's base colour. Texture state also persists across display
lists, so a part that draws untextured has to actively say so via
`gsSPTexture(..., G_OFF)` or a shade-only combiner; without tracking that, the
last texture bound leaks onto every solid part after it.

**Textures must not be sRGB-decoded.** Panda3D's glTF loader flags
`baseColorTexture` as sRGB, which the spec asks for — but nothing here
re-encodes, and every other texture in the project is loaded raw, so the decode
lands once and is never undone. `sm64py.level.use_linear_textures` swaps those
formats back before the Actor is parented.

The symptom is a colour *split*, not an overall shift: a material's
`baseColorFactor` is used as written while its texture is darkened. Mario's
composited face rendered orange at (253, 136, 49) beside untextured parts at
the intended (254, 193, 121) — which is exactly sRGB-to-linear applied once,
and how the bug was identified.

**Panda3D needs the mesh under the skeleton.** The skinned mesh node has to be
a *child* of the skeleton root, not a sibling. Panda3D's glTF loader builds its
Character from the joint hierarchy and only adopts geometry sitting underneath
it — as a sibling the mesh loads and renders but never binds, so the Actor
animates nothing.

The app maps 48 actions onto 33 clips (`sm64py/mario/animations.py`), including
the speed-dependent tiptoe/walk/run split and the rise/fall halves of a double
jump.

## Exporting the Hero

The Hero has no decomp behind him, so `export_actor_gltf.py` has nothing to
work from and a three-step pipeline stands in its place. Run inside Blender,
with the character's .blend open:

```python
exec(open("//wsl.localhost/Ubuntu/home/bob/mario/tools/export_hero_gltf.py").read())
```

then, back on this side:

```bash
python3 tools/adopt_blender_export.py assets/hero/hero_raw.glb \
    --out assets/hero/hero.glb --sidecar assets/hero/hero_clips.json \
    --skeleton-root rig
python3 tools/lock_root_motion.py assets/hero/hero.glb
```

Four things about the source file make each step necessary, and every one of
them fails *silently* — the export succeeds, the game loads it, and the
character is quietly wrong:

**The rig ships in rest position.** `pose_position` is `REST`, which makes the
armature ignore pose evaluation altogether. The keyframes are all still there
and visible in the dope sheet; every exported clip is the bind pose held still.
`export_hero_gltf.py` forces it to `POSE`.

**Rigify carries 240 bones, 53 of which deform.** `export_def_bones` drops the
controls and mechanisms, which is also what gets the exported skeleton down to
something comparable with Mario's 30 joints.

**The material is lit through an Emission node.** Blender writes the texture as
`emissiveTexture` and leaves the base colour black, and since an emissive-only
material carries no metallic/roughness, glTF's defaults of 1.0 apply — a fully
metallic surface, which Panda3D's fixed-function pipeline draws as a flat white
silhouette with the texture washed out of it. `adopt_blender_export.py` moves
the texture to `baseColorTexture` and pins metallic to 0.

**The clips were authored on a stage, not on the spot.** Measured at the spine,
`Idle` sits at the origin, the run cycles start 0.68 units forward of it, and
`Attack 2` starts 0.83 forward and lunges another 0.85 mid-swing — a couple of
hundred game units. Played one after another they slide the character across
the ground and snap him back on every transition, while the physics position
that walls are tested against never moves. `lock_root_motion.py` shifts every
frame so the spine sits at the horizontal origin, which removes the authored
offset and the in-clip travel together. Height is left alone: the feet already
sit on the origin plane, and the vertical differences between clips are real
crouches and leaps. The attack's lunge is handed back as forward velocity in
`sm64py/hero/actions.py`, where a wall can stop it.

He is scaled on the Panda3D side (`HERO_SCALE` in `app/main.py`) rather than in
the export, so it is one number to change instead of a re-export. 81 puts him
at ~154 units, which is the height Mario is exported at — that is what lets the
two share a collision radius and a jump height.

## Borrowing animations from another rig

`tools/retarget_anim.py` puts a clip authored on somebody else's skeleton onto
Mario's. It is what the zombie shamble on the `Z` key is: `Zombie_Walk` and
`Zombie_Idle` out of `reference/mesh2motion-app`, which ships a CC0 humanoid
library on a rig with proper T-pose bind.

The usual way to do this — take each bone's world-space rotation relative to
its rig's rest pose, and apply that delta to the other rig — is unavailable
here, for the reason two sections up: **Mario's rest pose is meaningless**. The
bind pose is not a pose. Every joint is written with an identity rotation and
its offset along local +X, so unposed Mario is a stack of parts all pointing
the same way, and a delta measured against that means nothing.

What makes it work is that Mario has an A-pose *clip* — `MARIO_ANIM_A_POSE`,
`0x0E` — a real, untwisted, upright pose. Reading each joint's world rotation
there recovers the one fact the bind pose withholds: how that joint's local
axes sit relative to the body. Every SM64 joint puts its bone on local +X, but
the roll about that axis is per-joint and mirrored between the left and right
limbs, and only the A-pose measures it. Both rigs are then reduced to the same
body-relative terms — bone direction, plus a forward reference projected
perpendicular to it — and the transfer is absolute rather than relative, so the
two skeletons' rest postures never have to agree.

Some notes on what that does and does not buy:

**Proportions do not transfer, and cannot.** Bone *directions* are copied;
lengths stay Mario's. His legs are shorter and his thigh/shin ratio is inverted
against the source's, so a bent-knee pose that reaches the floor on the human
does not reach it on Mario. The clip is dropped to sit at the same average
clearance as `MARIO_ANIM_WALKING` rather than pinned so nothing ever
penetrates: the decomp's own walk swings between seven units through the floor
and six above it, so pinning the worst frame is what would look wrong.

**The head reads louder than it does on a human.** The source droops its head
54° forward. On the mannequin that is a slouch; on Mario, whose head is a third
of his height, it is the whole silhouette. This is faithful, not broken —
rendering the source rig beside it is how to tell the difference, and worth
doing before adjusting anything.

**Clips that are not the decomp's have no number.** Their id is their name, so
`anim_zombie_walk` sits in the same tables as `anim_48` and `anim_C5`, and
`animations.anim_name` handles either. The tool appends to `mario.glb` and
updates the `_clips.json` sidecar, replacing a clip of the same name, so it is
safe to re-run. It is not safe to *skip*: re-exporting Mario from the decomp
rewrites the `.glb` and drops every borrowed clip.

## Ice skating

`C` puts Mario's skates on and takes them off again. It latches rather than
being held, because the whole level is the rink. The Hero skates too, on the
jets rather than on a pair of blades and off the trigger rather than off `C`,
and everything in this section about the physics applies to both — see
[The jetpack](#the-jetpack) for the two places his differ.

**The physics is SM64's, with one piece added.** `update_sliding` already *is*
the game's ice — momentum that keeps going, a friction that barely bites, and
steering that rotates the velocity vector rather than the body, so a turn costs
distance instead of speed. All of it reads the floor *class* rather than the
surface type, so `MarioState.get_floor_class` reporting very-slippery while
the character's `skating_action` is running is the whole of what makes the
ground icy: the 10.0
sliding acceleration, the 0.98 retained per frame, the 5.3 slope acceleration
and the tightened slope thresholds all follow from that one override. The
decomp plays the same trick in the other direction for crawling, a few lines
below.

What SM64 has no equivalent of is the **push**. Nothing in the game makes a
sliding Mario go faster on the flat, because a slide is something that happens
to him — he is always either falling down a hill or bleeding off speed he
already had. A skater is the one supplying the speed, so that is the piece that
had to be written: `SKATE_PUSH` per tick at full stick, up to `SKATE_TOP_SPEED`
of 44, against a running top speed of 32. Skates that were slower than his own
legs would be a downgrade.

Panda3D was the other option offered and is the wrong one. What it has is
`panda3d.bullet`, a rigid-body engine — using it here would mean replacing the
30 Hz action state machine that every constant in this port is written against,
to get a worse-feeling version of code the port already contains.

Things that follow from reusing the sliding path, rather than being decided:
a wall bounces him back at half speed instead of stopping him dead; a jump
leaves `ACT_SKATING` and lands back in it; and he cannot climb a steep hill,
because very-slippery slope deceleration takes 45 units of speed off him in
five frames on the 68° bank behind the castle, and then he slides back down it.

**The animation is authored, not borrowed.** The mesh2motion library has no
skating clip — the nearest are a slide and a sprint — so `tools/author_skate.py`
writes one, using the same A-pose joint constants `rig.py` gives the retargeter.
The stride is stated as blade positions rather than joint angles: the blade goes
down under the hip, travels out to the side and back as the body glides over it,
lifts, and swings in and forward to land again. Two-bone IK solves the knee, so
the blade lands where it was asked to and the leg bends however Mario's leg has
to bend to reach.

The arms are aimed directly instead, because IK was the wrong tool for them.
His arms are 31 units against 39 for his legs, so hand positions that look right
for a skater are mostly places he cannot reach, and the solver clamps to a pose
nobody chose. Two bones aimed by angle have no unreachable case.

Two clips come out: `skate_stride` for pushing and `skate_glide` for coasting,
picked by whether there is anything on the stick. The stride's play rate is
picked by eye rather than by stride length, which is the one place these clips
differ from the walk cycles — matching foot travel to ground covered is what
keeps a walk honest, and a blade is the one foot that is *supposed* to slide.

There is no skating sound. The port's sound bank is keyed to decomp sound ids
and there is no blade sample to point one at.

## Verified behaviour

`python3 tools/check_movement.py` measures all of this and prints it. It is a
script rather than prose because these numbers were prose, and one of them had
quietly stopped being true:

```
walking on flat ground
  settles into a sawtooth between 31.01 and 32.35, mean 31.68
  reaches it after 37 ticks

standing jump
  A held      252.0 units
  A released  106.5 units

jump chain (running)
  jump 1      102.6 units
  jump 2      140.6 units
  jump 3      630.0 units

converted level
  490 vertices, 879 triangles, 2 water boxes
  floor under the spawn point: 260.0
```

**Walking does not cap at a flat 32.0**, which is what this said for a long
time. `update_walking_speed` approaches its target and then decrements past it,
so the settled speed oscillates between 31.01 and 32.35 around the 32.0 target.
That sawtooth is the original's behaviour, not drift in the port — but "caps at
exactly 32.0" was wrong, and nothing was checking.

Releasing A during the ascent cuts the rise to well under half. The triple jump
only triggers above forward speed 20, which is why jump 3 clears 630 units while
the two below it stay near 100.

The measurements run on a synthetic flat floor rather than on the level, so
terrain cannot confuse them. Two things about that floor fail quietly if you get
them wrong, and both did while writing the script: wound the wrong way every
triangle is a ceiling and Mario drops through a floor that looks perfectly fine
in the data; made wider than ±32768 it wraps, because collision samples truncate
to `s16`.

## Water

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

## Sound

The decomp names every noise Mario makes -- 467 packed sound IDs, with the
terrain folded into the low bits so one constant covers grass, sand, snow,
stone and water -- but ships no audio. Its samples come out of a ROM at build
time, exactly as the textures do. `sm64pcbuilder2` extracts them, so:

```bash
python3 tools/import_sounds.py
```

pulls the 15 samples the port actually plays out of
`reference/sm64pcbuilder2/assets/US/sound/samples/` and converts them to WAV in
`assets/sounds/mario64/` -- 57 runtime files once the terrain variants are
expanded. Those are committed
(see "Assets"), so this step is only needed to regenerate them. Without any
samples at all the game synthesises crude stand-ins and says so at startup, so
the two are never confused:

```
Audio: AudioManager ready, 57 samples
       imported from .../US/sound/samples
```

If the game is silent, `python3 tools/check_sound.py` separates the three
things that can be wrong -- no audio device, samples that failed to load, or
samples that load but never play -- and then plays them all out loud, so a
silent run there points at your audio output rather than at the game.

**Sample paths are converted, not passed raw.** Panda3D's loaders take its own
path syntax rather than the platform's, and the difference only shows on
Windows: a native `C:\...\assets\sounds\x.wav` is read as a *relative* path,
the model path is searched in vain, and the loader hands back a silent sound
instead of failing. On Linux the raw path is already in the right form, so the
bug is invisible there -- which is how it survived being tested. A sound that
loads with zero length is now treated as missing and reported once.

**Actions never play anything.** They append real IDs to
`MarioState.sound_events` and the front end drains it once a tick, so the
simulation runs identically with no audio device attached -- the normal case
under WSL.

**The sample bank is not ordered by terrain code.** Its file `02` is stone
while terrain code 2 is water, and it carries a metal step that no terrain code
selects, so the import maps by name. Lining the two up numerically would have
put the wrong sound underfoot on four of the eight surfaces. SM64 has no water
*step* sample at all -- stepping in shallow water uses the splash.

**Footsteps come from the animation, not from distance travelled.** They fire
on the two frames of each cycle a foot lands on -- 10 and 49 walking, 9 and 45
running, and so on -- which are the original's own numbers. Because the clip is
played back at speed/4, the cadence then follows Mario's speed with no constant
to tune: 3.1 steps/sec walking, 6.7 running, measured against 3.12 and 6.67
predicted from the clip lengths.

Driving them from distance instead was an approximation, and a bad one. It
fired every 52 units, which at a running 960 units/sec is 18 footfalls a second
-- nearly three times too fast. The simulation tracks its own animation frame
for this rather than asking the renderer, so footfalls stay in step whether or
not anything is being drawn.

## The asset workbench

`tools/workbench.py` draws one asset, alone, against a key-coloured background,
with any part of it hideable. That isolation is the whole point: every wrong
conclusion drawn about the models in this project came from measuring something
that was not isolated. Counting how many pixels a scuttlebug covered to check
its billboards measured mostly leg geometry, which swings with the viewing
angle for reasons that have nothing to do with billboards -- the numbers looked
fine whether the fix worked or not.

```bash
# look at it
python3 tools/workbench.py assets/actors/scuttlebug.glb

# measure it, with no screen -- for CI, or for an agent
python3 tools/workbench.py assets/actors/goomba.glb --headless --orbit 8 --json

# measure one part of it
python3 tools/workbench.py assets/actors/scuttlebug.glb \
    --headless --isolate 'billboard_' --orbit 8

# gate a build: non-zero exit if any check fails
python3 tools/workbench.py assets/actors/goomba.glb --orbit 8 --frame 0 \
    --billboard --expect
```

Note the `--frame 0`: an SM64 actor unposed is in its bind pose, which is not a
pose at all, and both checks are meaningless there.

Interactive mode orbits, tilts, cycles clips, toggles wireframe, and prints a
measurement on demand. `--list-parts` shows what `--isolate` and `--hide` can
match; `--compare` sizes an asset against another (usually Mario).

**Isolating a skinned actor is not hiding nodes.** All its parts live in one
GeomNode, so hiding by node name hides everything or nothing -- collapsing the
*joint* is what removes its vertices. And it has to keep the ancestors of a
match alive, because collapsing a joint takes its whole subtree with it: the
first version flattened the parents of the very parts it was asked to show and
measured an empty screen.

**Interactive keys.** Arrows orbit and tilt; `space` pauses the spin and
`[` `]` change its speed; `0`-`9` jump straight to a clip and `n`/`p` step
through them; `b` turns on cross-fading so switching clips shows the
*transition* rather than a cut; `w` wireframe, `t` two-sided, `m` prints a
measurement.

**Billboard settings can be adjusted here and saved for the game to read.**
`,` `.` select a setting and `-` `=` change it, with the current values shown on
screen; `g` makes changes an override for this actor alone rather than global;
`k` measures how well the billboards are tracking right now; `y` sweeps the
settings and applies whatever measures best; `s` `l` `r` save, reload and reset;
`d` prints what the geometry says about them. The same things from the command
line:

```bash
# what the geometry says: parents, extents, the rotation that needs cancelling
python3 tools/workbench.py assets/actors/goomba.glb --probe --frame 0

# try a setting without committing to it
python3 tools/workbench.py assets/actors/goomba.glb --billboard \
    --isolate 'billboard_4$' --set cancel_parent=off --orbit 12 --json

# let the measurement choose, and keep it
python3 tools/workbench.py assets/actors/goomba.glb --tune --save-tuning
```

`--tune` is for telling regimes apart, not for micro-optimising: a working
setting measures about fourteen times better than a broken one, so anything
within a tenth of the best counts as the same answer and the plainest of them
wins. Without that it cheerfully picked a 135-degree pitch on two percent of
pixel-quantisation noise. Run against the goomba it now lands on all-zeroes,
which is what ships.

**Two checks ship with it.** `billboard` orbits the asset and compares its
narrowest silhouette against its widest -- something that turns to face you
holds its width, something that only pretends to collapses toward a line.
`grounded` catches an asset whose origin straddles z=0 instead of sitting on
it, which is what a rigged model in its bind pose does, and is exactly why
enemies loaded as plain geometry looked half-sunk and tipped over. Every SM64
actor fails `grounded` unposed; that is the check working, not a false alarm.

## Objects

Trees and enemies run on the same fixed 30 Hz tick as Mario and use the same
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

**Warp pipes produce more of them.** `PIPE_SPAWNS` puts three on the field: one
by the spawn on the castle path that produces Marios, one in the western corner
that produces goombas, and one in the eastern corner that produces
scuttlebugs. Each throws one out every 30 seconds until five of its own are
alive, and then holds.

Three decisions in that are worth writing down.

*A pipe counts its own brood, not the level.* Counting every goomba would have
the goomba pipe stop two short of its quota, because `ENEMY_SPAWNS` already put
three of them out there — and it would leave "until one of them is killed"
meaning something the pipe had no hand in. Each pipe is responsible for exactly
what it produced, and for replacing that.

*The countdown runs only while there is room.* At the cap it stops where it
stands instead of resetting, so a kill resumes the clock rather than triggering
a spawn. That is the difference between one along every thirty seconds and a
replacement appearing the instant something dies.

*A mob is spawned at the pipe's feet and thrown upward*, not placed on the rim,
so it starts hidden inside the barrel rather than appearing in mid-air above
it. Objects fall at 4 a frame, so a launch at v peaks v²/8 units up and stays
airborne for v/2 frames: at 60 that is 480 units, clearing the 205-unit pipe by
as much again, with a full second in the air to be carried 20 a frame outwards
— about 620 units, four pipe-widths.

*And the arc is flown by `Object.coast()`, not by the behaviour.* Every one of
these behaviours writes its own speed each tick — a goomba bleeds 0.4 a frame
off whatever it has back toward a walk, a scuttlebug overwrites it outright
with its crawl — so a launch handed straight to the behaviour is gone within a
tick or two and the mob lands back down the barrel it came out of. While
`launched` is running the object set flies the object ballistically instead and
hands it back the moment it lands, which is what makes the distance thrown mean
anything. There is a 120-frame cap on that, in case an arc never finds a floor
to end on.

*The heading is chosen rather than simply random.* At 620 units out, which way
matters: the west pipe stands a few hundred units from where the ground falls
1700 into the moat, and measured over 200 throws a quarter of its goombas were
coming down in the water. The pipe now samples the floor where the arc would
put one down and rerolls, up to eight times, until it finds ground no more than
400 below itself. That takes it to 3 in 200 — the arcs that clip terrain on the
way and so do not land where the flat estimate said they would.

The pipes are drawn but not collided with:
the level's own collision is all the physics reads, and the actor's
`collision.inc.c` is not loaded, so you can walk through one.

**Mario fights, now that he is not the one being played.** An enemy within 3500
units is dropped everything for: he runs it down at 22, and inside arm's reach
— his own radius plus the enemy's, so a wide scuttlebug is hit from further out
than a goomba — he throws `MARIO_ANIM_FIRST_PUNCH_FAST`. The blow lands three
frames into the ten the jab holds him still for, so the enemy falls to the
punch rather than to having been stood next to. Fighting outranks the wandering
and outranks greeting the player, because a Mario who stops to wave while a
goomba walks into him reads as broken rather than as friendly.

Killing is `Object.defeat()` rather than writing the death timer directly, and
the death frames are counted down by `ObjectSet.update`. They used to be
counted inside the interaction pass, which returned early while Mario was
invincible — with two things in the level that can kill, an enemy's death has
to play out the same way whoever landed the blow.

**Only one Mario is ever in the field.** His pipe makes four more of him, and
the F2 swap has to stand all of them down rather than just the one the level
placed — re-applied per tick, since the pipe can fire while Mario himself is
being played. They are switched off, not killed, which is why a pipe counts
`defeated` rather than `active`: standing an NPC down is not a vacancy.

**A scuttlebug's wall recoil only fires with its feet down.** It hops backwards
off a wall at 30, and that was re-applied on every frame of contact — so one
held against a wall in mid-air was thrown up again before gravity had taken the
last hop back, and climbed. Five of them out of a pipe in a corner of the map
walked into the cliff beside it and levitated 1800 units up its face. Nothing
in the open field had ever pressed against a wall long enough to show it.

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
`tools/check_billboards.py` does both halves: that the joints actually move when
the camera does, and that each quad holds its width alone.

## Performance

Measured with an offscreen buffer, so the numbers below are CPU-side. The
machine this was profiled on falls back to Mesa's `llvmpipe` software
rasteriser, which means GPU-side timing could not be measured at all — the
large frame spikes that remain in the profile are software-rendering
artifacts, not something the port is doing.

The game logic is not the bottleneck and never was: simulation, camera,
animation and HUD together run at a 0.29 ms median, 1.50 ms at p99.

Four things were found and fixed.

**The camera moved in steps, not sweeps.** Its yaw was stored as a whole
16-bit binary angle and converted to a direction through SM64's 4096-entry
sine table. That table resolves 0.088°, worth 2.3 units at the camera's
1500-unit orbit radius, and `s16()` truncated any easing step below one unit
to zero. Easing a 30° pan at 60 fps moved the camera on **53 of 239 frames**
and then stopped 8 units short of the target permanently — it stalled and
jumped rather than sweeping. The camera now carries its yaw as a float and
uses real trig (`sins_f`/`coss_f`); stalled frames drop to 50 of 239, matching
an ideal float reference, and the pan arrives at 29.998° of 30.

This is the one fix that is *not* Panda3D-specific — `FollowCamera` is shared,
so the ModernGL front end had the same stepping.

Gameplay still sees a quantised angle: `mario_yaw` rounds to a whole binary
unit before the movement code reads it. The sine table stays in use everywhere
it affects simulation, which is the whole point of the port. It is only
bypassed for placing the render camera.

**Ten of 26 textures were uploaded mid-gameplay.** Panda3D prepares a texture
the first frame it is actually drawn, so scenery that came into view as the
camera swung paid its upload as a dropped frame. `preload()` now calls
`prepare_scene` once at startup; all 26 are resident before frame 1, for 0.2 ms.

**The HUD rebuilt its glyph geometry every frame.** Assigning to an
`OnscreenText` only marks it dirty — the text is regenerated during the
following cull traversal, so the cost lands in the render, not in the call.
It measured 0.58 ms per frame with spikes to 25 ms. It now refreshes at 10 Hz,
which is 1.03 → 0.45 ms median and 12.81 → 2.92 ms worst case.

**The level drew as 45 nodes.** One node per material group is how the loader
keeps render state separate, but groups sharing a state can be merged, and the
level never moves. `flatten_strong()` takes it to 24. Small at this scale (785
triangles) — worth doing, not worth expecting much from.

Mario's textures also arrived from the `.glb` without mipmaps, at 512×512 drawn
over a few dozen pixels, so the detail crawled as he moved. `use_mipmaps()`
gives them the filtering the level geometry already had. That is image quality
rather than frame time.

Two things were suspected, measured, and ruled out — recorded so they don't get
re-investigated:

- **Skinning is not a cost.** SM64 actors are rigidly segmented, so each geom
  binds to exactly one joint with no blend weights. The actor costs 0.13 ms.
  `hardware-animated-vertices` changes nothing, because there is nothing to
  blend.
- **Clips are not bound lazily.** The glTF loader binds all 209 up front; a
  clip's first play costs 0.002 ms. No pre-binding pass is needed.

### Vsync is off by default

This is what actually cleared up the microstutters, and it turned out not to be
a Panda3D overhead problem at all — the two front ends were simply not
configured the same way. `modernGL/main.py` free-runs with `clock.tick(240)` and
never requests vsync, while Panda3D defaults it on. Under vsync a frame that
overruns its refresh interval waits for the whole next one, which is a visible
hitch; free-running just shows that frame late and carries on.

So both Panda3D front ends now set `sync-video 0`. To put it back:

```sh
MARIO_VSYNC=1 python3 app/main.py
```

The tradeoff is tearing, and an uncapped frame rate that will spin the GPU as
fast as it can. If that becomes a problem, cap it rather than re-enabling vsync:

```sh
python3 app/main.py    # with: clock-mode limited / clock-frame-rate 120
```

## Not done yet

- **Animation blending.** Clips are swapped on action change with no crossfade.
  Playback rate and start frame follow the original. Actions whose animation
  depends on finer state (second punch, ledge climbs) fall back to a
  near-enough clip.
- **Tiptoe and walk cycles are unreachable on a keyboard.** The clip is chosen
  from whichever is larger, Mario's speed or how far the stick is pushed, and a
  key is always full deflection — so it selects the run cycle immediately. That
  is faithful; it just needs an analog stick to show.
- **The camera is not a port of the original's.** It never was, and it is now
  deliberately something else: a third-person shooter's spring arm rather than
  Lakitu. The original is a large state machine with per-area modes and
  hand-authored triggers, and none of that is here. Mario's control feel
  depends on the camera's yaw, which is wired up correctly either way.
- **Objects are a thin slice.** Trees, goombas and scuttlebugs run, and warp
  pipes produce more of them; there are no coins, no other enemies, and a pipe
  is scenery that things come out of rather than something either character can
  travel through. `collision_objects.json` carries more presets than anything
  consumes.
- **Nothing fights back at an NPC Mario.** He hunts goombas and scuttlebugs and
  they ignore him entirely, since both of them chase whoever is being played.
  He cannot be hurt, and he never dies — which also means a squad cannot be
  lost, only disbanded.
- **The squad does not steer around anything.** An ally walks at his goal and
  slides along whatever wall comes between the two, which gets him out of most
  corners and not out of all of them; there is no path through the level, and
  nothing keeps two of them from walking through each other on the way. The
  formation slots are what keep them apart once they arrive.
- **Scuttlebug legs render as thin wire quads.** Fifteen of its animated parts
  carry flat `LAYER_ALPHA` display lists that the geo does not billboard, so
  unlike its body they have nothing to turn toward the camera.
- **The Hero skates in a jump pose.** There is no skating clip among his twenty
  and no equivalent of `author_skate.py` for his rig, so `ACT_HERO_SKATING`
  holds `jump up` — a body with its legs gathered under it, which is what a
  character riding thrust should look like and is not the same thing as an
  animation for it. The camera does not see objects either: trees and warp
  pipes are not in the collision set, so the boom will not pull in for them.
- **Most cutscene and automatic actions** (poles, hanging, cannons), and the
  parts of swimming that need systems this port does not have: drowning and the
  breath meter (no health), metal-cap water walking, and carrying an object
  while swimming.
