# Space Crusaders

A native Rust/Bevy game and research project that aims to be able to handle 10k+ units on screen on modest hardware, seemless interplanetary travel in a solarsistem with multiple planets that orbit and rotate.  

The current build includes the castle grounds, water, enemies, warp pipes, a
squad you can fill with Marios or with AI Lunas, stellarators you build and a
pylon network you plant to carry their power across the map, sound effects, a
third-person aiming camera, keyboard/mouse and gamepad input, a pause menu with
level selection and display settings, and a debug console.

**`Tab` says what `X` does.** One action button with four things it can be
aimed at — command the squad, plant a pylon, build a stellarator, or whistle up
a circle of loose balls — because a pad has one face button left and a player
should not have to remember which of three keys is which. Left and right
walk the picker, `1`–`4` jump straight to one, and `Tab` closes it. `B` and `G`
still build and plant directly, whatever the picker says.

Hold `B` to put a stellarator on the ground you are looking at, and `G` to
plant a pylon. Pylons string beams to every mast they can *see* within 150 m —
most of the map, so what you plan around is what stands between two masts rather
than how far apart they are — and power floods out from the machines along them;
`pylon 5` in the console puts a whole ring of them down at once. The network's
graph search is the crowd's — `src/route.rs` holds both the breadth-first flood
the flow field sweeps the castle with and the travelling-salesman tour the
supply packet makes its rounds on. See
[Machines and the pylon network](docs/project-guide.md#machines-and-the-pylon-network).

The beams are also the road the takings come home on. One enemy kill in twenty
drops a ball of **nuclonium**, the glowing green stuff everything here runs on.
Four things can collect one: a Mario sent for it, Luna walking past it, Luna
whistling for it, or the nearest mast simply reaching out and taking what is
lying at its foot. The whistle is the squad's own gesture pointed at the ground
instead of at the Marios — hold `X` to open a circle where you are looking, hold
it longer to grow one, and let go to call everything loose inside it. What Luna
picks up swims along behind her until she passes a mast, gliding once per drawn
frame after the pose she is drawn at rather than in the thirty steps a second
the simulation runs in — and so does the squad now, which is the same defect one
leader further out: a Mario is drawn between two ticks instead of at them, so
the train no longer stutters along behind a leader who glides. A Mario carries
it over its head to the nearest live mast, and the mast ships it back down the
beams to a machine — where it stays, turning inside the coils.

**Nothing made of nuclonium changes place without travelling there.** A ball is
snatched up off the grass into a Mario's hands rather than appearing in them; a
ball handed in at a mast rises off the ground it was lying on and into the beams
rather than starting at the top of the tower; one that is dropped falls; and one
that reaches a machine swims out from the point its flight ended to the orbit it
will turn in, drawing its own wake into the coils.

A stellarator's field *is* its stock, so an empty reactor is dark, a full one
is a solid green band, and you can read what you have banked from across the
valley — in light as well as in motes, since the field lights its own coils
and the ground it stands on as it fills. Five hundred is as many as it will
draw; past that it keeps counting. Nuclonium that is moving — carried over a
Mario's head, flying home down a beam, turning in a machine — draws a trail
along the path it actually took, as long as the ground it covered in the last
half second and no longer, hanging in the air a moment after it stops. The far
end of one slides back along the path rather than dropping a mark at a time,
so a trail at a steady speed is the same length every frame; the near end
closes over the ball in a dome and comes out from under its glow rather than
starting on top of it, so there is no edge where the two meet. Each ball is
also a **light**, not just a bright picture of one: it puts its own green on
the ground it floats over, on the wall beside it and on the Mario carrying it
— plain at night, gone in daylight — alongside its HDR emissive core and bloom
halo. All orb and reactor-mote halos are rebuilt into one transparent mesh and
one draw call, so thousands do not become thousands of transparent entities,
and the sixteen nearest the camera light the world out of one buffer every
surface in the game already reads. A ball nobody comes for shrinks away after
three minutes; any interest at all — a claim, a whistle, a magnet, a mast —
puts that clock back to nothing.

Whether a Mario goes for one is goal-oriented action planning — `src/goap.rs`
scores fighting, obeying, fetching, delivering and ambling against each other
every tick, on what each is worth up close and how far off it is. **A Mario in
a fight does not haul**: hauling is struck off the list outright while something
it can see is coming for it, which is a rule above the scoring rather than a big
number in it. Past sight range the same target is a grudge rather than a fight,
and the squad gets on with its work. Deep water is a cost in that score rather
than a wall, the squad steers round ponds on the way to somewhere, and an ally
out of its depth swims. And the buildings are part of the fight now: the crowd
knocks your pylons over, and your squad and your sword take slime and ant warp
pipes apart, all of it out of one component that says which side a thing
standing still is on.

About one kill in twenty-five drops a **red** ball instead, and that is hit
points — a quarter of Luna's pool, and there is nothing else in the game that
puts a Mario's back. It is not had at arm's length: a red ball notices whoever
is nearest that it would do any good to, Luna or a Mario, drifts in to them and
is absorbed on contact. Something at full health is not somebody it notices, so
it lies there until there is somebody it is for.

There is a second level, and it is round. The pause menu's **Level** page also
offers the generated planet from
[`experimental/planet_gen`](experimental/planet_gen/readme.md) — 1.2 km across,
786,432 triangles of terrain, and gravity pointing at the middle of it. Walking
far enough turns the ground over under your feet and takes you back where you
started. Its collision is its render mesh, read out of the glTF as it loads, so
choosing it is a short wait rather than a frame. See
[The level](docs/project-guide.md#the-level) and
[Gravity](docs/project-guide.md#gravity).

Crowds are the thing it is built around. `crowd 4000 mix` in the console puts
four thousand enemies on the lawn: the nearest couple of hundred are simulated
in full, and the rest are carried by a flow field, drawn as baked sprites, and
stripped down to a single entity each — so the whole distant horde costs two
draw calls and no level queries at all. See the
[performance notes](docs/project-guide.md#performance) for how it works and how
to measure it.

Escape pauses the game and opens the menu; its level page swaps between the
castle and the planet, and its display page changes the internal render
resolution, which is the world's own resolution rather than the window's — the
setting that buys frame rate, and the one that puts the console's pixels back.

## Run the project

Bevy 0.19 needs Rust 1.95 or newer, which is usually the rustup toolchain
rather than the distro compiler:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo run
```

`./run_bevy.sh` runs from any working directory, cross-builds/packages the
Windows executable, and launches it. Its paths are derived from the checkout,
so they do not depend on a particular user or machine. Pass `--source` to build
and run the native Rust source instead, or `--packaged` to launch an existing
Windows package without rebuilding it. On Linux, sound and gamepad build
against ALSA and udev and are opt-in: `cargo run --features sound,gamepad`. The
Windows build always has both.

Game assets required at runtime and their Blender authoring sources are
included under `assets/`. The old SM64 reference sources are no longer used by
the active asset pipeline. A normal build uses the committed runtime assets and
does not require Blender; run `tools/build_assets.py` only when regenerating
them from their authoring sources. Asset tools do not ship with the game.

## Documentation wiki

The [documentation home](docs/README.md) is the entry point for project
guides, controls, asset workflows, and design notes.

- [Project guide](docs/project-guide.md) — gameplay systems, controls,
  architecture, and porting notes.
- [Asset pipeline](docs/pipeline.md) — what is bundled under `assets/`, how to
  regenerate it, and the actor and Luna export workflows.
- [Aiming and attack animation design](docs/aim.md) — the proposed
  partial-body animation and procedural aiming approach.

## Asset notice

Some assets are derived from Nintendo game data and community texture work.
Review the asset notice in the [asset pipeline](docs/pipeline.md#what-is-committed)
before publishing or redistributing the repository.



***
<!-- AI do not edit below here. -->

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

Very distant units: billboards/impostors or extremely low-poly models. *(Done — see `impostor.rs` and `flow.rs`.)*

goal oriented action planning GOAP

The presentation of the game should be like a "recomp" of a fictional dreamcast game.  This will ballence the retro nostalgia with modern QOL and mix updated graphics with older low poly assets.  per pixel lighting, vertex lighting, and ray tracing.  

Gearbits - indie AC + bugs
Megaton Musashi W: Wired - terrain
dysonsphere project - interplanetary RTS.  
Perimeter - pylons

PBR materials
- git@github.com:Kimbatt/cc0-textures.git
- https://github.com/NVIDIA-Omniverse/PhysicalAI-SimReady-Materials
- https://github.com/texturedesign/materials-dataset

## Design

energy is green, luna's hair is green
nuclonium theme is like stranger things main theme mixed with all quiet on the western front.  
