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
        self._surface_cache = {}

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
