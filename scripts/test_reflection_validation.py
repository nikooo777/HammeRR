import tempfile
import unittest
from pathlib import Path

from reflection_validation import difference, ppm


class ReflectionEvidence(unittest.TestCase):
    def test_ppm_rejects_malformed_dimensions_and_truncated_pixels(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "capture.ppm"
            pixels = bytes(range(48))
            path.write_bytes(b"P6\n4 4\n255\n" + pixels)
            self.assertEqual(ppm(path, 4, 4), pixels)
            for malformed in [b"P3\n4 4\n255\n" + pixels,
                              b"P6\n8 2\n255\n" + pixels,
                              b"P6\n4 4\n255\n" + pixels[:-1]]:
                path.write_bytes(malformed)
                with self.subTest(header=malformed[:12]), self.assertRaises(RuntimeError):
                    ppm(path, 4, 4)

    def test_roi_metrics_exclude_unrelated_pixels_and_retain_channel_signatures(self):
        before = bytes(48)
        after = bytearray(before)
        after[0:3] = bytes([255, 255, 255])
        after[(1 * 4 + 2) * 3] = 9
        result = difference(before, after, (1, 1, 3, 3), width=4)
        self.assertEqual(result["changed_fraction"], 0.25)
        self.assertEqual(result["mean_component_difference"], 0.75)
        self.assertEqual(result["first_rgb"], [0, 0, 0])
        self.assertEqual(result["second_rgb"], [2.25, 0, 0])


if __name__ == "__main__":
    unittest.main()
