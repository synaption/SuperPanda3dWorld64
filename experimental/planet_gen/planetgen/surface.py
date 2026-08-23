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
    a, b, c = (positions[triangles[:, i]] for i in range(3))
    n = np.cross(b - a, c - a)
    length = np.linalg.norm(n, axis=1, keepdims=True)
    n = n / np.maximum(length, 1e-12)
    centre = (a + b + c) / 3.0
    up = centre / np.maximum(np.linalg.norm(centre, axis=1, keepdims=True), 1e-12)
    return np.einsum("ij,ij->i", n, up) > ground_normal
