"""Builds the three diagnostic fixtures the solar system gains on `test_world 1`.

Each is its own glb and its own body -- they stand in the space between the
sun and the planets' orbits, in addition to the planets and not in place of
them -- so movement bugs can be told apart from terrain: a velocity that
jerks on the perfectly smooth sphere is not the ground's doing, one that
jerks on the flat platform is not the curvature's either, and the toroid is
every slope class at once with none of them accidental.

    assets/bevy/test_sphere.glb    a smooth icosphere, planet-sized
    assets/bevy/test_platform.glb  a flat circular platform, planet-sized
    assets/bevy/test_torus.glb     a toroid

Where each stands, and how its gravity pulls, lives in `world::FIXTURES`.

    python3 tools/build_test_world.py
"""

import json
import struct
from pathlib import Path

import numpy as np

SPHERE_RADIUS = 300.0
# Six subdivisions of an icosahedron: ~5 m edges, under a tenth of a degree
# of normal change per edge. Any felt bump on this surface is not geometry.
SUBDIVISIONS = 6
# The platform: planet-scale (300 m across). Its body pulls uniformly along
# its own -Y (see `gravity::Well::down`), so the whole face is level ground
# the way the castle's lawn is -- the flat control for the round experiments.
PLATFORM_RADIUS = 150.0
PLATFORM_THICKNESS = 0.5
PLATFORM_RINGS = 20
PLATFORM_SECTORS = 64
# The toroid: its outward band is a curved floor and its flanks are walls,
# which is the point -- one body with every slope class on it, none of them
# accidental. Radial gravity towards its centre, so the outer equator is
# walkable ground and the hole is open sky.
TORUS_MAJOR = 380.0
TORUS_MINOR = 30.0
TORUS_MAJOR_SECTORS = 192
TORUS_MINOR_SECTORS = 32


def icosphere(radius: float, subdivisions: int):
    phi = (1.0 + 5.0**0.5) / 2.0
    verts = np.array(
        [
            [-1, phi, 0], [1, phi, 0], [-1, -phi, 0], [1, -phi, 0],
            [0, -1, phi], [0, 1, phi], [0, -1, -phi], [0, 1, -phi],
            [phi, 0, -1], [phi, 0, 1], [-phi, 0, -1], [-phi, 0, 1],
        ],
        dtype=np.float64,
    )
    verts /= np.linalg.norm(verts, axis=1)[:, None]
    faces = np.array(
        [
            [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
            [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
            [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
            [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
        ],
        dtype=np.int64,
    )
    for _ in range(subdivisions):
        edges = {}
        verts = list(verts)

        def midpoint(a, b):
            key = (min(a, b), max(a, b))
            if key not in edges:
                middle = (verts[a] + verts[b]) / 2.0
                middle /= np.linalg.norm(middle)
                edges[key] = len(verts)
                verts.append(middle)
            return edges[key]

        split = []
        for a, b, c in faces:
            ab, bc, ca = midpoint(a, b), midpoint(b, c), midpoint(c, a)
            split += [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]
        verts = np.array(verts)
        faces = np.array(split, dtype=np.int64)
    return (verts * radius).astype(np.float32), verts.astype(np.float32), faces


def platform():
    """A disc with a matching underside, so it reads from below too. The top
    face sits at y = 0, which is where its body's gravity plane is."""
    verts, normals, faces = [], [], []

    def disc(y: float, up: float):
        base = len(verts)
        verts.append([0.0, y, 0.0])
        normals.append([0.0, up, 0.0])
        for ring in range(1, PLATFORM_RINGS + 1):
            r = PLATFORM_RADIUS * ring / PLATFORM_RINGS
            for sector in range(PLATFORM_SECTORS):
                angle = 2.0 * np.pi * sector / PLATFORM_SECTORS
                verts.append([r * np.cos(angle), y, r * np.sin(angle)])
                normals.append([0.0, up, 0.0])
        at = lambda ring, sector: (
            base if ring == 0 else base + 1 + (ring - 1) * PLATFORM_SECTORS + sector % PLATFORM_SECTORS
        )
        for ring in range(PLATFORM_RINGS):
            for sector in range(PLATFORM_SECTORS):
                a, b = at(ring, sector), at(ring, sector + 1)
                c, d = at(ring + 1, sector), at(ring + 1, sector + 1)
                # Counter-clockwise seen from the side the normal leaves by.
                if up > 0:
                    if ring > 0:
                        faces.append([a, b, d])
                    faces.append([a, d, c])
                else:
                    if ring > 0:
                        faces.append([a, d, b])
                    faces.append([a, c, d])

    disc(0.0, 1.0)
    disc(-PLATFORM_THICKNESS, -1.0)
    return (
        np.array(verts, dtype=np.float32),
        np.array(normals, dtype=np.float32),
        np.array(faces, dtype=np.int64),
    )


def torus():
    verts, normals, faces = [], [], []
    for major in range(TORUS_MAJOR_SECTORS):
        theta = 2.0 * np.pi * major / TORUS_MAJOR_SECTORS
        ring_out = np.array([np.cos(theta), 0.0, np.sin(theta)])
        for minor in range(TORUS_MINOR_SECTORS):
            phi = 2.0 * np.pi * minor / TORUS_MINOR_SECTORS
            normal = ring_out * np.cos(phi) + np.array([0.0, np.sin(phi), 0.0])
            verts.append(ring_out * TORUS_MAJOR + normal * TORUS_MINOR)
            normals.append(normal)
    at = lambda major, minor: (
        (major % TORUS_MAJOR_SECTORS) * TORUS_MINOR_SECTORS + minor % TORUS_MINOR_SECTORS
    )
    for major in range(TORUS_MAJOR_SECTORS):
        for minor in range(TORUS_MINOR_SECTORS):
            a, b = at(major, minor), at(major + 1, minor)
            c, d = at(major, minor + 1), at(major + 1, minor + 1)
            faces += [[a, c, b], [b, c, d]]
    return (
        np.array(verts, dtype=np.float32),
        np.array(normals, dtype=np.float32),
        np.array(faces, dtype=np.int64),
    )


def glb(out: Path, name: str, pos, norm, idx, colour):
    binary = bytearray()
    views, accessors = [], []

    def push(array, target, component, kind):
        data = array.astype("<f4" if component == 5126 else "<u4").tobytes()
        offset = len(binary)
        binary.extend(data)
        while len(binary) % 4:
            binary.append(0)
        views.append(
            {"buffer": 0, "byteOffset": offset, "byteLength": len(data), "target": target}
        )
        accessor = {
            "bufferView": len(views) - 1,
            "componentType": component,
            "count": len(array),
            "type": kind,
        }
        if kind == "VEC3":
            accessor["min"] = array.min(axis=0).tolist()
            accessor["max"] = array.max(axis=0).tolist()
        accessors.append(accessor)
        return len(accessors) - 1

    gltf = {
        "asset": {"version": "2.0", "generator": "build_test_world.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": name, "mesh": 0}],
        "meshes": [
            {
                "name": name,
                "primitives": [
                    {
                        "attributes": {
                            "POSITION": push(pos, 34962, 5126, "VEC3"),
                            "NORMAL": push(norm, 34962, 5126, "VEC3"),
                        },
                        "indices": push(idx.reshape(-1), 34963, 5125, "SCALAR"),
                        "material": 0,
                    }
                ],
            }
        ],
        "materials": [
            {
                "name": name,
                "pbrMetallicRoughness": {
                    "baseColorFactor": colour + [1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
            }
        ],
        "buffers": [{"byteLength": len(binary)}],
        "bufferViews": views,
        "accessors": accessors,
    }

    payload = json.dumps(gltf, separators=(",", ":")).encode()
    while len(payload) % 4:
        payload += b" "
    total = 12 + 8 + len(payload) + 8 + len(binary)
    with open(out, "wb") as handle:
        handle.write(struct.pack("<4sII", b"glTF", 2, total))
        handle.write(struct.pack("<I4s", len(payload), b"JSON"))
        handle.write(payload)
        handle.write(struct.pack("<I4s", len(binary), b"BIN\0"))
        handle.write(binary)
    print(f"{out}: {len(idx)} tris, {total / 1e6:.1f} MB")


if __name__ == "__main__":
    assets = Path(__file__).resolve().parent.parent / "assets" / "bevy"
    sphere_pos, sphere_norm, sphere_idx = icosphere(SPHERE_RADIUS, SUBDIVISIONS)
    glb(assets / "test_sphere.glb", "test_sphere", sphere_pos, sphere_norm, sphere_idx, [0.45, 0.62, 0.42])
    plate_pos, plate_norm, plate_idx = platform()
    glb(assets / "test_platform.glb", "test_platform", plate_pos, plate_norm, plate_idx, [0.5, 0.55, 0.68])
    torus_pos, torus_norm, torus_idx = torus()
    glb(assets / "test_torus.glb", "test_torus", torus_pos, torus_norm, torus_idx, [0.7, 0.5, 0.35])
