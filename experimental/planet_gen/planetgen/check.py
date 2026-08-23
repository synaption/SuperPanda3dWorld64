"""Validators that read the written tiles back off disk.

Deliberately independent of the build: these load .npz files and re-derive
adjacency from the vertex ids they find, so a bug in the export path cannot
hide behind the in-memory structure that produced it.
"""

import numpy as np


def load_tiles(root, lod=0):
    tiles = {}
    for path in sorted((root / "tiles" / f"lod{lod}").glob("*.npz")):
        with np.load(path) as z:
            tiles[path.stem] = {k: z[k] for k in z.files}
    return tiles


def seam_report(tiles):
    """Every pair of tiles sharing vertices must agree on them exactly.

    Positions, normals and material indices all have to match. Positions alone
    would pass a planet whose seams are geometrically perfect and still show a
    hard lighting line down every tile boundary.
    """
    ids = {name: t["vertex_ids"] for name, t in tiles.items()}
    order = {name: np.argsort(v) for name, v in ids.items()}
    sorted_ids = {name: ids[name][order[name]] for name in ids}
    names = sorted(tiles)
    shared_pairs, failures = 0, []
    worst = {"positions": 0.0, "normals": 0.0, "material": 0}
    for a in range(len(names)):
        for b in range(a + 1, len(names)):
            na, nb = names[a], names[b]
            common = np.intersect1d(sorted_ids[na], sorted_ids[nb])
            if common.size < 2:
                continue
            shared_pairs += 1
            pick_a = order[na][np.searchsorted(sorted_ids[na], common)]
            pick_b = order[nb][np.searchsorted(sorted_ids[nb], common)]
            deltas = {}
            for field in ("positions", "normals"):
                deltas[field] = float(np.abs(tiles[na][field][pick_a]
                                             - tiles[nb][field][pick_b]).max())
            deltas["material"] = int(np.abs(
                tiles[na]["material"][pick_a].astype(int)
                - tiles[nb]["material"][pick_b].astype(int)).max())
            for field, value in deltas.items():
                worst[field] = max(worst[field], value)
            if any(v > 0 for v in deltas.values()):
                failures.append((na, nb, common.size, deltas))
    return {"pairs": shared_pairs, "failures": failures, "worst": worst}


def _find(parent, x):
    while parent[x] != x:
        parent[x] = parent[parent[x]]
        x = parent[x]
    return x


def traversal_report(tiles, min_area_triangles=8):
    """Flood-fill walkable ground across tile seams.

    Not an error report. "There is no way up there" is sometimes the design --
    but on a planet this size an unreachable region is very easy to create by
    accident and very hard to notice by eye, so it should at least be a
    decision rather than a surprise.
    """
    edges = {}
    tri_count = 0
    tri_area = []
    for name, t in tiles.items():
        gid = t["vertex_ids"]
        tris = gid[t["triangles"]]
        walk = t["walkable"]
        pos = t["positions"]
        a, b, c = (pos[t["triangles"][:, i]] for i in range(3))
        area = 0.5 * np.linalg.norm(np.cross(b - a, c - a), axis=1)
        for local, keep in enumerate(walk):
            if not keep:
                continue
            index = tri_count
            tri_count += 1
            tri_area.append(area[local])
            v = tris[local]
            for e in ((v[0], v[1]), (v[1], v[2]), (v[2], v[0])):
                edges.setdefault((min(e), max(e)), []).append(index)

    parent = list(range(tri_count))
    for members in edges.values():
        first = members[0]
        for other in members[1:]:
            ra, rb = _find(parent, first), _find(parent, other)
            if ra != rb:
                parent[ra] = rb

    groups = {}
    for i in range(tri_count):
        groups.setdefault(_find(parent, i), []).append(i)
    sizes = sorted((len(v) for v in groups.values()), reverse=True)
    areas = sorted((sum(tri_area[i] for i in v) for v in groups.values()), reverse=True)
    return {
        "walkable_triangles": tri_count,
        "regions": len(sizes),
        "largest": sizes[0] if sizes else 0,
        "largest_area": areas[0] if areas else 0.0,
        "total_area": float(sum(tri_area)),
        "slivers": sum(1 for s in sizes if s < min_area_triangles),
        "sizes": sizes[:10],
    }
