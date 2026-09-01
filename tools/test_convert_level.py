import math
import unittest

from tools.convert_level import uniform_scale, world_scale


class CastleScaleTest(unittest.TestCase):
    def test_uniform_positive_scale_is_accepted(self):
        self.assertEqual(uniform_scale((2.0, 2.0, 2.0)), 2.0)

    def test_nearly_uniform_blender_values_are_accepted(self):
        self.assertTrue(math.isclose(
            uniform_scale((2.0, 2.000001, 2.0)), 2.0))

    def test_non_uniform_scale_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "source proportions"):
            uniform_scale((2.0, 1.0, 2.0))

    def test_zero_or_negative_scale_is_rejected(self):
        for values in ((0.0, 0.0, 0.0), (-1.0, -1.0, -1.0)):
            with self.subTest(values=values):
                with self.assertRaisesRegex(ValueError, "positive"):
                    uniform_scale(values)

    def test_blender_metres_define_the_source_conversion(self):
        self.assertEqual(
            world_scale((300.0, 100.0, 200.0), (2.0, 3.0, 1.0)),
            0.01)

    def test_applied_non_uniform_resize_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "source proportions"):
            world_scale((100.0, 200.0, 300.0), (1.0, 2.0, 6.0))


if __name__ == "__main__":
    unittest.main()
