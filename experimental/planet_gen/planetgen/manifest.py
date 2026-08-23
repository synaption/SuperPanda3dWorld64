"""planet.json: every number that shapes the planet, in one editable file."""

import json

from . import surface

DEFAULTS = {
    "radius": 300.0,
    "depth": 2,
    "tile_res": 64,
    "face_map_res": 513,
    "min_altitude": -40.0,
    "max_altitude": 40.0,
    "sea_level": 0.0,
    "snow_line": 22.0,
    "steep_cos": 0.55,
    "ground_normal": 0.7,
    "lod1_depth": 0,
    "seed": 20260822,
    "relief": 1.0,
    "detail": 0.22,
    "detail_frequency": 4.5,
    "detail_octaves": 5,
    "generator_version": 1,
}


def load(root):
    path = root / "planet.json"
    data = dict(DEFAULTS)
    if path.is_file():
        data.update(json.loads(path.read_text()))
    data["materials"] = list(surface.MATERIALS)
    return data


def save(root, data):
    path = root / "planet.json"
    out = {k: v for k, v in data.items() if k != "materials"}
    path.write_text(json.dumps(out, indent=2) + "\n")
    return path


def grid_size(m):
    """Quads along a full face edge at LOD0."""
    return m["tile_res"] * (2 ** m["depth"])
