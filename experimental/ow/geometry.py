"""Procedural meshes, so the port has no asset dependencies at all."""

import math
import random

from panda3d.core import (
    Geom,
    GeomNode,
    GeomPoints,
    GeomTriangles,
    GeomVertexData,
    GeomVertexFormat,
    GeomVertexWriter,
)


def make_sphere(radius=1.0, segments=48, rings=24, color=(1, 1, 1, 1)):
    """A UV sphere with outward normals, centred on the origin."""
    fmt = GeomVertexFormat.getV3n3c4()
    vdata = GeomVertexData("sphere", fmt, Geom.UHStatic)
    vdata.setNumRows((rings + 1) * (segments + 1))
    vertex = GeomVertexWriter(vdata, "vertex")
    normal = GeomVertexWriter(vdata, "normal")
    colour = GeomVertexWriter(vdata, "color")

    for ring in range(rings + 1):
        phi = math.pi * ring / rings          # 0 at +Z pole, pi at -Z
        z, r = math.cos(phi), math.sin(phi)
        for seg in range(segments + 1):
            theta = 2.0 * math.pi * seg / segments
            nx = r * math.cos(theta)
            ny = r * math.sin(theta)
            vertex.addData3(nx * radius, ny * radius, z * radius)
            normal.addData3(nx, ny, z)
            colour.addData4(*color)

    tris = GeomTriangles(Geom.UHStatic)
    row = segments + 1
    for ring in range(rings):
        for seg in range(segments):
            a = ring * row + seg
            b = a + 1
            c = a + row
            d = c + 1
            if ring != 0:
                tris.addVertices(a, c, b)
            if ring != rings - 1:
                tris.addVertices(b, c, d)

    geom = Geom(vdata)
    geom.addPrimitive(tris)
    node = GeomNode("sphere")
    node.addGeom(geom)
    return node


def make_starfield(count=2200, radius=1.0, seed=7):
    """Points on a unit sphere, to be drawn at the far plane around the camera.

    Uses its own RNG so the sky is stable between runs without disturbing
    global random state.
    """
    rng = random.Random(seed)
    fmt = GeomVertexFormat.getV3c4()
    vdata = GeomVertexData("stars", fmt, Geom.UHStatic)
    vdata.setNumRows(count)
    vertex = GeomVertexWriter(vdata, "vertex")
    colour = GeomVertexWriter(vdata, "color")

    points = GeomPoints(Geom.UHStatic)
    for i in range(count):
        # Uniform on the sphere: z uniform, angle uniform.
        z = rng.uniform(-1.0, 1.0)
        theta = rng.uniform(0.0, 2.0 * math.pi)
        r = math.sqrt(max(0.0, 1.0 - z * z))
        vertex.addData3(r * math.cos(theta) * radius, r * math.sin(theta) * radius, z * radius)
        brightness = rng.uniform(0.25, 1.0)
        tint = rng.uniform(-0.06, 0.06)
        colour.addData4(
            min(1.0, brightness + max(0.0, tint)),
            brightness,
            min(1.0, brightness + max(0.0, -tint)),
            1.0,
        )
        points.addVertex(i)

    geom = Geom(vdata)
    geom.addPrimitive(points)
    node = GeomNode("starfield")
    node.addGeom(geom)
    return node
