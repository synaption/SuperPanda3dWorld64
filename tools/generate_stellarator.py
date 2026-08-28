#!/usr/bin/env python3
"""Generate a stylized modular-coil stellarator as a binary glTF asset."""

from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from glb import ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER, FLOAT, UNSIGNED_INT, GLB


def surface_mesh(position, nu: int, nv: int):
    """Sample a closed parametric surface and return positions/normals/triangles."""
    p = np.empty((nu, nv, 3), dtype=np.float64)
    for i in range(nu):
        for j in range(nv):
            p[i, j] = position(2 * math.pi * i / nu, 2 * math.pi * j / nv)

    du = np.roll(p, -1, axis=0) - np.roll(p, 1, axis=0)
    dv = np.roll(p, -1, axis=1) - np.roll(p, 1, axis=1)
    n = np.cross(du, dv)
    n /= np.maximum(np.linalg.norm(n, axis=2, keepdims=True), 1e-12)

    faces = []
    for i in range(nu):
        for j in range(nv):
            a = i * nv + j
            b = ((i + 1) % nu) * nv + j
            c = ((i + 1) % nu) * nv + (j + 1) % nv
            d = i * nv + (j + 1) % nv
            faces.extend(((a, b, c), (a, c, d)))
    return p.reshape(-1, 3), n.reshape(-1, 3), np.asarray(faces, dtype=np.uint32)


def tube_mesh(points: np.ndarray, radius: float, sides: int = 10):
    """Sweep a circular tube along a closed polyline using stable local frames."""
    count = len(points)
    tangent = np.roll(points, -1, axis=0) - np.roll(points, 1, axis=0)
    tangent /= np.linalg.norm(tangent, axis=1, keepdims=True)
    radial = points.copy()
    radial[:, 2] = 0
    radial /= np.maximum(np.linalg.norm(radial, axis=1, keepdims=True), 1e-9)
    binormal = np.cross(tangent, radial)
    binormal /= np.maximum(np.linalg.norm(binormal, axis=1, keepdims=True), 1e-9)
    normal = np.cross(binormal, tangent)

    vertices, normals = [], []
    for i in range(count):
        for j in range(sides):
            a = 2 * math.pi * j / sides
            direction = math.cos(a) * normal[i] + math.sin(a) * binormal[i]
            vertices.append(points[i] + radius * direction)
            normals.append(direction)
    faces = []
    for i in range(count):
        ni = (i + 1) % count
        for j in range(sides):
            nj = (j + 1) % sides
            a, b = i * sides + j, ni * sides + j
            c, d = ni * sides + nj, i * sides + nj
            faces.extend(((a, b, c), (a, c, d)))
    return np.asarray(vertices), np.asarray(normals), np.asarray(faces, dtype=np.uint32)


def add_material(glb: GLB, name: str, color, metallic=0.0, roughness=0.5, emissive=None):
    material = {
        "name": name,
        "pbrMetallicRoughness": {
            "baseColorFactor": list(color),
            "metallicFactor": metallic,
            "roughnessFactor": roughness,
        },
        "doubleSided": True,
    }
    if emissive:
        material["emissiveFactor"] = list(emissive)
    glb.json["materials"].append(material)
    return len(glb.json["materials"]) - 1


def add_mesh(glb: GLB, name: str, vertices, normals, faces, material: int):
    vertices = np.asarray(vertices, dtype=np.float32)
    normals = np.asarray(normals, dtype=np.float32)
    faces = np.asarray(faces, dtype=np.uint32).reshape(-1)
    pos = glb.add_array(vertices.tolist(), FLOAT, "VEC3", ARRAY_BUFFER, True)
    nor = glb.add_array(normals.tolist(), FLOAT, "VEC3", ARRAY_BUFFER)
    ind = glb.add_array(faces.tolist(), UNSIGNED_INT, "SCALAR", ELEMENT_ARRAY_BUFFER)
    glb.json["meshes"].append({
        "name": name,
        "primitives": [{"attributes": {"POSITION": pos, "NORMAL": nor}, "indices": ind,
                        "material": material}],
    })
    glb.json["nodes"].append({"name": name, "mesh": len(glb.json["meshes"]) - 1})
    glb.json["scenes"][0]["nodes"].append(len(glb.json["nodes"]) - 1)


def generate(output: Path, field_periods: int = 5, coils: int = 10):
    glb = GLB()
    glb.json["asset"]["generator"] = "Python stellarator geometry generator"
    glb.json["scenes"][0]["name"] = "Stellarator"

    plasma_mat = add_material(glb, "Plasma", (0.16, 0.52, 1.0, 0.58), 0.05, 0.22,
                              emissive=(0.05, 0.2, 0.8))
    coil_mat = add_material(glb, "Copper Coils", (0.72, 0.22, 0.055, 1), 0.82, 0.24)
    support_mat = add_material(glb, "Steel Supports", (0.12, 0.15, 0.19, 1), 0.88, 0.32)

    major, minor = 3.0, 0.72

    def plasma(u, v):
        # A toroidal plasma surface with a rotating elliptical cross section.
        twist = field_periods * u
        xsec = minor * (1.0 + 0.16 * math.cos(twist))
        r = major + xsec * math.cos(v) + 0.16 * math.cos(twist)
        z = 0.72 * minor * math.sin(v) + 0.18 * math.sin(twist)
        return (r * math.cos(u), r * math.sin(u), z)

    verts, norms, faces = surface_mesh(plasma, 160, 32)
    add_mesh(glb, "Twisted Plasma Surface", verts, norms, faces, plasma_mat)

    # Non-planar modular coils wrap around the plasma boundary.
    samples = 128
    for k in range(coils):
        phase = 2 * math.pi * k / coils
        t = np.linspace(0, 2 * math.pi, samples, endpoint=False)
        u = phase + 0.24 * np.sin(t) + 0.07 * np.sin(2 * t + field_periods * phase)
        local_twist = field_periods * u
        r = major + 1.13 * np.cos(t) + 0.2 * np.cos(local_twist)
        z = 0.92 * np.sin(t) + 0.22 * np.sin(local_twist)
        points = np.column_stack((r * np.cos(u), r * np.sin(u), z))
        v, n, f = tube_mesh(points, 0.105, 12)
        add_mesh(glb, f"Modular Coil {k + 1:02d}", v, n, f, coil_mat)

    # Two toroidal support rings communicate the machine's structural scale.
    for ring_index, z in enumerate((-1.32, 1.32), 1):
        t = np.linspace(0, 2 * math.pi, 192, endpoint=False)
        points = np.column_stack((4.18 * np.cos(t), 4.18 * np.sin(t), np.full_like(t, z)))
        v, n, f = tube_mesh(points, 0.12, 10)
        add_mesh(glb, f"Support Ring {ring_index}", v, n, f, support_mat)

    output.parent.mkdir(parents=True, exist_ok=True)
    glb.write(output)
    return len(glb.json["meshes"]), sum(a["count"] for a in glb.json["accessors"] if a["type"] == "VEC3")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", nargs="?", type=Path, default=Path("stellarator.glb"))
    parser.add_argument("--field-periods", type=int, default=5)
    parser.add_argument("--coils", type=int, default=10)
    args = parser.parse_args()
    meshes, vector_count = generate(args.output, args.field_periods, args.coils)
    print(f"Wrote {args.output} ({meshes} meshes, {vector_count // 2} vertices)")


if __name__ == "__main__":
    main()
