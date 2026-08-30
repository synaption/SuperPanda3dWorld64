"""Slope classification and material assignment.

src/level.rs:34 sets GROUND_NORMAL_Y = 0.7: a triangle is a floor you stand on
if its normal leans less than that off vertical, and a wall you get pushed out
of otherwise -- about 45.6 degrees.

On a sphere "vertical" is the radial direction, not +Y. Everything here uses
dot(normal, radial). The runtime still tests normal.y, which is correct for a
flat level and wrong for a planet; see the readme's "Bevy, later".
"""

import numpy as np

OCEAN, SAND, GRASS, ROCK, SNOW = range(5)

MATERIALS = (
    {"name": "ocean_floor", "surface": "SURFACE_DEFAULT", "color": (0.16, 0.20, 0.26),
     "texture": "ocean.png", "texture_scale": 14.0},
    {"name": "sand",        "surface": "SURFACE_SAND",    "color": (0.76, 0.70, 0.48),
     "texture": "sand.png", "texture_scale": 10.0},
    {"name": "grass",       "surface": "SURFACE_DEFAULT", "color": (0.24, 0.44, 0.16),
     "texture": "grass.png", "texture_scale": 12.0},
    {"name": "rock",        "surface": "SURFACE_HARD",    "color": (0.38, 0.36, 0.34),
     "texture": "rock.png", "texture_scale": 10.0},
    {"name": "snow",        "surface": "SURFACE_SNOW",    "color": (0.90, 0.92, 0.95),
     "texture": "snow.png", "texture_scale": 12.0},
)


def radial_slope(normals, directions):
    """cos(angle) between each vertex normal and its own up."""
    return np.einsum("ij,ij->i", normals, directions)


def classify(altitude, slope_cos, sea_level, snow_line, steep):
    """Material index per vertex, from height above sea and steepness."""
    material = np.full(altitude.shape, GRASS, dtype=np.uint8)
    material[altitude > snow_line] = SNOW
    material[slope_cos < steep] = ROCK
    material[np.abs(altitude - sea_level) < 1.5] = SAND
    material[altitude < sea_level - 1.5] = OCEAN
    return material


def walkable_triangles(positions, triangles, ground_normal):
    """Per-triangle: is this a floor, by the game's own floor/wall split?"""
    return triangle_slope(positions, triangles) > ground_normal


def triangle_slope(positions, triangles):
    """Cosine of each triangle's angle from its local radial up.

    Keeping this as the common primitive matters now that walking, farming and
    actor navigation use different slope limits.  They must disagree only on
    the limit, not because each one measured the same triangle differently.
    """
    a, b, c = (positions[triangles[:, i]] for i in range(3))
    n = np.cross(b - a, c - a)
    length = np.linalg.norm(n, axis=1, keepdims=True)
    n = n / np.maximum(length, 1e-12)
    centre = (a + b + c) / 3.0
    up = centre / np.maximum(np.linalg.norm(centre, axis=1, keepdims=True), 1e-12)
    return np.einsum("ij,ij->i", n, up)


def traversal_classes(positions, triangles, altitude, sea_level,
                      ground_normal, farm_slope_cos, farm_min_altitude,
                      farm_max_altitude):
    """Actor and farming semantics for every terrain triangle.

    Water is a movement cost, not missing topology.  Luna can walk on every
    ordinary floor including the seabed.  Mario uses dry floors and swims once
    he crosses the shore.  Ants prefer the same dry graph but retain the full
    graph as a safe fallback, so falling into water is inconvenient rather
    than lethal.  Farming is deliberately stricter: dry lowland whose entire
    triangle clears the beach and whose slope is gentle enough for fields.
    """
    slope = triangle_slope(positions, triangles)
    tri_altitude = altitude[triangles]
    mean_altitude = tri_altitude.mean(axis=1)
    water = mean_altitude < sea_level
    walkable = slope > ground_normal
    mario_walkable = walkable & ~water
    luna_walkable = walkable
    farmable = (
        walkable
        & (slope > farm_slope_cos)
        & np.all(tri_altitude >= farm_min_altitude, axis=1)
        & np.all(tri_altitude <= farm_max_altitude, axis=1)
    )
    return {
        "walkable": walkable,
        "water": water,
        "mario_walkable": mario_walkable,
        "mario_accessible": mario_walkable.copy(),
        "luna_walkable": luna_walkable,
        "farmable": farmable,
        "ant_preferred": mario_walkable.copy(),
        "ant_allowed": walkable.copy(),
    }


def enforce_topology(positions, triangles, classes, farm_min_area):
    """Turn geometric candidates into reachable, useful gameplay regions.

    A dry component is Mario-accessible when at least one of its edges meets
    water; Mario can then swim between any two such shores.  A farm must lie
    in one of those components and be part of a contiguous patch large enough
    to use, rather than a technically flat triangle on a cliff ledge.

    High and isolated walkable terrain is intentionally retained in
    ``mario_walkable``. It is scenery and optional exploration, not a broken
    promise made to spawning or farming systems.
    """
    edge_members = {}
    for index, vertices in enumerate(triangles):
        for a, b in ((vertices[0], vertices[1]), (vertices[1], vertices[2]),
                     (vertices[2], vertices[0])):
            edge_members.setdefault((min(a, b), max(a, b)), []).append(index)

    def components(mask):
        parent = np.arange(len(triangles), dtype=np.int64)

        def find(index):
            while parent[index] != index:
                parent[index] = parent[parent[index]]
                index = parent[index]
            return index

        for members in edge_members.values():
            first = next((i for i in members if mask[i]), None)
            if first is None:
                continue
            for other in members:
                if not mask[other]:
                    continue
                a, b = find(first), find(other)
                if a != b:
                    parent[a] = b
        roots = np.full(len(triangles), -1, dtype=np.int64)
        for index in np.flatnonzero(mask):
            roots[index] = find(index)
        return roots

    mario = classes["mario_walkable"]
    water = classes["water"]
    mario_roots = components(mario)
    shore_roots = set()
    for members in edge_members.values():
        if any(water[i] for i in members):
            shore_roots.update(mario_roots[i] for i in members if mario[i])
    accessible = mario & np.isin(mario_roots, list(shore_roots))

    candidates = classes["farmable"] & accessible
    farm_roots = components(candidates)
    p = positions[triangles]
    area = 0.5 * np.linalg.norm(
        np.cross(p[:, 1] - p[:, 0], p[:, 2] - p[:, 0]), axis=1)
    region_area = {}
    for index in np.flatnonzero(candidates):
        root = int(farm_roots[index])
        region_area[root] = region_area.get(root, 0.0) + float(area[index])
    useful_roots = {root for root, value in region_area.items()
                    if value >= farm_min_area}

    out = {key: value.copy() for key, value in classes.items()}
    out["mario_accessible"] = accessible
    out["farmable"] = candidates & np.isin(farm_roots, list(useful_roots))
    return out
