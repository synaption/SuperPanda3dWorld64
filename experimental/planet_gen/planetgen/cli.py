"""planetgen -- build a cube-sphere planet from authored face rasters."""

import argparse
import sys
from pathlib import Path

import numpy as np

from . import check, manifest, rasters, surface
from .build import Planet
from .cubesphere import face_directions

ROOT = Path(__file__).resolve().parent.parent


def cmd_init_faces(args):
    m = manifest.load(ROOT)
    if args.seed is not None:
        m["seed"] = args.seed
    for key in ("relief", "detail", "detail_frequency"):
        if getattr(args, key, None) is not None:
            m[key] = getattr(args, key)
    manifest.save(ROOT, m)

    n = m["face_map_res"] - 1
    directions = [face_directions(f, n) for f in range(6)]
    fields = rasters.seed_elevation(directions, m["seed"], m["relief"],
                                    m["detail"], m["detail_frequency"],
                                    m["detail_octaves"])
    for face, field in enumerate(fields):
        path = rasters.face_path(ROOT, rasters.ELEVATION, face)
        rasters.save_elevation(path, field)
        print(f"  {path.relative_to(ROOT)}  {field.shape[0]}x{field.shape[1]} 16-bit")
    land = np.concatenate([f.ravel() for f in fields])
    print(f"seed {m['seed']}, relief {m['relief']}: "
          f"{100 * (land > 0.5).mean():.1f}% above sea level")
    return 0


def cmd_build(args):
    m = manifest.load(ROOT)
    if not rasters.face_path(ROOT, rasters.ELEVATION, 0).is_file():
        print("no face rasters; run `init-faces` first", file=sys.stderr)
        return 1

    planet = Planet(ROOT, m).build()
    n = planet.n
    print(f"grid {n}x{n} per face, {planet.grid.count:,} welded vertices, "
          f"{len(planet.all_triangles):,} triangles")

    # First build writes paintable material maps; later builds read whatever is
    # there, so a hand edit survives a rebuild.
    _sync_material_rasters(planet, m)

    for lod in (0, 1):
        written = planet.write_tiles(lod=lod)
        print(f"lod{lod}: {len(written)} tiles -> tiles/lod{lod}/")

    alt = planet.altitude
    walk = surface.walkable_triangles(planet.positions, planet.all_triangles,
                                      m["ground_normal"])
    print(f"altitude {alt.min():+.1f} m to {alt.max():+.1f} m about r={m['radius']:.0f}")
    print(f"land {100 * (alt > m['sea_level']).mean():.1f}%, "
          f"walkable triangles {100 * walk.mean():.1f}%")
    return 0


def _sync_material_rasters(planet, m):
    res = m["face_map_res"]
    # Sampled at the vertex grid, not the raster: the face map is a 2x
    # supersample (513 px against 257 vertices), so its own resolution is the
    # wrong shape to ask for. This is the inverse of the `pick` mapping below.
    t = np.linspace(-1.0, 1.0, planet.n + 1)
    for face in range(6):
        path = rasters.face_path(ROOT, rasters.MATERIAL, face)
        if path.is_file():
            painted = rasters.load_index(path)
            planet.material[planet.grid.ids[face]] = rasters.sample_nearest(
                painted,
                np.broadcast_to(t[np.newaxis, :], (planet.n + 1, planet.n + 1)),
                np.broadcast_to(t[:, np.newaxis], (planet.n + 1, planet.n + 1)))
            continue
        ids = planet.grid.ids[face]
        step = planet.n / (res - 1)
        pick = np.clip(np.rint(np.arange(res) * step).astype(int), 0, planet.n)
        rasters.save_index(path, planet.material[ids[np.ix_(pick, pick)]])
        print(f"  wrote {path.relative_to(ROOT)} (paint it; rebuilds read it back)")


def cmd_check(args):
    tiles = check.load_tiles(ROOT, lod=args.lod)
    if not tiles:
        print("no tiles; run `build` first", file=sys.stderr)
        return 1
    seams = check.seam_report(tiles)
    w = seams["worst"]
    print(f"seams: {seams['pairs']} adjacent tile pairs; worst disagreement "
          f"position {w['positions']:.3g} m, normal {w['normals']:.3g}, "
          f"material {w['material']}")
    for a, b, count, deltas in seams["failures"][:10]:
        print(f"  FAIL {a} / {b}: {count} shared vertices, {deltas}")
    ok = not seams["failures"]

    if args.traversal:
        t = check.traversal_report(tiles)
        print(f"traversal: {t['walkable_triangles']:,} walkable triangles in "
              f"{t['regions']} regions")
        print(f"  largest region {t['largest']:,} triangles "
              f"({100 * t['largest'] / max(t['walkable_triangles'], 1):.1f}% of walkable, "
              f"{t['largest_area']:,.0f} m2)")
        print(f"  isolated pockets under 8 triangles: {t['slivers']}")
        print(f"  ten largest: {t['sizes']}")
    return 0 if ok else 1


def main(argv=None):
    p = argparse.ArgumentParser(prog="planetgen", description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)

    i = sub.add_parser("init-faces", help="seed the six elevation rasters")
    i.add_argument("--seed", type=int)
    i.add_argument("--relief", type=float)
    i.add_argument("--detail", type=float)
    i.add_argument("--detail-frequency", type=float)
    i.set_defaults(func=cmd_init_faces)

    b = sub.add_parser("build", help="rasters -> tiles")
    b.set_defaults(func=cmd_build)

    c = sub.add_parser("check", help="validate the written tiles")
    c.add_argument("--lod", type=int, default=0)
    c.add_argument("--traversal", action="store_true")
    c.set_defaults(func=cmd_check)

    args = p.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
