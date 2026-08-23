"""Cube-sphere topology: faces, the tangent warp, and the shared vertex grid.

The whole seam problem is solved here rather than downstream. Every vertex on
the planet gets one canonical index, and the three or four faces that meet at a
shared edge or corner all resolve to that same index. Tiles are then nothing
but slices of index into one global vertex array, so "adjacent tiles share a
vertex ring" is true by construction -- there is no comparison to make, no
tolerance to tune, and no way for two tiles to disagree.
"""

import math

import numpy as np

FACE_NAMES = ("+X", "-X", "+Y", "-Y", "+Z", "-Z")

# (normal, axis_u, axis_v), consistently wound viewed from outside, matching
# tools/build_quad_planet.py so placements authored against it still land.
FACES = (
    ((1, 0, 0), (0, 1, 0), (0, 0, 1)),
    ((-1, 0, 0), (0, -1, 0), (0, 0, 1)),
    ((0, 1, 0), (-1, 0, 0), (0, 0, 1)),
    ((0, -1, 0), (1, 0, 0), (0, 0, 1)),
    ((0, 0, 1), (1, 0, 0), (0, 1, 0)),
    ((0, 0, -1), (1, 0, 0), (0, -1, 0)),
)

# Dividing by tan(pi/4) rather than trusting it to be 1.0 is what makes warp(1)
# come back as exactly 1.0. math.tan(math.pi / 4) is 0.9999999999999999, and a
# face edge that lands a half-ulp short of the cube's edge does not match the
# neighbouring face's edge, which is precisely the thing this module exists to
# guarantee.
_TAN45 = math.tan(math.pi / 4.0)


def warp(t):
    """Map uniform face coordinates to uniform *angles* on the sphere.

    Without this, normalizing (normal + u, v) bunches vertices toward the cube
    corners: at 32 subdivisions a face-centre quad spans 3.58 degrees and a
    corner quad 1.74, a ratio of 2.05. The warp takes that to 1.06, so a vertex
    is worth about the same number of metres wherever it sits and a fixed tile
    grid means the same thing on every tile.
    """
    return np.tan(np.asarray(t, dtype=np.float64) * (math.pi / 4.0)) / _TAN45


def face_axes(face):
    n, au, av = FACES[face]
    return (np.array(n, dtype=np.float64),
            np.array(au, dtype=np.float64),
            np.array(av, dtype=np.float64))


def grid_parameters(n):
    """The n+1 face coordinates of a grid with n quads along an edge.

    linspace pins both endpoints at exactly -1.0 and 1.0, which the shared-edge
    matching below depends on.
    """
    return np.linspace(-1.0, 1.0, n + 1, dtype=np.float64)


def face_directions(face, n):
    """Unit directions for one face's (n+1) x (n+1) grid, indexed [v, u]."""
    normal, axis_u, axis_v = face_axes(face)
    t = warp(grid_parameters(n))
    u = t[np.newaxis, :, np.newaxis]
    v = t[:, np.newaxis, np.newaxis]
    d = normal + axis_u * u + axis_v * v
    return d / np.linalg.norm(d, axis=-1, keepdims=True)


class VertexGrid:
    """One canonical index per vertex on the planet, shared across faces.

    ``ids[face, v, u]`` is the canonical index of that face grid point, and
    ``directions[index]`` is its unit direction. A cube subdivided n times per
    face has exactly 6n^2 + 2 distinct vertices; anything else means the edge
    matching failed, so the constructor asserts it.
    """

    def __init__(self, n):
        self.n = n
        self.ids = np.full((6, n + 1, n + 1), -1, dtype=np.int64)
        directions = []
        lookup = {}

        # Boundary points first, through a dictionary keyed on the rounded
        # direction. Two faces meeting at an edge compute the same point by two
        # different routes; rounding to 12 places is far tighter than the grid
        # spacing and far looser than the float error between those routes.
        for face in range(6):
            d = face_directions(face, n)
            edge = np.zeros((n + 1, n + 1), dtype=bool)
            edge[0, :] = edge[-1, :] = edge[:, 0] = edge[:, -1] = True
            for v, u in np.argwhere(edge):
                # Adding 0.0 folds -0.0 onto 0.0, which otherwise hashes apart.
                key = tuple(np.round(d[v, u], 12) + 0.0)
                index = lookup.get(key)
                if index is None:
                    index = len(directions)
                    lookup[key] = index
                    directions.append(d[v, u])
                self.ids[face, v, u] = index

        # Interior points belong to exactly one face, so they need no lookup.
        for face in range(6):
            d = face_directions(face, n)
            free = self.ids[face] < 0
            count = int(free.sum())
            start = len(directions)
            self.ids[face][free] = np.arange(start, start + count)
            directions.extend(d[free])

        self.directions = np.asarray(directions, dtype=np.float64)
        expected = 6 * n * n + 2
        if len(self.directions) != expected:
            raise AssertionError(
                f"cube-sphere welding produced {len(self.directions)} vertices, "
                f"expected {expected}")

    @property
    def count(self):
        return len(self.directions)

    def tile_ids(self, face, depth, tu, tv, res):
        """The (res+1) x (res+1) block of canonical indices a tile covers."""
        if res * (2 ** depth) != self.n:
            raise ValueError("tile resolution does not match this grid")
        u0, v0 = tu * res, tv * res
        return self.ids[face, v0:v0 + res + 1, u0:u0 + res + 1]


def tile_quad_indices(res):
    """Triangle indices into a (res+1)^2 tile block, wound outward."""
    row = res + 1
    j, i = np.meshgrid(np.arange(res), np.arange(res), indexing="ij")
    a = (j * row + i).ravel()
    b, c, d = a + 1, a + row + 1, a + row
    return np.stack([np.stack([a, b, c], axis=1),
                     np.stack([a, c, d], axis=1)], axis=1).reshape(-1, 3)


def tiles_at(depth):
    """Every tile id at a depth, as (face, tu, tv)."""
    side = 2 ** depth
    return [(f, tu, tv)
            for f in range(6) for tv in range(side) for tu in range(side)]


def tile_name(face, depth, tu, tv):
    return f"{face}_{depth}_{tu}_{tv}"
