"""Static level collision: surface construction, spatial partition, queries.

The level is diced into a 16x16 grid of 1024-unit cells on X/Z, and every
triangle is filed into each cell its bounding box touches (plus a 50-unit
bleed so a query near a seam still sees both sides).  Each cell keeps three
ordered lists -- floors, ceilings, walls -- and a query walks a list and takes
the *first* triangle that passes, not the best one.

That "first, not best" rule is load-bearing.  Combined with the ordering
(floors highest-first by their *first vertex*, which is not necessarily their
highest vertex) it is what produces the familiar surface-cucking behaviour,
where a lower triangle can shadow a higher one.  Queries also truncate the
sample position to s16 before testing.  Both are reproduced deliberately;
"fixing" either changes how the game plays.
"""

import os

from concurrent.futures import ThreadPoolExecutor

import numpy as np

from .math_util import atan2s

LEVEL_BOUNDARY_MAX = 0x2000
CELL_SIZE = 0x400
CELL_COUNT = 16

FLOOR_LIST, CEIL_LIST, WALL_LIST = 0, 1, 2

# Returned when a query finds nothing.
HEIGHT_NONE = -11000.0

SURFACE_FLAG_NO_CAM_COLLISION = 1 << 1
SURFACE_FLAG_X_PROJECTION = 1 << 3

SURFACE_CAMERA_BOUNDARY = 0x0072
SURFACE_INTANGIBLE = 0x0012

_NO_CAM_COLLISION_TYPES = {0x0076, 0x0077, 0x0078, 0x007A}


def _lower_cell_index(coord):
    coord = int(coord) + LEVEL_BOUNDARY_MAX
    if coord < 0:
        coord = 0
    index = coord // CELL_SIZE
    if coord % CELL_SIZE < 50:
        index -= 1
    return max(index, 0)


def _upper_cell_index(coord):
    coord = int(coord) + LEVEL_BOUNDARY_MAX
    if coord < 0:
        coord = 0
    index = coord // CELL_SIZE
    if coord % CELL_SIZE > CELL_SIZE - 50:
        index += 1
    return min(index, CELL_COUNT - 1)


def _to_s16(value):
    """Truncate toward zero into s16, the way the C cast does."""
    return int(np.int16(int(value)))


class Surface:
    """A single collision triangle, as handed back to gameplay code."""

    __slots__ = ("index", "type", "force", "flags", "normal", "origin_offset",
                 "vertex1", "vertex2", "vertex3", "lower_y", "upper_y")

    def __init__(self, index, stype, force, flags, normal, origin_offset,
                 verts, lower_y, upper_y):
        self.index = index
        self.type = stype
        self.force = force
        self.flags = flags
        self.normal = normal
        self.origin_offset = origin_offset
        self.vertex1, self.vertex2, self.vertex3 = verts
        self.lower_y = lower_y
        self.upper_y = upper_y

    @property
    def yaw(self):
        """Facing angle of the surface, used for wall pushback and slopes."""
        return atan2s(self.normal[2], self.normal[0])

    def __repr__(self):
        return f"<Surface #{self.index} type=0x{self.type:04X} n={self.normal}>"


class WallCollisionData:
    """Accumulator for a wall query: position gets pushed out of each hit."""

    __slots__ = ("x", "y", "z", "offset_y", "radius", "walls")

    def __init__(self, x, y, z, offset_y, radius):
        self.x = float(x)
        self.y = float(y)
        self.z = float(z)
        self.offset_y = float(offset_y)
        self.radius = float(radius)
        self.walls = []


class SurfaceSet:
    """All static collision for one level area."""

    def __init__(self, vertices, tri_verts, tri_type, tri_force,
                 water_boxes=None):
        verts = np.asarray(vertices, dtype=np.int64)
        idx = np.asarray(tri_verts, dtype=np.int64)

        self.v1 = verts[idx[:, 0]]
        self.v2 = verts[idx[:, 1]]
        self.v3 = verts[idx[:, 2]]
        self.type = np.asarray(tri_type, dtype=np.int32)
        self.force = np.asarray(tri_force, dtype=np.int32)

        # Each row is (id, x1, z1, x2, z2, y). The bounds are not stored in any
        # particular order, so normalise them once rather than at every query.
        boxes = np.zeros((0, 6), dtype=np.int32) if water_boxes is None \
            else np.asarray(water_boxes, dtype=np.int32).reshape(-1, 6)
        self.water_boxes = boxes
        self._water_min_x = np.minimum(boxes[:, 1], boxes[:, 3])
        self._water_max_x = np.maximum(boxes[:, 1], boxes[:, 3])
        self._water_min_z = np.minimum(boxes[:, 2], boxes[:, 4])
        self._water_max_z = np.maximum(boxes[:, 2], boxes[:, 4])
        self._water_y = boxes[:, 5]

        self._build_normals()
        self._build_flags()
        self._build_partition()
        self._build_padded()
        self._surface_cache = {}

        # Threaded crowd resolve -- OFF by default because it measured as a net
        # loss (see `resolve_crowd`). In isolation the wall-and-floor numpy pass
        # threads ~1.7x, since numpy drops the GIL for elementwise work, but end
        # to end it regresses: the pass is a minority of a tick whose bulk is the
        # GIL-bound per-object behaviour loop, and the per-cell dispatch path a
        # clustered crowd takes is itself Python -- four threads then contend on
        # the GIL rather than run in parallel, and a clustered crowd came out ~8x
        # slower. Real parallelism here waits on the per-object AI becoming numpy
        # (structure-of-arrays), which would also thread cleanly. The switch is
        # left in, defaulting off; flip `threading_enabled` to profile it.
        self.threading_enabled = False
        self._workers = min(4, os.cpu_count() or 1)
        self._pool = None

    def _ensure_pool(self):
        if self._pool is None and self._workers > 1:
            self._pool = ThreadPoolExecutor(self._workers,
                                            thread_name_prefix="surf")
        return self._pool

    # -- water --------------------------------------------------------------

    def find_water_level(self, x, z):
        """Height of the water surface over (x, z), or None if there is none.

        Water is stored as axis-aligned boxes rather than as collision, so this
        is a plain containment test. The first containing box wins, matching
        how the engine walks the list -- overlapping boxes are a level-data
        error, not something to resolve here.
        """
        if len(self.water_boxes) == 0:
            return None
        inside = ((x >= self._water_min_x) & (x <= self._water_max_x)
                  & (z >= self._water_min_z) & (z <= self._water_max_z))
        if not inside.any():
            return None
        return float(self._water_y[int(np.argmax(inside))])

    # -- construction -------------------------------------------------------

    def _build_normals(self):
        a = (self.v2 - self.v1).astype(np.float64)
        b = (self.v3 - self.v2).astype(np.float64)

        normal = np.empty((len(self.v1), 3), dtype=np.float64)
        normal[:, 0] = a[:, 1] * b[:, 2] - a[:, 2] * b[:, 1]
        normal[:, 1] = a[:, 2] * b[:, 0] - a[:, 0] * b[:, 2]
        normal[:, 2] = a[:, 0] * b[:, 1] - a[:, 1] * b[:, 0]

        mag = np.linalg.norm(normal, axis=1)
        self.degenerate = mag < 0.0001
        safe = np.where(self.degenerate, 1.0, mag)
        self.normal = normal / safe[:, None]

        self.origin_offset = -np.einsum(
            "ij,ij->i", self.normal, self.v1.astype(np.float64)
        )

        ys = np.stack([self.v1[:, 1], self.v2[:, 1], self.v3[:, 1]], axis=1)
        self.lower_y = ys.min(axis=1) - 5
        self.upper_y = ys.max(axis=1) + 5

    def _build_flags(self):
        ny = self.normal[:, 1]
        self.is_floor = ny > 0.01
        self.is_ceil = ny < -0.01
        self.is_wall = ~self.is_floor & ~self.is_ceil

        flags = np.zeros(len(self.type), dtype=np.int32)
        # Walls facing mostly along X are tested in the X-Y plane instead.
        flags[self.is_wall & (np.abs(self.normal[:, 0]) > 0.707)] |= SURFACE_FLAG_X_PROJECTION
        for stype in _NO_CAM_COLLISION_TYPES:
            flags[self.type == stype] |= SURFACE_FLAG_NO_CAM_COLLISION
        self.flags = flags

    def _build_partition(self):
        # cells[list_index][cell_z][cell_x] -> ordered array of triangle indices
        self.cells = [
            [[[] for _ in range(CELL_COUNT)] for _ in range(CELL_COUNT)]
            for _ in range(3)
        ]

        xs = np.stack([self.v1[:, 0], self.v2[:, 0], self.v3[:, 0]], axis=1)
        zs = np.stack([self.v1[:, 2], self.v2[:, 2], self.v3[:, 2]], axis=1)
        min_x, max_x = xs.min(axis=1), xs.max(axis=1)
        min_z, max_z = zs.min(axis=1), zs.max(axis=1)

        for i in range(len(self.type)):
            if self.degenerate[i]:
                continue

            if self.is_floor[i]:
                list_index, sort_dir = FLOOR_LIST, 1
            elif self.is_ceil[i]:
                list_index, sort_dir = CEIL_LIST, -1
            else:
                list_index, sort_dir = WALL_LIST, 0

            # Sort key is the first vertex's height -- not the highest one.
            priority = int(self.v1[i, 1]) * sort_dir

            for cz in range(_lower_cell_index(min_z[i]), _upper_cell_index(max_z[i]) + 1):
                for cx in range(_lower_cell_index(min_x[i]), _upper_cell_index(max_x[i]) + 1):
                    self.cells[list_index][cz][cx].append((priority, i))

        # Descending priority, stable so equal keys keep load order.
        for list_index in range(3):
            for cz in range(CELL_COUNT):
                for cx in range(CELL_COUNT):
                    entries = self.cells[list_index][cz][cx]
                    entries.sort(key=lambda e: -e[0])
                    self.cells[list_index][cz][cx] = np.array(
                        [i for _, i in entries], dtype=np.int64
                    )

    def _build_padded(self):
        # The batched floor and wall queries used to loop over occupied cells in
        # Python, paying numpy's per-call overhead once per cell -- a couple of
        # hundred small vectorised passes a tick when a crowd is spread across
        # the level, which the profile showed dominating the whole simulation.
        # Every cell's triangle list is instead packed into one (256, M) table,
        # -1 past each cell's own length, so a query gathers every point's cell
        # list in one indexing op and runs a single vectorised pass over all
        # points at once. M is the busiest cell's triangle count (~40 here), so
        # the padding waste is small, and the result is identical to the per-cell
        # loop -- the argmax over columns still takes the first passer in the
        # cell's priority order. Only floors and walls are packed; ceilings are
        # never queried in bulk.
        self._pad_tris = {}
        for list_index in (FLOOR_LIST, WALL_LIST):
            cells = self.cells[list_index]
            m = max(1, max(len(cells[cz][cx])
                           for cz in range(CELL_COUNT)
                           for cx in range(CELL_COUNT)))
            pad = np.full((CELL_COUNT * CELL_COUNT, m), -1, dtype=np.int64)
            for cz in range(CELL_COUNT):
                for cx in range(CELL_COUNT):
                    tris = cells[cz][cx]
                    if len(tris):
                        pad[cz * CELL_COUNT + cx, :len(tris)] = tris
            self._pad_tris[list_index] = pad

    @staticmethod
    def _trim(tri):
        """Drop the padded columns no cell in this query actually fills.

        The packed table is as wide as the busiest cell in the whole level, but
        a single query only touches some cells; sizing the vectorised pass to the
        widest cell it *did* touch keeps a sparse or clustered crowd from paying
        for the busiest cell's column count. Columns fill left to right and pad
        with -1, so the used width is the largest per-row valid count.
        """
        if tri.shape[0] == 0:
            return tri
        width = int((tri >= 0).sum(axis=1).max())
        return tri[:, :max(width, 1)]

    # -- access -------------------------------------------------------------

    def surface(self, index):
        """Wrap triangle `index` in a Surface object, memoised."""
        surf = self._surface_cache.get(index)
        if surf is None:
            surf = Surface(
                index,
                int(self.type[index]),
                int(self.force[index]),
                int(self.flags[index]),
                tuple(self.normal[index]),
                float(self.origin_offset[index]),
                (
                    tuple(int(v) for v in self.v1[index]),
                    tuple(int(v) for v in self.v2[index]),
                    tuple(int(v) for v in self.v3[index]),
                ),
                int(self.lower_y[index]),
                int(self.upper_y[index]),
            )
            self._surface_cache[index] = surf
        return surf

    def _cell(self, list_index, x, z):
        cell_x = ((x + LEVEL_BOUNDARY_MAX) // CELL_SIZE) & 0xF
        cell_z = ((z + LEVEL_BOUNDARY_MAX) // CELL_SIZE) & 0xF
        return self.cells[list_index][cell_z][cell_x]

    def _lateral_mask(self, tris, x, z):
        """Winding test for containment of (x, z); positive means inside."""
        x1, z1 = self.v1[tris, 0], self.v1[tris, 2]
        x2, z2 = self.v2[tris, 0], self.v2[tris, 2]
        x3, z3 = self.v3[tris, 0], self.v3[tris, 2]

        e1 = (z1 - z) * (x2 - x1) - (x1 - x) * (z2 - z1)
        e2 = (z2 - z) * (x3 - x2) - (x2 - x) * (z3 - z2)
        e3 = (z3 - z) * (x1 - x3) - (x3 - x) * (z1 - z3)
        return e1, e2, e3

    def _height_on(self, tris, x, z):
        nx = self.normal[tris, 0]
        ny = self.normal[tris, 1]
        nz = self.normal[tris, 2]
        oo = self.origin_offset[tris]
        with np.errstate(divide="ignore", invalid="ignore"):
            return -(x * nx + nz * z + oo) / ny, ny

    # -- queries ------------------------------------------------------------

    def find_floor(self, x_pos, y_pos, z_pos, for_camera=False):
        """Highest floor at or below the point. Returns (height, Surface|None)."""
        x, y, z = _to_s16(x_pos), _to_s16(y_pos), _to_s16(z_pos)

        if x <= -LEVEL_BOUNDARY_MAX or x >= LEVEL_BOUNDARY_MAX:
            return HEIGHT_NONE, None
        if z <= -LEVEL_BOUNDARY_MAX or z >= LEVEL_BOUNDARY_MAX:
            return HEIGHT_NONE, None

        tris = self._cell(FLOOR_LIST, x, z)
        height, surf = self._find_floor_in(tris, x, y, z, for_camera)

        # An intangible floor hides whatever is under it; look again from below.
        if surf is not None and surf.type == SURFACE_INTANGIBLE:
            height, surf = self._find_floor_in(
                tris, x, int(height - 200.0), z, for_camera
            )

        return height, surf

    def _find_floor_in(self, tris, x, y, z, for_camera):
        if len(tris) == 0:
            return HEIGHT_NONE, None

        e1, e2, e3 = self._lateral_mask(tris, x, z)
        ok = (e1 >= 0) & (e2 >= 0) & (e3 >= 0)
        if not ok.any():
            return HEIGHT_NONE, None

        height, ny = self._height_on(tris, x, z)
        ok &= ny != 0.0
        # A 78-unit buffer lets Mario stand slightly below a floor.
        ok &= (y - (height - 78.0)) >= 0.0

        if for_camera:
            ok &= (self.flags[tris] & SURFACE_FLAG_NO_CAM_COLLISION) == 0
        else:
            ok &= self.type[tris] != SURFACE_CAMERA_BOUNDARY

        if not ok.any():
            return HEIGHT_NONE, None

        first = int(np.argmax(ok))
        return float(height[first]), self.surface(int(tris[first]))

    def find_ceil(self, x_pos, y_pos, z_pos, for_camera=False):
        """Lowest ceiling at or above the point. Returns (height, Surface|None)."""
        x, y, z = _to_s16(x_pos), _to_s16(y_pos), _to_s16(z_pos)
        tris = self._cell(CEIL_LIST, x, z)
        if len(tris) == 0:
            return HEIGHT_NONE * -1.0, None

        e1, e2, e3 = self._lateral_mask(tris, x, z)
        ok = (e1 <= 0) & (e2 <= 0) & (e3 <= 0)
        if not ok.any():
            return 20000.0, None

        height, ny = self._height_on(tris, x, z)
        ok &= ny != 0.0
        ok &= (y - (height + 78.0)) <= 0.0

        if for_camera:
            ok &= (self.flags[tris] & SURFACE_FLAG_NO_CAM_COLLISION) == 0
        else:
            ok &= self.type[tris] != SURFACE_CAMERA_BOUNDARY

        if not ok.any():
            return 20000.0, None

        first = int(np.argmax(ok))
        return float(height[first]), self.surface(int(tris[first]))

    def find_wall_collisions(self, data, for_camera=False):
        """Push `data` out of every wall it overlaps. Returns the hit count."""
        x, z = _to_s16(data.x), _to_s16(data.z)
        if abs(x) >= LEVEL_BOUNDARY_MAX or abs(z) >= LEVEL_BOUNDARY_MAX:
            return 0

        tris = self._cell(WALL_LIST, x, z)
        if len(tris) == 0:
            return 0

        radius = min(data.radius, 200.0)
        # Tests all read the entry position; only the output is accumulated,
        # so overlapping walls each push by their full amount.
        px, py, pz = data.x, data.y + data.offset_y, data.z

        ok = (py >= self.lower_y[tris]) & (py <= self.upper_y[tris])
        if not ok.any():
            return 0

        nx = self.normal[tris, 0]
        ny = self.normal[tris, 1]
        nz = self.normal[tris, 2]
        offset = nx * px + ny * py + nz * pz + self.origin_offset[tris]
        ok &= (offset >= -radius) & (offset <= radius)
        if not ok.any():
            return 0

        if for_camera:
            ok &= (self.flags[tris] & SURFACE_FLAG_NO_CAM_COLLISION) == 0
        else:
            ok &= self.type[tris] != SURFACE_CAMERA_BOUNDARY

        ok &= self._wall_face_mask(tris, px, py, pz)
        if not ok.any():
            return 0

        hits = np.flatnonzero(ok)
        for k in hits:
            index = int(tris[k])
            push = radius - offset[k]
            data.x += self.normal[index, 0] * push
            data.z += self.normal[index, 2] * push
            if len(data.walls) < 4:
                data.walls.append(self.surface(index))

        return int(len(hits))

    # -- batched queries ----------------------------------------------------
    #
    # The scalar queries above answer one point at a time, and their cost is
    # almost all numpy call overhead -- the triangle maths is cheap, but paying
    # it per entity means thousands of tiny numpy calls a tick. These answer a
    # whole array of points at once. The trick is that every point in the same
    # 1024-unit cell tests the same ordered triangle list, so the points are
    # grouped by cell (there are at most 16x16 of them) and each group is one
    # vectorised pass over points-by-triangles. The overhead is paid per cell,
    # not per point, which is the whole difference between a crowd that runs and
    # one that does not. The results are identical to the scalar queries -- see
    # tools/check_batch_surfaces.py, which holds them to that.

    # Above this many occupied cells in one query, a single packed pass over all
    # points beats looping per cell: the per-cell numpy overhead (paid once per
    # occupied cell) has overtaken the padding waste of the packed pass. Below
    # it -- a clustered crowd in a handful of cells -- the per-cell loop wins,
    # because it never pads a sparse cell's points up to a dense cell's width.
    # The 16x16 grid tops out at 256 occupied cells; the crossover sits well
    # inside that. Purely a speed knob -- both paths return identical results.
    _CELL_DISPATCH = 64

    @staticmethod
    def _s16(a):
        """Truncate an array toward zero into s16, as the C cast does."""
        return a.astype(np.int64).astype(np.int16).astype(np.int64)

    def _cell_keys(self, xi, zi):
        """A single cell index per point, and which points are in bounds."""
        inb = ((xi > -LEVEL_BOUNDARY_MAX) & (xi < LEVEL_BOUNDARY_MAX)
               & (zi > -LEVEL_BOUNDARY_MAX) & (zi < LEVEL_BOUNDARY_MAX))
        cell_x = ((xi + LEVEL_BOUNDARY_MAX) // CELL_SIZE) & 0xF
        cell_z = ((zi + LEVEL_BOUNDARY_MAX) // CELL_SIZE) & 0xF
        return np.where(inb, cell_z * CELL_COUNT + cell_x, -1)

    def find_floors(self, xs, ys, zs, for_camera=False):
        """Highest floor at or below each point. Returns (heights, ids).

        `ids` is the triangle index of the floor found, or -1 where there is
        none -- the array counterpart of the (height, Surface) the scalar query
        hands back, with the surface left as an index the caller can look up.
        """
        xs = np.asarray(xs, dtype=np.float64)
        ys = np.asarray(ys, dtype=np.float64)
        zs = np.asarray(zs, dtype=np.float64)
        n = len(xs)
        xi, yi, zi = self._s16(xs), self._s16(ys), self._s16(zs)

        keys = self._cell_keys(xi, zi)
        valid = keys >= 0
        occ = np.unique(keys[valid]) if n else np.empty(0, np.int64)
        if len(occ) <= self._CELL_DISPATCH:
            # Few cells touched -- a clustered crowd. Resolve each cell against
            # its own triangle list, so a dense cell's column count is not padded
            # onto the points sitting in sparse ones.
            heights = np.full(n, HEIGHT_NONE, dtype=np.float64)
            ids = np.full(n, -1, dtype=np.int64)
            for key in occ:
                cz, cx = divmod(int(key), CELL_COUNT)
                tris = self.cells[FLOOR_LIST][cz][cx]
                if len(tris) == 0:
                    continue
                pts = np.flatnonzero(keys == key)
                tri = np.broadcast_to(tris, (len(pts), len(tris)))
                cm = np.ones(tri.shape, dtype=bool)
                h, tid = self._resolve_floor(
                    tri, cm, xi[pts], yi[pts], zi[pts], for_camera)
                heights[pts] = h
                ids[pts] = tid
        else:
            # A crowd spread across the level -- one packed pass over all points
            # beats paying numpy's per-call overhead once per occupied cell.
            tri = self._trim(self._pad_tris[FLOOR_LIST][np.where(valid, keys, 0)])
            colmask = (tri >= 0) & valid[:, None]
            heights, ids = self._resolve_floor(
                tri, colmask, xi, yi, zi, for_camera)

        # An intangible floor hides what is under it; look again from below.
        # Rare enough -- often absent from a level entirely -- that the handful
        # of points that hit one are re-queried with the exact scalar path
        # rather than a second vectorised pass.
        if len(ids):
            hit = ids >= 0
            if hit.any():
                intangible = hit & (self.type[np.where(hit, ids, 0)]
                                    == SURFACE_INTANGIBLE)
                for i in np.flatnonzero(intangible):
                    tris = self._cell(FLOOR_LIST, int(xi[i]), int(zi[i]))
                    h2, surf = self._find_floor_in(
                        tris, int(xi[i]), int(heights[i] - 200.0),
                        int(zi[i]), for_camera)
                    heights[i] = h2
                    ids[i] = surf.index if surf is not None else -1
        return heights, ids

    def _resolve_floor(self, tri, colmask, x, y, z, for_camera):
        """Pick each point's first passing floor from its packed cell list.

        `tri` is (n, m): a row per point holding its cell's ordered triangle
        indices, padded past the cell's own length; `colmask` marks the real
        columns. Every array below is (n, m) -- a point by its candidate floors,
        in priority order -- so the first passing column is the same "first, not
        best" floor the scalar query walks the cell list for. The padded columns
        are masked out of `ok`, so they never win the argmax.
        """
        ts = np.where(colmask, tri, 0)
        x1, z1 = self.v1[ts, 0], self.v1[ts, 2]
        x2, z2 = self.v2[ts, 0], self.v2[ts, 2]
        x3, z3 = self.v3[ts, 0], self.v3[ts, 2]
        xk, zk = x[:, None], z[:, None]

        e1 = (z1 - zk) * (x2 - x1) - (x1 - xk) * (z2 - z1)
        e2 = (z2 - zk) * (x3 - x2) - (x2 - xk) * (z3 - z2)
        e3 = (z3 - zk) * (x1 - x3) - (x3 - xk) * (z1 - z3)
        ok = (e1 >= 0) & (e2 >= 0) & (e3 >= 0) & colmask

        nx, ny, nz = self.normal[ts, 0], self.normal[ts, 1], self.normal[ts, 2]
        oo = self.origin_offset[ts]
        with np.errstate(divide="ignore", invalid="ignore"):
            height = -(xk * nx + nz * zk + oo) / ny
        ok &= ny != 0.0
        ok &= (y[:, None] - (height - 78.0)) >= 0.0
        if for_camera:
            ok &= (self.flags[ts] & SURFACE_FLAG_NO_CAM_COLLISION) == 0
        else:
            ok &= self.type[ts] != SURFACE_CAMERA_BOUNDARY

        has = ok.any(axis=1)
        first = ok.argmax(axis=1)
        rows = np.arange(len(x))
        chosen_h = height[rows, first]
        chosen_t = tri[rows, first]
        return (np.where(has, chosen_h, HEIGHT_NONE),
                np.where(has, chosen_t, -1))

    def find_walls(self, xs, ys, zs, offset_ys, radii, for_camera=False):
        """Push every point out of the walls it overlaps.

        Returns (xs_out, zs_out, counts). Each point is pushed by the sum of its
        walls the way the scalar query accumulates them -- the tests read the
        entry position, so the pushes are independent and their order does not
        matter. Only the cell lookup truncates to s16; the geometry reads the
        raw position, matching find_wall_collisions.
        """
        xs = np.asarray(xs, dtype=np.float64)
        ys = np.asarray(ys, dtype=np.float64)
        zs = np.asarray(zs, dtype=np.float64)
        offset_ys = np.asarray(offset_ys, dtype=np.float64)
        radii = np.asarray(radii, dtype=np.float64)
        n = len(xs)

        xi, zi = self._s16(xs), self._s16(zs)
        radius = np.minimum(radii, 200.0)
        py = ys + offset_ys

        keys = self._cell_keys(xi, zi)
        valid = keys >= 0
        occ = np.unique(keys[valid]) if n else np.empty(0, np.int64)
        if len(occ) <= self._CELL_DISPATCH:
            out_x, out_z = xs.copy(), zs.copy()
            counts = np.zeros(n, dtype=np.int64)
            for key in occ:
                cz, cx = divmod(int(key), CELL_COUNT)
                tris = self.cells[WALL_LIST][cz][cx]
                if len(tris) == 0:
                    continue
                pts = np.flatnonzero(keys == key)
                tri = np.broadcast_to(tris, (len(pts), len(tris)))
                cm = np.ones(tri.shape, dtype=bool)
                dx, dz, cnt = self._resolve_walls(
                    tri, cm, xs[pts], py[pts], zs[pts], radius[pts], for_camera)
                out_x[pts] = xs[pts] + dx
                out_z[pts] = zs[pts] + dz
                counts[pts] = cnt
            return out_x, out_z, counts

        tri = self._trim(self._pad_tris[WALL_LIST][np.where(valid, keys, 0)])
        colmask = (tri >= 0) & valid[:, None]
        dx, dz, counts = self._resolve_walls(
            tri, colmask, xs, py, zs, radius, for_camera)
        return xs + dx, zs + dz, counts

    def _resolve_walls(self, tri, colmask, px, py, pz, radius, for_camera):
        """Total push and hit count for each point against its packed walls.

        `tri`/`colmask` pack each point's cell wall list the way `_resolve_floor`
        packs the floors. The push is summed across a point's walls -- the tests
        all read its entry position, so the pushes are independent of order --
        and padded columns are masked out of both the sum and the count.
        """
        ts = np.where(colmask, tri, 0)
        lower, upper = self.lower_y[ts], self.upper_y[ts]
        pyk = py[:, None]
        ok = (pyk >= lower) & (pyk <= upper) & colmask

        nx, ny, nz = self.normal[ts, 0], self.normal[ts, 1], self.normal[ts, 2]
        oo = self.origin_offset[ts]
        offset = nx * px[:, None] + ny * pyk + nz * pz[:, None] + oo
        rad = radius[:, None]
        ok &= (offset >= -rad) & (offset <= rad)
        if for_camera:
            ok &= (self.flags[ts] & SURFACE_FLAG_NO_CAM_COLLISION) == 0
        else:
            ok &= self.type[ts] != SURFACE_CAMERA_BOUNDARY
        ok &= self._wall_face_mask_batch(ts, px, py, pz)

        push = rad - offset
        dx = np.where(ok, nx * push, 0.0).sum(axis=1)
        dz = np.where(ok, nz * push, 0.0).sum(axis=1)
        return dx, dz, ok.sum(axis=1)

    def _wall_face_mask_batch(self, ts, x, y, z):
        """_wall_face_mask over (n, m) packed walls at once, giving (n, m).

        `ts` is the point-by-wall index table; `x`/`y`/`z` are per point. Padded
        columns produce a value the caller has already masked to false, so their
        containment result is never read.
        """
        y1, y2, y3 = self.v1[ts, 1], self.v2[ts, 1], self.v3[ts, 1]
        x_proj = (self.flags[ts] & SURFACE_FLAG_X_PROJECTION) != 0

        w1 = np.where(x_proj, -self.v1[ts, 2], self.v1[ts, 0])
        w2 = np.where(x_proj, -self.v2[ts, 2], self.v2[ts, 0])
        w3 = np.where(x_proj, -self.v3[ts, 2], self.v3[ts, 0])
        point = np.where(x_proj, (-z)[:, None], x[:, None])
        yk = y[:, None]

        e1 = (y1 - yk) * (w2 - w1) - (w1 - point) * (y2 - y1)
        e2 = (y2 - yk) * (w3 - w2) - (w2 - point) * (y3 - y2)
        e3 = (y3 - yk) * (w1 - w3) - (w3 - point) * (y1 - y3)

        positive = np.where(x_proj, self.normal[ts, 0] > 0.0,
                            self.normal[ts, 2] > 0.0)
        inside_neg = (e1 <= 0) & (e2 <= 0) & (e3 <= 0)
        inside_pos = (e1 >= 0) & (e2 >= 0) & (e3 >= 0)
        return np.where(positive, inside_neg, inside_pos)

    # The batch size below which threading the crowd resolve is not worth the
    # dispatch: a small crowd is faster resolved on the calling thread.
    _THREAD_MIN = 512

    def resolve_crowd(self, xs, ys, zs, offs, rad, floor_lift=50.0):
        """One tick's whole crowd: push out of walls, then land on floors.

        Bundles the wall pass and the floor pass a crowd step needs -- the floor
        is looked for from the wall-corrected position -- so the pair can be
        split across worker threads as one unit. Returns
        (out_x, out_z, wall_counts, floor_heights, floor_ids). Each point is
        independent, so a large crowd is sliced across the pool and the slices'
        results are stitched back in order; the answer does not depend on the
        slicing, and a small crowd is done in-thread.
        """
        xs = np.asarray(xs, dtype=np.float64)
        ys = np.asarray(ys, dtype=np.float64)
        zs = np.asarray(zs, dtype=np.float64)
        offs = np.asarray(offs, dtype=np.float64)
        rad = np.asarray(rad, dtype=np.float64)
        n = len(xs)

        if not self.threading_enabled or n < self._THREAD_MIN:
            return self._resolve_crowd_chunk(xs, ys, zs, offs, rad, floor_lift)
        pool = self._ensure_pool()
        if pool is None:
            return self._resolve_crowd_chunk(xs, ys, zs, offs, rad, floor_lift)

        k = self._workers
        bounds = [(i * n // k, (i + 1) * n // k) for i in range(k)]
        futures = [pool.submit(self._resolve_crowd_chunk,
                               xs[a:b], ys[a:b], zs[a:b],
                               offs[a:b], rad[a:b], floor_lift)
                   for a, b in bounds]

        out_x = np.empty(n, dtype=np.float64)
        out_z = np.empty(n, dtype=np.float64)
        counts = np.empty(n, dtype=np.int64)
        heights = np.empty(n, dtype=np.float64)
        ids = np.empty(n, dtype=np.int64)
        for (a, b), fut in zip(bounds, futures):
            ox, oz, cnt, h, tid = fut.result()
            out_x[a:b], out_z[a:b], counts[a:b] = ox, oz, cnt
            heights[a:b], ids[a:b] = h, tid
        return out_x, out_z, counts, heights, ids

    def _resolve_crowd_chunk(self, xs, ys, zs, offs, rad, floor_lift):
        out_x, out_z, counts = self.find_walls(xs, ys, zs, offs, rad)
        heights, ids = self.find_floors(out_x, ys + floor_lift, out_z)
        return out_x, out_z, counts, heights, ids

    def _wall_face_mask(self, tris, x, y, z):
        """Containment test in the wall's dominant projection plane."""
        y1, y2, y3 = self.v1[tris, 1], self.v2[tris, 1], self.v3[tris, 1]
        x_proj = (self.flags[tris] & SURFACE_FLAG_X_PROJECTION) != 0

        # X-projected walls are tested against -Z; the rest against X.
        w1 = np.where(x_proj, -self.v1[tris, 2], self.v1[tris, 0])
        w2 = np.where(x_proj, -self.v2[tris, 2], self.v2[tris, 0])
        w3 = np.where(x_proj, -self.v3[tris, 2], self.v3[tris, 0])
        point = np.where(x_proj, -z, x)

        e1 = (y1 - y) * (w2 - w1) - (w1 - point) * (y2 - y1)
        e2 = (y2 - y) * (w3 - w2) - (w2 - point) * (y3 - y2)
        e3 = (y3 - y) * (w1 - w3) - (w3 - point) * (y1 - y3)

        # Sign of the facing axis decides which winding counts as inside.
        positive = np.where(x_proj, self.normal[tris, 0] > 0.0,
                            self.normal[tris, 2] > 0.0)
        inside_neg = (e1 <= 0) & (e2 <= 0) & (e3 <= 0)
        inside_pos = (e1 >= 0) & (e2 >= 0) & (e3 >= 0)
        return np.where(positive, inside_neg, inside_pos)


def load(npz_path):
    """Load a SurfaceSet from a file produced by tools/parse_collision.py."""
    data = np.load(npz_path)
    return SurfaceSet(
        data["vertices"], data["tri_verts"], data["tri_type"], data["tri_force"],
        data["water_boxes"] if "water_boxes" in data else None,
    )
