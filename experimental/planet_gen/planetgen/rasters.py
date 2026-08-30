"""Authored input rasters: six face maps per layer, and how to seed them.

Elevation is 16-bit. Eight bits gives 256 height steps, which across a useful
altitude range terraces visibly on any gentle slope -- and gentle slopes are
exactly the ground the player walks on. Index layers are 8-bit and must be
sampled nearest-neighbour: interpolating between material 3 and material 7
yields material 5, which is a different material, and the artifact is a fringe
of wrong terrain along every boundary.
"""

import numpy as np
from PIL import Image

ELEVATION = "elevation"
MATERIAL = "material"
LOCK = "lock"


def face_path(root, layer, face):
    return root / "faces" / f"{layer}_{face}.png"


def save_elevation(path, data01):
    """Write a float field in [0, 1] as a 16-bit grayscale PNG."""
    q = np.clip(np.rint(np.asarray(data01) * 65535.0), 0, 65535).astype(np.uint16)
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(q, mode="I;16").save(path)


def load_elevation(path):
    im = Image.open(path)
    if im.mode not in ("I;16", "I;16B", "I", "L"):
        raise ValueError(f"{path.name}: expected 16-bit grayscale, got {im.mode}")
    if im.mode == "L":
        raise ValueError(f"{path.name}: 8-bit elevation terraces; author 16-bit")
    return np.asarray(im).astype(np.float64) / 65535.0


def save_index(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    Image.fromarray(np.asarray(data).astype(np.uint8), mode="L").save(path)


def load_index(path):
    return np.asarray(Image.open(path).convert("L"))


def sample_bilinear(raster, t_u, t_v):
    """Sample a face raster at face coordinates in [-1, 1].

    The raster is indexed by the *unwarped* face parameter, which is the one
    that is uniform in angle once cubesphere.warp is applied -- so a texel is
    worth the same number of metres everywhere and painting resolution is even
    across the face.
    """
    h, w = raster.shape
    x = np.clip((np.asarray(t_u) + 1.0) * 0.5 * (w - 1), 0, w - 1)
    y = np.clip((np.asarray(t_v) + 1.0) * 0.5 * (h - 1), 0, h - 1)
    x0, y0 = np.floor(x).astype(int), np.floor(y).astype(int)
    x1, y1 = np.minimum(x0 + 1, w - 1), np.minimum(y0 + 1, h - 1)
    fx, fy = x - x0, y - y0
    top = raster[y0, x0] * (1 - fx) + raster[y0, x1] * fx
    bot = raster[y1, x0] * (1 - fx) + raster[y1, x1] * fx
    return top * (1 - fy) + bot * fy


def sample_nearest(raster, t_u, t_v):
    h, w = raster.shape
    x = np.clip(np.rint((np.asarray(t_u) + 1.0) * 0.5 * (w - 1)), 0, w - 1).astype(int)
    y = np.clip(np.rint((np.asarray(t_v) + 1.0) * 0.5 * (h - 1)), 0, h - 1).astype(int)
    return raster[y, x]


# ---------------------------------------------------------------------------
# Seeding. Noise is used to create a starting canvas, not as the generator:
# once written these are ordinary PNGs to paint over, and the build reads the
# pixels rather than re-running any of this.

_OFFSET = np.int64(4096)


def _hash01(ix, iy, iz, seed):
    x = (ix + _OFFSET).astype(np.uint64)
    y = (iy + _OFFSET).astype(np.uint64)
    z = (iz + _OFFSET).astype(np.uint64)
    h = (x * np.uint64(73856093)) ^ (y * np.uint64(19349663)) ^ (z * np.uint64(83492791))
    h ^= np.uint64(seed) * np.uint64(2654435761)
    h ^= h >> np.uint64(13)
    h *= np.uint64(1274126177)
    h ^= h >> np.uint64(16)
    return (h & np.uint64(0xFFFFFF)).astype(np.float64) / float(0xFFFFFF)


def value_noise(points, seed):
    """Trilinear value noise on 3D points, quintic-smoothed."""
    i = np.floor(points).astype(np.int64)
    f = points - i
    w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0)
    ix, iy, iz = i[..., 0], i[..., 1], i[..., 2]
    wx, wy, wz = w[..., 0], w[..., 1], w[..., 2]
    c = {}
    for dx in (0, 1):
        for dy in (0, 1):
            for dz in (0, 1):
                c[(dx, dy, dz)] = _hash01(ix + dx, iy + dy, iz + dz, seed)
    x00 = c[(0, 0, 0)] * (1 - wx) + c[(1, 0, 0)] * wx
    x10 = c[(0, 1, 0)] * (1 - wx) + c[(1, 1, 0)] * wx
    x01 = c[(0, 0, 1)] * (1 - wx) + c[(1, 0, 1)] * wx
    x11 = c[(0, 1, 1)] * (1 - wx) + c[(1, 1, 1)] * wx
    y0 = x00 * (1 - wy) + x10 * wy
    y1 = x01 * (1 - wy) + x11 * wy
    return y0 * (1 - wz) + y1 * wz


def fbm(directions, seed, frequency=1.0, octaves=5, lacunarity=2.0, gain=0.5):
    total = np.zeros(directions.shape[:-1], dtype=np.float64)
    amplitude, norm, freq = 1.0, 0.0, frequency
    for octave in range(octaves):
        total += amplitude * value_noise(directions * freq, seed + octave * 101)
        norm += amplitude
        amplitude *= gain
        freq *= lacunarity
    return total / norm


def _smoothstep01(value):
    value = np.clip(value, 0.0, 1.0)
    return value * value * (3.0 - 2.0 * value)


def _terrace(value, steps, flatness):
    """Broad level benches separated by deliberately steep transitions."""
    value = np.clip(value, 0.0, 1.0)
    width = max((1.0 - flatness) / steps, 1e-3)
    result = np.full_like(value, 0.5 / steps)
    for boundary in range(1, steps):
        centre = boundary / steps
        result += _smoothstep01((value - centre + width * 0.5) / width) / steps
    # The lowest bench must meet the water rather than form a vertical rim.
    # Its approach is broad enough to be a beach and narrow enough that most
    # of the bench remains useful level ground.
    shore_width = 0.7 / steps
    return result * _smoothstep01(value / shore_width)


def seed_elevation(face_directions_list, seed, relief=1.0,
                   detail=0.22, detail_frequency=4.5, octaves=5,
                   terrace_steps=4, terrace_flatness=0.72,
                   route_width=0.08, seabed_relief=0.16):
    """A playable starting planet: farm benches, routed cliffs and seabeds.

    Takes all six faces at once and returns all six, because the normalization
    has to be shared. Range-normalizing each face against its own min and max
    would give every face a different mapping from noise to height, and the
    seams -- correct up to that point -- would part company by metres.

    Sampling 3D noise at the sphere direction rather than in face space is what
    makes the six maps agree across the 12 face seams with no stitching pass:
    the shared edge is literally the same point, so it gets the same value.
    """
    fields = []
    roughness = []
    routes = []
    for directions in face_directions_list:
        # A low-frequency domain warp keeps coastlines from reading as noise.
        warp_vec = np.stack([fbm(directions + 11.0 * axis, seed + 700 + axis, 1.1, 3)
                             for axis in range(3)], axis=-1)
        warped = directions + 0.35 * (warp_vec - 0.5)
        continents = fbm(warped, seed, frequency=1.15, octaves=5)
        rough = fbm(warped, seed + 31, frequency=detail_frequency, octaves=octaves)
        fields.append(continents + detail * (rough - 0.5))
        roughness.append(rough)
        # Long zero contours cross the height contours from more than one
        # direction.  Where they do, the smooth source terrain is kept instead
        # of terraced, cutting broad ramps through otherwise steep shoulders.
        route_field = fbm(directions, seed + 1901, frequency=1.45, octaves=3)
        routes.append(np.abs(route_field - 0.5))

    low = min(float(f.min()) for f in fields)
    high = max(float(f.max()) for f in fields)
    span = max(high - low, 1e-9)

    out = []
    for field, rough, route_distance in zip(fields, roughness, routes):
        f = (field - low) / span
        # Push the histogram away from the sea-level threshold so coasts are
        # decisive rather than a fringe of half-submerged mush, then flatten
        # the deep ocean floor, which nothing walks on and which otherwise eats
        # most of the 16-bit range.
        f = np.clip((f - 0.5) * 1.35 + 0.5, 0.0, 1.0)
        signed = (f - 0.5) * 2.0

        # Below sea level, keep the whole bottom gently varying.  A cliff on
        # land is an obstacle; a cliff under water would make Luna's promised
        # walking route disappear while Mario simply swims over it.
        depth = _smoothstep01(np.maximum(-signed, 0.0))
        seabed = -seabed_relief * depth

        # Above sea level, most of each broad altitude band is genuinely flat,
        # with steep hills between.  Ribbons sampled independently of height
        # soften selected shoulders back to the original broad slope, so every
        # useful bench has approaches rather than being a mesa sealed on all
        # sides. Fine detail fades out of the low farm belt and returns toward
        # the high, optional terrain.
        land = np.maximum(signed, 0.0)
        terraced = _terrace(land, terrace_steps, terrace_flatness)
        ramp = 1.0 - _smoothstep01(route_distance / max(route_width, 1e-3))
        shaped_land = terraced * (1.0 - ramp) + land * ramp
        highland_detail = np.clip((land - 0.20) / 0.55, 0.0, 1.0)
        shaped_land += (rough - 0.5) * detail * 0.18 * highland_detail

        shaped = np.where(signed < 0.0, seabed, shaped_land)
        # Relief scales about sea level so slope can be tuned without
        # repainting the shape of the continents or moving the shoreline.
        out.append(np.clip(0.5 + 0.5 * shaped * relief, 0.0, 1.0))
    return out
