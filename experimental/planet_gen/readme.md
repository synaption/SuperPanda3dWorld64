# Planet generation

> [Documentation home](../../docs/README.md) · [Asset pipeline](../../docs/pipeline.md) ·
> [Project guide](../../docs/project-guide.md)

Generate a 3D spherical planet tile by tile, from authored height and material
rasters, with hand-made tiles like the castle grounds dropped in as first-class
content. Output is Blender. The game eats it later.

This supersedes [`tools/build_quad_planet.py`](../../tools/build_quad_planet.py),
which already builds a cube-sphere at `RADIUS = 300.0` with 32 subdivisions per
face and flattens a tangent patch under the castle grounds. That script is one
monolithic mesh with no tiles, no heightfield and no LODs; its cube-sphere
mapping, its tangent-patch flatten and its asset placement are all kept here.

## What exists

A working planet, with a sea on it. `planetgen build` reads six face rasters
and writes 96 LOD0 tiles plus 6 LOD1 tiles in about a second; the Blender
export takes another three and writes `out/planet.blend` alongside
`out/planet.glb` (LOD0) and `out/planet_lod1.glb` (LOD1), each carrying the
terrain and one sphere of water over it. Renders are in `out/`.

```bash
python3 -m planetgen.cli init-faces          # seed the six elevation rasters
python3 -m planetgen.cli build               # rasters -> tiles/lod0, tiles/lod1
python3 -m planetgen.cli check --traversal   # validate seams and reachability
python3 -m unittest discover -s tests        # 22 tests, no Blender needed

blender --background --factory-startup --python blender/export_tiles.py
blender --background --factory-startup --python blender/render_preview.py
```

Current planet, at the scale in [Scale](#scale):

| | |
|---|---|
| Welded vertices | 393,218 (`6n^2 + 2` at n=256, exactly) |
| Triangles | 786,432 across 96 LOD0 tiles, plus 49,152 of sea |
| Land | 55.1% above sea level |
| Walkable | 88.4% of triangles, 99.1% of it one connected region |
| Seam disagreement | **0** — position, normal and material, at both LODs |

### How the seam guarantee is actually met

The readme below describes an edge record store, and it is still the right
answer for hand edits. But the whole-planet build does not need it, because
of how the data is laid out: there is **one global vertex array**, and a tile
is a slice of *index* into it. Adjacent tiles do not hold matching copies of a
boundary ring, they hold the same ring. `VertexGrid` welds the six faces by
hashing rounded directions, and asserts the result is exactly `6n^2 + 2`
vertices -- the count is only correct if all 12 face edges and 8 corners
welded, so a mistake there cannot fail quietly.

Vertex normals follow from the same layout: they are accumulated across the
whole planet, so a boundary vertex already sees the triangles on both sides
of the seam. That is the [one-cell apron](#normals-across-the-seam) realized
for free. A *single-tile* rebuild still needs the real apron.

LOD1 is a strict subsample of the LOD0 grid -- its vertices *are* LOD0
vertices, asserted by `test_lod1_lies_on_the_lod0_surface` -- so the two
cannot disagree about the silhouette.

### Not built yet

Designed below, and deliberately not implemented in this pass:

- Cross-tile features: splines, cliff lines, region polygons.
- The edge record store and `publish-edges`; nothing hand-edits a tile yet.
- `stitch-faces`. Seeding samples 3D noise at the sphere direction, so the six
  maps already agree across all 12 seams. Painting them by hand is what will
  need the tool.
- Authored tiles and patches, including the castle grounds.
- Dirty tracking and `--dirty`; a build is currently whole-planet.
- LOD skirts. Tiles carry a material index, a diagnostic vertex colour, and
  the five Render96 terrain textures used by the Blender and glTF exports.

## Requirements

- Spherical planets built one tile at a time.
- Pre-existing hand-made tiles, such as the castle grounds, used as-is.
- Regenerate any tile from the edges of whichever tiles happen to neighbour it.
- Adjacent tiles share a vertex ring — the same positions, not merely the same
  heights.
- Height comes from authored raster maps, in the manner of
  [`docs/reference/df_terrain.png`](../../docs/reference/df_terrain.png): a
  stack of painted layers, elevation being one of them.
- Cross-tile features must survive the seam.
- Work on one tile, one face, or the whole planet.
- Two LODs: one for space, one for the player's immediate vicinity.
- Python. Blender's API where it helps, not as a dependency of the core.

## Non-goals

**Determinism.** The generator is not a pure function of a seed. Tiles are
*data*, not a formula: once written, the tile file is the truth. This is the
deliberate cost of accepting arbitrary neighbours, direct hand edits, and
tiles supplied whole. Everything in [Seams](#seams) and
[Staleness](#staleness) exists to replace what determinism would have given
for free.

**Simulation realism.** No erosion, no drainage, no plate tectonics, no
climate model. `df_terrain.png` is a reference for *layered painted input*,
not for simulating the layers. What matters is traversability and the level
design features that are placed deliberately.

**Runtime performance.** Nothing here runs during play. Whole-planet passes may
take minutes.

**Unit LOD.** Actor and enemy LOD is handled separately, in
[`src/impostor.rs`](../../src/impostor.rs).

## Topology

A **cube-sphere quadtree**. Six root faces, each subdivided four ways to a
working depth.

Chosen over an icosphere because neighbours are trivially addressable, the LOD
levels *are* quadtree depths, every edge is an axis-aligned row of vertices,
and a flat authored patch like the castle grounds maps onto a face cell almost
directly. The cost is 8 corner points where three faces meet, handled
explicitly in [Seams](#seams).

### Face to sphere

`build_quad_planet.py` normalizes `(normal + axis_u*u + axis_v*v)`, which
bunches vertices toward the cube corners: at 32 subdivisions a face-centre quad
spans 3.58 degrees and a corner quad only 1.74, a ratio of **2.05**. Half the
resolution in one place, twice the metres per vertex in the other. Apply the
tangent warp before normalizing:

```python
u' = tan(u * pi / 4)     # u in [-1, 1]
v' = tan(v * pi / 4)
radial = normalize(normal + axis_u * u' + axis_v * v')
```

That gives near-uniform angular spacing — the same ratio falls to **1.06** —
so a vertex is worth about the same number of metres wherever it sits, which is
what makes a fixed 65x65 tile grid mean the same thing on every tile. It
changes every existing vertex position,
so it is a breaking change against the current `quad_planet.blend` and should
land before any tile is authored against the grid.

### Tile identity

A tile is `(face, depth, u, v)` — face `0..5`, and `u, v` in `0..2^depth`.
That tuple is the filename, the Blender collection name and the key in every
manifest. Never an array index: an index shifts the moment a tile is added,
which is the same reasoning that makes animation clips choose
[by name](../../docs/project-guide.md#animation).

### Neighbours

Within a face, neighbours are `(u±1, v)` and `(u, v±1)`. Across the 12 face
edges the axes rotate and sometimes flip. The obvious implementation is a
hand-written table mapping `(face, edge) -> (neighbour face, edge,
orientation)`, and getting one entry wrong produces terrain that looks fine
until you walk over a face boundary and find it mirrored.

`VertexGrid` **derives the correspondence instead of declaring it**: it hashes
each boundary point's rounded direction, and two faces that compute the same
point by two different routes land on the same key and therefore the same
canonical vertex. There is no table to get wrong, and the rotations and flips
never have to be written down at all.

What makes that safe is that the result is checkable. A cube subdivided `n`
times per face has exactly `6n^2 + 2` distinct vertices, so the constructor
asserts the count: it is only right if all 12 edges and all 8 corners welded.
`test_every_face_edge_is_shared` goes further and asserts that vertices are
shared by exactly one, two or three faces and that precisely 8 are shared by
three -- the cube corners, found rather than special-cased.

## Scale

Kept from the existing script, which is Outer Wilds scale and already has the
castle placed against it:

| | |
|---|---|
| Radius | 300 m |
| Circumference | ~1885 m |
| Face edge arc | ~471 m |

Proposed working grid, **depth 2**:

| Depth | Tiles | Tile edge | Role |
|---|---|---|---|
| 0 | 6 | ~471 m | LOD1, the space mesh |
| 2 | 96 | ~118 m | LOD0, the working tile grid |

At depth 2 a tile is a little smaller than the castle grounds' 164x150 m
footprint, so the castle occupies roughly one tile plus a margin.

Each LOD0 tile carries a **65x65 vertex grid** — 64 quads, 2^6+1, the `+1`
being the shared ring. That is ~1.84 m between vertices, which is a sensible
floor for platforming at this scale. The whole planet is then ~405k vertices
and ~790k triangles, which Blender handles without complaint as long as tiles
are separate objects.

These numbers are a starting point, not a constraint of the design; depth,
grid size and radius are all manifest fields.

## Input rasters

Six square images per layer, one per cube face, in `faces/`. Six face maps
rather than one equirectangular map: no pole distortion, no sampling density
collapse, and each face paints independently.

| Layer | Format | Sampling |
|---|---|---|
| `elevation_N.png` | 16-bit grayscale | bilinear |
| `material_N.png` | 8-bit indexed | **nearest** |
| `lock_N.png` | 8-bit grayscale | nearest |

**16-bit elevation is not optional.** 8-bit gives 256 height steps; across a
useful altitude range that terraces visibly on any gentle slope, and the
terraces are exactly the shallow gradients the player walks on.

Elevation maps `0..65535` onto `[min_altitude, max_altitude]` from the
manifest, as an offset from `RADIUS`.

`material_N.png` is an index into the material table — biome, texture set and
surface type in one. It must be sampled **nearest-neighbour**; bilinear
interpolation between index 3 and index 7 produces index 5, which is a
different material, and the artifact is a fringe of wrong terrain along every
material boundary.

`lock_N.png` marks regions the generator may not touch, at raster resolution.
Coarser than tile-level locking and useful for protecting a hand-sculpted
hollow without freezing the whole tile.

### Face map resolution

Face maps should be `4 * 64 + 1 = 257` px to map 1:1 onto depth-2 tile
vertices. Author at **513 px** — a 2x supersample — so pushing to depth 3
later does not mean repainting.

### The 12 face seams

Six independently painted images do not agree where the cube faces meet. This
is the one place the raster layer needs a tool of its own:

```
planetgen stitch-faces
```

It walks the 12 shared edges plus the 8 corners and reconciles them —
averaging elevation, and for the index layers taking the lower face index as
authoritative, since averaging two material indices is meaningless. Run it
after painting and before building. A `--check` mode reports disagreement
without writing, for CI.

## Seams

The requirement is a shared vertex ring, and with determinism off, "shared"
has to be *stored* rather than *recomputed*.

### The edge record store

`edges/` holds one record per shared edge, keyed by the canonical edge
identity — the ordered pair of tile IDs, lower first, so both tiles name the
same record. A record holds the vertex ring: positions, normals, material
indices, and a version.

Building a tile:

1. For each of its four edges, if a record exists, **read the ring from it**.
2. Otherwise sample the ring from the heightfield and **write the record**.
3. Generate the interior, constrained to that ring.
4. Same for the four corners, from the corner record store.

That is what makes "regenerate a tile given the edges of the adjacent tiles"
work with arbitrary neighbours: the neighbour does not need to exist, be
generated, or be reachable — only its shared record does.

### Publishing an edit

A hand edit does not silently win. Editing a tile and then:

```
planetgen publish-edges <tile-id>
```

promotes that tile's rings into the record store, bumps their versions, and
marks the neighbours dirty. Authored and locked tiles publish automatically on
import — hand-made content is always authoritative over generated content.

Without this step a hand edit changes only the tile you edited, and the
neighbour keeps the old ring. That is the correct default: it means an
experimental edit cannot quietly corrupt the tiles around it.

### Corners

Interior corners are shared by four tiles; the 8 cube corners by three. Both
get records in the same store, keyed by the sorted tuple of participating tile
IDs. A corner is written by whichever tile builds first, or by a publish,
exactly like an edge.

### Normals across the seam

Matching geometry is not enough. Normals computed from a tile's own vertices
alone go wrong on the boundary row, because those vertices have no neighbours
on the far side — and the artifact is a hard lighting seam on terrain that is
geometrically perfect.

Every tile therefore generates with a **one-cell apron**: an extra ring of
vertices sampled from beyond its footprint, used for normals and for anything
else needing a neighbourhood, then discarded before output. The apron comes
from the edge record where one exists and from the heightfield otherwise.

### Creases into authored tiles

A shared ring guarantees C0 continuity — no gap, no crack. It does not
guarantee C1, so a generated tile meeting an authored one meets it at a
visible crease.

Generated tiles therefore blend toward the authored neighbour's interior
*gradient* over a band of N cells, not just its boundary heights. This is the
tile-grid form of the smoothstep flatten `build_quad_planet.py` already
applies around the castle footprint, and reuses the same falloff.

## Authored content

Two mechanisms, because hand-made content arrives at two different sizes.

**Authored tiles** are whole tiles supplied as meshes. They must present a
conforming boundary ring — same vertex count, same positions — which
`planetgen check` verifies on import. They are locked, they publish their
edges, and the generator only ever reads them.

**Authored patches** are sub-tile assets placed at a lat/long with a footprint,
which is what the castle grounds currently is. The existing tangent-plane
placement and rectangular smoothstep flatten in `build_quad_planet.py` carry
over unchanged; the only difference is that the flatten now writes into tile
heightfields rather than into one monolithic mesh, and a patch straddling a
tile boundary writes into both.

Authored content is flat, and the planet is not. At 300 m radius the sagitta
across the castle's 164 m footprint is about 11 m — far too much to ignore.
The tangent patch is the answer: the terrain is bent to meet the flat asset
rather than the asset being bent to meet the terrain, and the blend band
absorbs the difference.

## Cross-tile features

Features live in **planet space**, not tile space. Each tile asks which
features intersect its footprint plus apron and applies whatever crosses it.
Because a feature is never cut up or assigned to an owner tile, it crosses
seams for free — the edge record only has to carry the *result* at the
boundary ring.

Authored as Blender curve and mesh objects in a features `.blend`, exported by
a small `bpy` script to a plain `features.json` that the core reads. Blender
is the editor; it is not in the loop at generation time.

### Splines — rivers, roads, paths

A curve in planet space plus a cross-section profile and a width. The tile
mesher carves or raises terrain along the curve. Because the profile is
applied in planet space, a river arriving at a tile boundary leaves the
neighbour at exactly the same depth without either tile knowing about the
other.

### Cliff and ledge lines

Traversability boundaries authored as lines rather than left to fall out of
the heightmap, because a heightfield cannot represent a vertical face at all —
it can only make a very steep ramp, and a very steep ramp is a *different
gameplay object* from a cliff.

A cliff line splits the edge loop it crosses: the mesher inserts a doubled
vertex row along the curve and offsets one side by the authored height delta,
producing a wall strip.

This is the one feature that breaks the pure grid, and it needs care at seams.
A cliff crossing a tile boundary means the boundary ring is no longer 65
vertices — it has extra ones where the cliff punches through. **The edge
record must carry the split**, as an explicit list of inserted vertices with
their parameter along the edge, or the two tiles will disagree about how many
vertices their shared ring has. `planetgen check` tests this specifically.

### Region boundaries

Closed polygons in planet space assigning material index and surface type,
rasterized per tile. These override the `material_N.png` raster, which makes
them the tool for a deliberate level-design decision — this ledge is ice, this
courtyard is stone — as opposed to the painted broad strokes.

## Traversability

The thing actually being designed for, so it is a validated output rather than
an emergent property.

[`src/level.rs:34`](../../src/level.rs#L34) defines `GROUND_NORMAL_Y = 0.7`:
a triangle is a floor you stand on if its normal leans less than that off
vertical, and a wall you are pushed out of otherwise. About 45.6 degrees.

**On a sphere, "vertical" is the radial direction, not `+Y`.** The generator
classifies with `dot(normal, radial) > 0.7`. The runtime currently tests
`normal.y` directly, which is correct for a flat level and wrong for a planet;
see [Bevy, now](#bevy-now).

Each triangle gets a surface type from the material table, written alongside
the collision mesh. The table is a small committed file listing the handful of
`SURFACE_*` names and values the planet needs — not a re-read of
`reference/`, which the [source-of-truth policy](../../docs/pipeline.md) puts
out of bounds.

### The reachability check

```
planetgen check --traversal
```

Flood-fills the walkable triangles across the whole planet, crossing tile
seams, and reports:

- Disconnected walkable regions — a plateau nothing reaches.
- Walkable pockets below a minimum area — slivers the player cannot stand on.
- Steep bands narrower than a step — terrain that reads as walkable and is not.
- Cliff lines with no authored route around or over them.

None of these are errors. They are a report, because "there is no way up
there" is sometimes exactly the design. But they should be *chosen*, and on a
planet the size of this one an unreachable region is very easy to create by
accident and very hard to notice by eye.

## Water

**The sea is one sphere at sea level.** Not a heightfield layer, not a per-tile
mask, not a list of boxes. Every basin on the planet is under the same surface
at the same radius, so a bay, a river mouth and the deep ocean are one object
with one number behind them, and there is nothing for two tiles to disagree
about at a seam. Land pokes through it wherever the terrain rises past
`sea_level`; the shoreline is the intersection of the two meshes, authored by
nobody and exact everywhere.

It is built on the same cube-sphere grid as the terrain — `ocean.py`, 64 quads
to a face edge, welded by the same `VertexGrid` — for the same reason
everything else here is: face coordinates scaled by arc length are the closest
thing this planet has to a metre ruler, which is what the surface texture tiles
against, and the vertices weld across all 12 face edges so the sea has no
cracks and no poles. It is deliberately *not* trimmed to the basins. A sea that
stopped at the shoreline would need the shoreline as geometry, which is a
cross-tile feature and the most fiddly kind; a whole sphere costs 49,152
triangles and needs none of it.

The texture is the castle's own water sheet, `assets/bevy/water.png`, at the
same 20.48 m per repeat, so the two bodies of water in this game are visibly
the same substance.

### How sea level reaches the game

As geometry, in a node named `ocean`, in the same `.glb` as the terrain.

The alternative was arithmetic, and it does not work. The game measures a
planet by averaging the distance from its centre out to every vertex, which
is an average over the mountains and the seabed alike: on this planet it comes
to 304.1 m against a sea at 300.0. Four metres is the difference between a
beach and a drowned one. Sea level is a number in `planet.json`, the game
deliberately does not read `planet.json` — one file for a planet, not two —
and a sphere of water is a perfectly good way to write a number down, since
every one of its vertices is exactly `radius + sea_level` from the centre.

That puts one rule on both sides of the fence: [`src/world.rs`](../../src/world.rs)
skips the `ocean` node when it reads collision out of the glTF, and takes its
mean radius as sea level. Reading it as ground instead would be a glass floor
over the whole world.

### What the sea does not do

No lakes above sea level: a lake on a slope is not a sphere and is not
designed. No waves, no tide, no flow. The surface drifts, and it drifts by
*turning*: a sphere spun about its own centre occupies exactly the space it
did before, so the ripples slide across it and the coastline does not move.

## LODs

**LOD0** — the working tile grid. Depth 2, 65x65 per tile, full material and
collision data. What the player walks on.

**LOD1** — the space mesh. Six meshes at depth 0, 65x65 each, with colour and
normal maps baked from LOD0.

LOD1 is **derived from LOD0 by resampling, never generated independently**.
This is the single most important rule in this section: two generators
producing "the same" terrain at two resolutions will disagree, and the
disagreement shows up as the planet visibly changing shape as you approach it.
Deriving guarantees they agree at the silhouette, which is what
seamless space-to-surface travel actually requires.

Baking colour and normals from LOD0 into LOD1's maps is the same trick
[`src/impostor/bake.rs`](../../src/impostor/bake.rs) already plays for
enemies, at a different scale.

Both LODs get **skirts** — a short vertical rim dropped around each tile's
boundary. Even with a shared ring, floating-point interpolation across the
LOD transition can open a hairline crack, and a skirt hides it for a few
vertices' worth of geometry rather than a solution.

Where the transition happens, and how it blends, is the game's problem, not
the generator's. The generator's contract is that both LODs exist, agree, and
are addressed by the same tile IDs.

## Architecture

The core is **pure Python and numpy with no `bpy` import anywhere**. Blender
appears only in a thin io layer that turns arrays into meshes.

This is not a stylistic preference. A bpy-free core runs under plain `python3`
in WSL, tests headlessly in milliseconds, parallelizes across tiles with
`multiprocessing`, and does not inherit Blender's scene-size limits. It also
matches how [`tools/`](../../tools/) already works.

The Blender MCP servers are for **inspection only** — looking at a tile,
checking a placement. Never a build dependency. The build must run from a
clean checkout with nothing listening on a port.

```
experimental/planet_gen/
  readme.md
  planet.json              manifest: radius, depth, grid, altitude range,
                           material table, face map paths
  faces/                   authored input rasters, 6 per layer
  features/features.json   splines, cliff lines, regions, in planet space
  tiles/<face>/<depth>/<u>_<v>.npz
  edges/                   shared ring records
  authored/               hand-made tiles and patches
  out/                     planet.blend, planet.glb (LOD0),
                           planet_lod1.glb (LOD1), preview renders
  planetgen/               core, no bpy
    cubesphere.py          face<->sphere, tile ids, the neighbour table
    rasters.py             face map loading, sampling, stitch-faces
    tiles.py               tile data model, manifest, dirty tracking
    seams.py               edge and corner records, apron assembly
    features.py            splines, cliffs, regions
    mesh.py                heightfield -> vertices, triangles, normals
    surface.py             slope classification, surface types
    ocean.py               the sea-level sphere
    lod.py                 LOD1 derivation and baking
    check.py               seam, apron and traversal validators
    cli.py
  blender/                 bpy only, here and nowhere else
    export_tiles.py        tile arrays -> collections -> .blend
    export_features.py     curve objects -> features.json
  tests/
```

### CLI

```
planetgen build <tile-id>          one tile
planetgen build --face 3           one face
planetgen build --all              the whole planet
planetgen build --dirty            only what is stale
planetgen publish-edges <tile-id>  promote hand edits to neighbours
planetgen stitch-faces [--check]   reconcile the 12 face-map seams
planetgen check [--traversal]      validate
planetgen export [--lod 0|1]       write the .blend
```

## Staleness

With no determinism there is no way to recompute a tile and compare, so
staleness is tracked explicitly. `planet.json` records per tile:

- Hashes of the input raster regions it reads.
- Versions of the edge and corner records it consumed.
- The IDs of the features intersecting its footprint, and their versions.
- A generator version.
- `locked`, and `hand_edited`.

A tile is dirty when any of those has moved. `--dirty` rebuilds exactly the
dirty set, which is what makes "work on individual tiles or the whole planet"
the same command with a different flag.

`hand_edited` is the safety catch: `--all` skips hand-edited tiles unless
given `--force`, so a whole-planet rebuild cannot silently destroy an
afternoon of sculpting.

## Blender output

One collection per tile, named by tile ID, so a re-export replaces a tile
rather than accumulating copies. Each LOD collection also holds the sea, as one
object named `ocean` — the name is load-bearing, and [Water](#water) says why. Tiles are separate objects, not one joined
mesh — the whole point is working on them one at a time.

Do not load the whole planet at LOD0 for authoring. `export --lod 1` plus the
handful of tiles being worked on is the working view.

### glTF

`export_tiles.py` writes a `.glb` per LOD next to the `.blend`, one node per
tile, keeping the tile ids as node names. Two details are load-bearing:

- **Y-up.** glTF is Y-up and Blender is Z-up, so the export applies the
  conversion, as [`tools/blend_to_glb.py`](../../tools/blend_to_glb.py) does
  for every other asset. Latitude is about `+Z` in the `.blend` and `+Y` in
  the `.glb`.
- **Colour rides on `COLOR_0`.** The material table becomes a per-vertex
  colour, so the `.glb` needs no textures at all yet. That requires the shader
  to read it through a *Color Attribute* node: the exporter decides whether a
  mesh's colours are used by walking the graph, and it does not recognize the
  generic Attribute node. Getting this wrong renders correctly in Blender and
  exports an untinted planet -- the same silent failure as an EEVEE-targeted
  Material Output, which is why both are forced here.

Blender is Z-up and the game is Y-up; `build_quad_planet.py` already builds
Z-up with latitude about `+Z`, and the glTF export handles the conversion.
Keep that convention.

### Textures

Render96, following the precedent in
[`docs/pipeline.md`](../../docs/pipeline.md): the specific terrain textures
the planet uses are **copied into `assets/` and committed**, not referenced
in place. The pack is 12 GB of untracked third-party material, and the last
time textures pointed into it a fresh clone parsed fine and then drew the
entire castle grounds untextured.

Material assignment is **triplanar**, not UV. A sphere has no non-degenerate
UV parameterization, and SM64-era textures are small tiling images that
triplanar projection suits well. It also sidesteps the pole and seam problems
entirely.

The portable glTF output uses dominant-axis projection per triangle, stored as
UV islands because glTF has no procedural triplanar shader. Texture scale stays
in world metres and the projection can change axis without splitting PlanetGen's
welded position/normal data. The five selected Render96 images live in
`assets/planet_gen/textures/`; exports never reference the external HD pack.

The sea is the exception, and takes `assets/bevy/water.png` instead — the
castle's own sheet, already committed for the moat. Same rule, different
committed source: nothing here reaches into the untracked pack.

The same redistributability caveat in `pipeline.md` applies to everything
here.

## Tests

Cheap, they fail loudly, and they cover the failure modes that are invisible
by eye:

- **Seam** — for every pair of adjacent tiles, boundary rings are identical
  within epsilon. Positions, normals and material indices.
- **Cliff seam** — a cliff line crossing a tile boundary produces the same
  vertex count and the same split parameters on both sides.
- **Neighbour table** — all 12 face edges and 8 corners resolve, round-trip,
  and agree on orientation. Exhaustive, since it is only 20 cases.
- **Apron** — a tile built with neighbours present and a tile built with only
  edge records produce identical normals on the boundary row.
- **LOD agreement** — LOD1 vertices lie within tolerance of the LOD0 surface.
- **Traversal** — a known-good planet has exactly one walkable region.
- **Authored ring** — an authored tile's boundary conforms before import.
- **Sea** — every vertex of it is exactly at sea level, it winds outward, and
  the terrain agrees with it about which vertices are under water. The game has
  its own half of this: that the sea in `planet.glb` is found, is not collision,
  and is deep enough over the basins to be water rather than a lid.

Tests live in `tests/` and run under plain `python3`, no Blender.

## Bevy, now

The game loads this planet. `out/planet.glb` is copied to
`assets/bevy/planet.glb` by the `planet` stage of
[`tools/build_assets.py`](../../tools/build_assets.py), and the pause menu's
level page puts it up. What the game does with it:

- **Collision is the render mesh.** No separate collision export and no blob:
  [`src/world.rs`](../../src/world.rs) reads the vertices and indices back out
  of the loaded glTF and hands them to `LevelData::planet`. The
  `include_bytes!` embed in [`src/level.rs`](../../src/level.rs) was never
  going to scale to 786,432 triangles, and this needs it not to.
- **The collision grid is a cube-sphere face grid**, 96x96 cells on each of the
  six faces, filed by the direction a triangle points rather than by `(x, z)`.
  The flat grid could not be reused: projected onto `(x, z)` the far side of a
  planet lands on top of the near side.
- **`GROUND_NORMAL_Y` is tested against `dot(normal, up)`**, exactly as
  predicted below, where `up` is handed in by the gravity resource. Same
  constant, different up.
- **Gravity towards the core exists**, and is one resource with two shapes:
  [`src/gravity.rs`](../../src/gravity.rs).
- **The sea is water.** [`src/level.rs`](../../src/level.rs) answers "how far
  under the surface is this point" for a sphere of water as readily as for the
  castle's boxes, measured along the local up either way, so Mario swims in the
  ocean, the Hero wades in it, and the camera takes the underwater fog with it
  when it goes under. The spawn search picks the lowest dry land against sea
  level, which puts the player on a beach.

Still not built, and still specified so that none of it needs a regenerate:

- Per-tile collision meshes with surface types in the `.npz`. The game
  classifies a surface from the triangle's own normal instead, which is enough
  to tell floor from wall and nothing more.
- Streaming. All 96 tiles load at once, which is 14 MB of glTF and a visible
  pause on the level change.
- Anything living on it. Enemies, pipes, the squad and the far-crowd flow field
  all assume a flat level; none of that was needed to walk around a planet.

## Open questions

- **Depth 2 or 3?** 96 tiles at 118 m, or 384 at 59 m. Depth 3 is a better fit
  for detailed level design and four times the tiles to manage. Depth 2 is the
  proposal; the manifest makes it a one-line change until tiles are authored.
- **Cliff height limit.** A wall strip is a vertical quad; past some height it
  should probably become authored geometry instead.
- **Lakes.** The ocean is [a sphere at sea level](#water) and that is settled.
  A lake on a slope is not a sphere, and nothing here can express one: it needs
  a surface height that varies across the planet, which is a per-region number
  rather than a per-planet one. Rivers are the same question with a gradient in
  it, and both wait on [cross-tile features](#cross-tile-features).
- **Does the castle stay flat?** The tangent patch is the current answer, and
  at 11 m of sagitta across its footprint it is a large flat spot on a small
  planet. Visible from space, possibly fine.
