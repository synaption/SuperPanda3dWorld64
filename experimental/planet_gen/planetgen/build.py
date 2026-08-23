"""Turn the authored rasters into tiles.

The architecture is one global vertex array plus per-tile slices of index. That
is what makes the seam guarantees free rather than enforced: adjacent tiles do
not hold matching copies of a boundary ring, they hold *the same* ring, and
vertex normals are accumulated across the whole planet so a boundary vertex
sees the triangles on both sides of the seam. The readme's one-cell apron is
what a single-tile rebuild needs; a whole-planet build gets the same answer by
having every neighbour present already.
"""

import numpy as np

from . import rasters, surface
from .cubesphere import (VertexGrid, grid_parameters, tile_quad_indices,
                         tile_name, tiles_at)
from .manifest import grid_size


class Planet:
    def __init__(self, root, m):
        self.root, self.m = root, m
        self.n = grid_size(m)
        self.grid = VertexGrid(self.n)
        self.directions = self.grid.directions

    def read_elevation(self):
        """Sample every face's elevation raster onto the shared vertex array.

        Faces are walked in reverse so face 0 writes last: where two faces meet,
        the lower face index is authoritative. One vertex, one height, so a
        disagreement between two painted maps cannot become a crack -- it just
        means the non-owner face's edge pixels were not the ones used.
        """
        t = grid_parameters(self.n)
        tu = t[np.newaxis, :]
        tv = t[:, np.newaxis]
        heights = np.zeros(self.grid.count, dtype=np.float64)
        for face in reversed(range(6)):
            raster = rasters.load_elevation(
                rasters.face_path(self.root, rasters.ELEVATION, face))
            values = rasters.sample_bilinear(raster, np.broadcast_to(tu, (self.n + 1, self.n + 1)),
                                             np.broadcast_to(tv, (self.n + 1, self.n + 1)))
            heights[self.grid.ids[face]] = values
        return heights

    def triangles(self):
        """Every triangle on the planet, indexing the shared vertex array."""
        out = []
        for face in range(6):
            ids = self.grid.ids[face]
            a, b = ids[:-1, :-1], ids[:-1, 1:]
            c, d = ids[1:, 1:], ids[1:, :-1]
            out.append(np.stack([a, b, c], axis=-1).reshape(-1, 3))
            out.append(np.stack([a, c, d], axis=-1).reshape(-1, 3))
        return np.concatenate(out).astype(np.int64)

    def vertex_normals(self, positions, triangles):
        """Area-weighted vertex normals, accumulated planet-wide.

        Accumulating globally rather than per tile is the whole reason the
        boundary rows light correctly: a vertex on a seam gets the triangles
        from both tiles, so there is no lighting seam over geometry that is
        already exact.
        """
        a, b, c = (positions[triangles[:, i]] for i in range(3))
        face_normal = np.cross(b - a, c - a)
        acc = np.zeros_like(positions)
        flat = triangles.ravel()
        for k in range(3):
            acc[:, k] = np.bincount(flat, weights=np.repeat(face_normal[:, k], 3),
                                    minlength=len(positions))
        length = np.linalg.norm(acc, axis=1, keepdims=True)
        return acc / np.maximum(length, 1e-12)

    def build(self):
        m = self.m
        h01 = self.read_elevation()
        altitude = m["min_altitude"] + h01 * (m["max_altitude"] - m["min_altitude"])
        positions = self.directions * (m["radius"] + altitude)[:, np.newaxis]
        tris = self.triangles()
        normals = self.vertex_normals(positions, tris)
        slope = surface.radial_slope(normals, self.directions)
        material = surface.classify(altitude, slope, m["sea_level"],
                                    m["snow_line"], m["steep_cos"])
        self.altitude = altitude
        self.positions = positions
        self.normals = normals
        self.material = material
        self.slope = slope
        self.all_triangles = tris
        return self

    def tile(self, face, tu, tv, depth=None, res=None):
        """One tile as plain arrays, sliced out of the planet-wide build."""
        depth = self.m["depth"] if depth is None else depth
        res = self.m["tile_res"] if res is None else res
        stride = self.n // (res * (2 ** depth))
        u0, v0 = tu * res * stride, tv * res * stride
        block = self.grid.ids[face,
                              v0:v0 + res * stride + 1:stride,
                              u0:u0 + res * stride + 1:stride]
        ids = block.ravel()
        local = tile_quad_indices(res)
        walk = surface.walkable_triangles(self.positions[ids], local,
                                          self.m["ground_normal"])
        return {
            "tile": np.array([face, depth, tu, tv], dtype=np.int32),
            "vertex_ids": ids.astype(np.int64),
            "positions": self.positions[ids].astype(np.float32),
            "normals": self.normals[ids].astype(np.float32),
            "material": self.material[ids].astype(np.uint8),
            "altitude": self.altitude[ids].astype(np.float32),
            "triangles": local.astype(np.uint32),
            "walkable": walk,
        }

    def write_tiles(self, lod=0):
        """Write every tile at a LOD.

        LOD1 is a strict subsample of the LOD0 grid, not a second generation
        pass. Its vertices *are* LOD0 vertices, so the two cannot disagree
        about the planet's silhouette -- which is what seamless approach from
        space actually requires.
        """
        m = self.m
        depth = m["depth"] if lod == 0 else m["lod1_depth"]
        res = m["tile_res"]
        out = self.root / "tiles" / f"lod{lod}"
        out.mkdir(parents=True, exist_ok=True)
        written = []
        for face, tu, tv in tiles_at(depth):
            data = self.tile(face, tu, tv, depth=depth, res=res)
            path = out / f"{tile_name(face, depth, tu, tv)}.npz"
            np.savez_compressed(path, **data)
            written.append(path)
        return written
