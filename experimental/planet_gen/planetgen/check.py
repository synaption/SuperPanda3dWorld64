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


def _triangle_graph(tiles):
    """Flatten tile triangles and derive their shared-edge graph."""
    edges = {}
    fields = ("walkable", "water", "mario_walkable", "mario_accessible",
              "luna_walkable",
              "farmable", "ant_preferred", "ant_allowed")
    chunks = {field: [] for field in fields}
    areas = []
    tri_count = 0
    for name, t in tiles.items():
        gid = t["vertex_ids"]
        local_triangles = t["triangles"]
        tris = gid[local_triangles]
        pos = t["positions"]
        a, b, c = (pos[local_triangles[:, i]] for i in range(3))
        area = 0.5 * np.linalg.norm(np.cross(b - a, c - a), axis=1)
        areas.append(area)
        count = len(tris)
        for field in fields:
            chunks[field].append(np.asarray(
                t[field] if field in t else np.zeros(count, dtype=bool), dtype=bool))
        for local, vertices in enumerate(tris):
            index = tri_count + local
            for e in ((vertices[0], vertices[1]), (vertices[1], vertices[2]),
                      (vertices[2], vertices[0])):
                edges.setdefault((min(e), max(e)), []).append(index)
        tri_count += count
    neighbours = [[] for _ in range(tri_count)]
    for members in edges.values():
        for index in members:
            neighbours[index].extend(other for other in members if other != index)
    records = {field: np.concatenate(value) for field, value in chunks.items()}
    records["area"] = np.concatenate(areas)
    return records, neighbours


def _components(records, neighbours, field):
    chosen = records[field]
    parent = list(range(len(chosen)))
    for here, adjacent in enumerate(neighbours):
        if not chosen[here]:
            continue
        for there in adjacent:
            if not chosen[there]:
                continue
            ra, rb = _find(parent, here), _find(parent, there)
            if ra != rb:
                parent[ra] = rb
    groups = {}
    for index, keep in enumerate(chosen):
        if keep:
            groups.setdefault(_find(parent, index), []).append(index)
    return sorted(groups.values(), key=len, reverse=True)


def traversal_report(tiles, min_area_triangles=8, field="walkable"):
    """Flood-fill one traversal class across tile seams.

    Not an error report. "There is no way up there" is sometimes the design --
    but on a planet this size an unreachable region is very easy to create by
    accident and very hard to notice by eye, so it should at least be a
    decision rather than a surprise.
    """
    records, neighbours = _triangle_graph(tiles)
    groups = _components(records, neighbours, field)
    sizes = [len(group) for group in groups]
    areas = [float(records["area"][group].sum()) for group in groups]
    tri_count = sum(sizes)
    return {
        "walkable_triangles": tri_count,
        "regions": len(sizes),
        "largest": sizes[0] if sizes else 0,
        "largest_area": areas[0] if areas else 0.0,
        "total_area": float(sum(areas)),
        "slivers": sum(1 for s in sizes if s < min_area_triangles),
        "sizes": sizes[:10],
    }


def topology_report(tiles):
    """Report the gameplay promises carried by the generated topology."""
    records, neighbours = _triangle_graph(tiles)
    mario_groups = _components(records, neighbours, "mario_walkable")
    mario_total = sum(len(group) for group in mario_groups)
    farm = set(np.flatnonzero(records["farmable"]).tolist())
    water = set(np.flatnonzero(records["water"]).tolist())
    underwater_walkable = set(np.flatnonzero(
        records["water"] & records["luna_walkable"]).tolist())
    shore = {
        dry for dry, is_dry in enumerate(records["mario_walkable"])
        if is_dry
        and any(wet in water for wet in neighbours[dry])
    }
    shore_groups = [group for group in mario_groups if any(i in shore for i in group)]
    water_accessible = set().union(*map(set, shore_groups)) if shore_groups else set()
    farm_area = float(records["area"][list(farm)].sum()) if farm else 0.0
    reachable = list(farm & water_accessible)
    reachable_farm_area = float(records["area"][reachable].sum()) if reachable else 0.0
    return {
        "mario_regions": len(mario_groups),
        "mario_walkable_triangles": mario_total,
        "mario_regions_reaching_water": len(shore_groups),
        "mario_water_access_fraction": len(water_accessible) / max(mario_total, 1),
        "shore_triangles": len(shore),
        "farmable_triangles": len(farm),
        "farmable_area": farm_area,
        "reachable_farm_fraction": min(
            reachable_farm_area / max(farm_area, 1e-12), 1.0),
        "underwater_triangles": len(water),
        "luna_underwater_walkable_fraction": (
            len(underwater_walkable) / max(len(water), 1)
        ),
        "ant_prefers_dry": all(
            not records["water"][i]
            for i in np.flatnonzero(records["ant_preferred"])
        ),
        "ant_water_is_safe": bool(np.all(
            ~records["luna_walkable"] | records["ant_allowed"])),
    }
