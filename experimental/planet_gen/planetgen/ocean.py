"""The sea: one sphere at sea level.

A planet ocean is a sphere, and that is the whole design. It is not a
heightfield layer, not a per-tile mask and not a box: every basin on the planet
is under the same sphere, so a river mouth, a bay and the deep ocean are one
surface with one radius, and there is nothing for two tiles to disagree about.
Land pokes through it wherever the terrain rises past `sea_level`; the
shoreline is the intersection, drawn by nobody and exact everywhere.

Built on the same cube-sphere grid as the terrain, for two reasons that are
both about not inventing a second parameterization of a sphere:

- The tangent warp already makes face coordinates near-uniform in angle, so
  face coordinates scaled by arc length are the closest thing this planet has
  to a metre ruler laid over the sea. That is what the surface texture tiles
  against, at a fixed size in metres however far you sail.
- The vertices weld across all 12 face edges exactly as the terrain's do, so
  the sea has no cracks and no poles.

The mesh is deliberately *not* trimmed to the basins. A sea that stopped at the
shoreline would need the shoreline as geometry -- a cross-tile feature, in the
readme's terms, and the most fiddly kind. A whole sphere costs 49,152 triangles
and needs no such thing.
"""

import math

import numpy as np

from .cubesphere import VertexGrid, grid_parameters, welded_triangles

#: Quads along one face edge. 64 matches the LOD1 terrain tile, and puts the
#: sphere within 2.3 cm of round at 300 m -- a millimetre of sagitta per metre
#: of surface, which no camera in this game gets close enough to see.
DEFAULT_RES = 64

#: Metres of sea per repeat of the surface texture. The castle's water sheet
#: uses 20.48 m (`src/water.rs`), and the ocean is the same sheet wrapped round
#: a planet, so it tiles at the same size rather than at a second one.
METRES_PER_REPEAT = 20.48


def sea_radius(m):
    """Distance from the planet's centre to the water's surface."""
    return m["radius"] + m["sea_level"]


def build(m, res=DEFAULT_RES):
    """The sea-level sphere as plain arrays, ready for the Blender io layer.

    UVs are per triangle corner rather than per vertex, because a vertex on a
    face edge belongs to two faces and the two faces measure it differently.
    Per-loop UVs are what Blender stores anyway, so this costs nothing and
    keeps the texture continuous across the interior of every face -- the
    discontinuity is pushed onto the 12 cube edges, where the terrain's own
    triplanar mapping already has one.
    """
    grid = VertexGrid(res)
    radius = sea_radius(m)
    positions = grid.directions * radius
    triangles = welded_triangles(grid)

    # Face coordinates are near-uniform in angle, so a quarter turn of arc
    # spread over t in [-1, 1] makes t * radius * pi / 4 a distance in metres.
    metres = grid_parameters(res) * radius * (math.pi / 4.0) / METRES_PER_REPEAT
    u = np.broadcast_to(metres[np.newaxis, :], (res + 1, res + 1))
    v = np.broadcast_to(metres[:, np.newaxis], (res + 1, res + 1))
    face_uv = np.stack([u, v], axis=-1)

    # The same index arithmetic welded_triangles uses, so corner k of triangle
    # t gets the UV of the grid point that triangle t names.
    uvs = []
    for _ in range(6):
        a, b = face_uv[:-1, :-1], face_uv[:-1, 1:]
        c, d = face_uv[1:, 1:], face_uv[1:, :-1]
        uvs.append(np.stack([a, b, c], axis=-2).reshape(-1, 3, 2))
        uvs.append(np.stack([a, c, d], axis=-2).reshape(-1, 3, 2))

    return {
        "positions": positions.astype(np.float32),
        # A sphere's normal is its direction. Handed to Blender as custom
        # normals so the six face patches shade as one ball.
        "normals": grid.directions.astype(np.float32),
        "triangles": triangles.astype(np.uint32),
        "uvs": np.concatenate(uvs).astype(np.float32),
        "radius": radius,
    }
