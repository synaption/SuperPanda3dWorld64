"""Tests for the planet generator. Plain unittest, no Blender, no pytest.

    python3 -m unittest discover -s tests -v

These cover the failure modes that are invisible by eye: a seam that is off by
a millimetre, a face-edge rotation that mirrors a face, a LOD that quietly
regenerates instead of subsampling.
"""

import math
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from planetgen import check, manifest, rasters, surface             # noqa: E402
from planetgen.cubesphere import (VertexGrid, face_directions,      # noqa: E402
                                  grid_parameters, tile_quad_indices,
                                  tiles_at, warp)

ROOT = Path(__file__).resolve().parent.parent


class TestWarp(unittest.TestCase):
    def test_endpoints_are_exact(self):
        # A face edge landing a half-ulp short of the cube's edge does not
        # match the neighbouring face's edge, which breaks every seam.
        self.assertEqual(float(warp(1.0)), 1.0)
        self.assertEqual(float(warp(-1.0)), -1.0)
        self.assertEqual(float(warp(0.0)), 0.0)

    def test_is_odd_and_monotonic(self):
        t = np.linspace(-1, 1, 101)
        np.testing.assert_allclose(warp(-t), -warp(t), atol=0)
        self.assertTrue(np.all(np.diff(warp(t)) > 0))

    def test_gives_uniform_angles(self):
        # The whole point: equal steps in face space become equal angles.
        t = grid_parameters(16)
        d = np.stack([warp(t), np.zeros_like(t), np.ones_like(t)], axis=-1)
        d /= np.linalg.norm(d, axis=-1, keepdims=True)
        angles = np.degrees(np.arccos(np.clip(np.einsum(
            "ij,ij->i", d[:-1], d[1:]), -1, 1)))
        self.assertLess(angles.max() / angles.min(), 1.10)


class TestVertexGrid(unittest.TestCase):
    def test_welded_count_matches_euler(self):
        # A cube subdivided n times per face has exactly 6n^2 + 2 vertices.
        # Any other number means two faces failed to agree on a shared edge.
        for n in (1, 2, 3, 4, 8, 16):
            with self.subTest(n=n):
                self.assertEqual(VertexGrid(n).count, 6 * n * n + 2)

    def test_every_face_edge_is_shared(self):
        grid = VertexGrid(8)
        counts = {}
        for face in range(6):
            for index in np.unique(grid.ids[face]):
                counts[int(index)] = counts.get(int(index), 0) + 1
        shared = sorted(set(counts.values()))
        # Interior vertices belong to one face, edges to two, cube corners to
        # three. Nothing else may occur.
        self.assertEqual(shared, [1, 2, 3])
        self.assertEqual(sum(1 for v in counts.values() if v == 3), 8)

    def test_shared_vertices_have_one_direction(self):
        grid = VertexGrid(8)
        for face in range(6):
            d = face_directions(face, 8)
            np.testing.assert_allclose(
                grid.directions[grid.ids[face]], d, atol=1e-12)

    def test_directions_are_unit_length(self):
        grid = VertexGrid(8)
        np.testing.assert_allclose(
            np.linalg.norm(grid.directions, axis=1), 1.0, atol=1e-12)


class TestTiles(unittest.TestCase):
    def test_adjacent_tiles_share_a_full_ring(self):
        res, depth = 4, 1
        grid = VertexGrid(res * 2 ** depth)
        left = set(grid.tile_ids(0, depth, 0, 0, res).ravel().tolist())
        right = set(grid.tile_ids(0, depth, 1, 0, res).ravel().tolist())
        # Not "some vertices in common" -- exactly one full edge, res + 1 of
        # them, which is what a shared vertex ring means.
        self.assertEqual(len(left & right), res + 1)

    def test_triangles_wind_outward(self):
        grid = VertexGrid(4)
        local = tile_quad_indices(4)
        for face, tu, tv in tiles_at(0):
            block = grid.tile_ids(face, 0, tu, tv, 4).ravel()
            pos = grid.directions[block]
            a, b, c = (pos[local[:, i]] for i in range(3))
            normal = np.cross(b - a, c - a)
            centre = (a + b + c) / 3.0
            with self.subTest(face=face):
                self.assertTrue(np.all(np.einsum("ij,ij->i", normal, centre) > 0))

    def test_lod1_vertices_are_a_subset_of_lod0(self):
        # LOD1 must be a subsample of LOD0, never an independent generation
        # pass: that is what stops the planet changing shape as you fly in.
        res, depth = 4, 2
        grid = VertexGrid(res * 2 ** depth)
        fine = set()
        for face, tu, tv in tiles_at(depth):
            fine.update(grid.tile_ids(face, depth, tu, tv, res).ravel().tolist())
        stride = 2 ** depth
        coarse = set(grid.ids[:, ::stride, ::stride].ravel().tolist())
        self.assertTrue(coarse.issubset(fine))


class TestSurface(unittest.TestCase):
    def test_ground_normal_matches_the_game(self):
        # src/level.rs:34 -- keep these in step or the generator's idea of
        # walkable stops matching what the player can actually stand on.
        self.assertAlmostEqual(manifest.DEFAULTS["ground_normal"], 0.7)
        self.assertAlmostEqual(math.degrees(math.acos(0.7)), 45.57, places=1)

    def test_walkable_uses_radial_up_not_world_y(self):
        # A flat patch on the far side of the planet is walkable. It would fail
        # any test written against +Y, which is the bug this guards.
        r = 300.0
        for axis in (0, 1, 2):
            for sign in (1, -1):
                up = np.zeros(3)
                up[axis] = sign
                e1 = np.roll(up, 1)
                e2 = np.cross(up, e1)
                tri = np.array([up * r, up * r + e1 * 5, up * r + e2 * 5])
                walk = surface.walkable_triangles(tri, np.array([[0, 1, 2]]), 0.7)
                with self.subTest(axis=axis, sign=sign):
                    self.assertTrue(bool(walk[0]))


class TestMaterialRasterRoundTrip(unittest.TestCase):
    """The face map is a 2x supersample, so writing it and reading it back use
    two different resolutions. Getting the read side wrong is invisible on a
    first build -- that branch only runs once the PNG exists."""

    def _write_pick(self, n, res):
        """The mapping cli._sync_material_rasters uses to write the raster."""
        step = n / (res - 1)
        return np.clip(np.rint(np.arange(res) * step).astype(int), 0, n)

    def test_indices_survive_write_then_read(self):
        for n, res in ((8, 17), (16, 33), (256, 513)):
            with self.subTest(n=n, res=res):
                rng = np.random.default_rng(0)
                values = rng.integers(0, 256, size=(n + 1, n + 1), dtype=np.uint8)

                pick = self._write_pick(n, res)
                painted = values[np.ix_(pick, pick)]
                self.assertEqual(painted.shape, (res, res))

                t = np.linspace(-1.0, 1.0, n + 1)
                read = rasters.sample_nearest(
                    painted,
                    np.broadcast_to(t[np.newaxis, :], (n + 1, n + 1)),
                    np.broadcast_to(t[:, np.newaxis], (n + 1, n + 1)))

                self.assertEqual(read.shape, (n + 1, n + 1))
                np.testing.assert_array_equal(read, values)

    def test_a_painted_edit_lands_on_the_vertex_it_was_painted_over(self):
        """Nearest sampling, not bilinear: index 3 next to index 7 must never
        read back as 5."""
        n, res = 16, 33
        painted = np.full((res, res), 3, dtype=np.uint8)
        painted[:, res // 2:] = 7

        t = np.linspace(-1.0, 1.0, n + 1)
        read = rasters.sample_nearest(
            painted,
            np.broadcast_to(t[np.newaxis, :], (n + 1, n + 1)),
            np.broadcast_to(t[:, np.newaxis], (n + 1, n + 1)))

        self.assertEqual(set(np.unique(read).tolist()), {3, 7})


class TestBuiltPlanet(unittest.TestCase):
    """Integration checks against whatever is currently in tiles/."""

    @classmethod
    def setUpClass(cls):
        cls.tiles = {lod: check.load_tiles(ROOT, lod) for lod in (0, 1)}
        if not cls.tiles[0]:
            raise unittest.SkipTest("no built tiles; run `planetgen build`")

    def test_seams_agree_exactly(self):
        for lod in (0, 1):
            with self.subTest(lod=lod):
                report = check.seam_report(self.tiles[lod])
                self.assertGreater(report["pairs"], 0)
                self.assertEqual(report["failures"], [])
                self.assertEqual(report["worst"]["positions"], 0.0)
                self.assertEqual(report["worst"]["normals"], 0.0)
                self.assertEqual(report["worst"]["material"], 0)

    def test_every_tile_pair_count_is_the_edge_count(self):
        # 96 tiles x 4 edges / 2 = 192 at LOD0; the 12 cube edges at LOD1.
        self.assertEqual(check.seam_report(self.tiles[0])["pairs"], 192)
        self.assertEqual(check.seam_report(self.tiles[1])["pairs"], 12)

    def test_lod1_lies_on_the_lod0_surface(self):
        fine = {}
        for tile in self.tiles[0].values():
            for gid, pos in zip(tile["vertex_ids"], tile["positions"]):
                fine[int(gid)] = pos
        checked = 0
        for tile in self.tiles[1].values():
            for gid, pos in zip(tile["vertex_ids"], tile["positions"]):
                self.assertIn(int(gid), fine)
                np.testing.assert_array_equal(fine[int(gid)], pos)
                checked += 1
        self.assertGreater(checked, 0)

    def test_the_planet_is_mostly_one_walkable_region(self):
        report = check.traversal_report(self.tiles[0])
        share = report["largest"] / report["walkable_triangles"]
        self.assertGreater(share, 0.9)


if __name__ == "__main__":
    unittest.main()
