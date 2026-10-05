# SPDX-License-Identifier: MPL-2.0
"""Visual baseline comparator regressions (tests/visual/compare.py) on synthetic images.

Kept with the e2e harness tests so the required tooling job runs them; no editor
is launched and nothing is rendered.
"""
import json
from pathlib import Path
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "visual"))
import compare  # noqa: E402


def bmp(path, width, height, pixel, top_down=True):
    """A 32-bit BGRA BMP like the offscreen capture writes."""
    rows = [bytes(pixel(x, y)[::-1] + (255,)) for y in range(height) for x in range(width)]
    body = b"".join(rows) if top_down else b"".join(b"".join(rows[y * width:(y + 1) * width]) for y in reversed(range(height)))
    header = b"BM" + struct.pack("<I4xI", 54 + len(body), 54) + struct.pack("<IiiHHI20x", 40, width, -height if top_down else height, 1, 32, 0)
    path.write_bytes(header + body)


class VisualCompareTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def image(self, width=8, height=4, changed=()):
        return compare.Image(width, height, b"".join(bytes((200, 30, 90)) if (x, y) in changed else bytes((x * 20 % 256, y * 40 % 256, 7))
                                                     for y in range(height) for x in range(width)))

    def test_bmp_orientation_and_png_round_trip_are_lossless(self):
        pixel = lambda x, y: (x * 20, y * 40, 7)
        for top_down in (True, False):
            bmp(self.root / "cell.bmp", 8, 4, pixel, top_down)
            self.assertEqual(compare.read_bmp(self.root / "cell.bmp").rgb, self.image().rgb)
        compare.write_png(self.root / "cell.png", self.image())
        self.assertEqual(compare.read_png(self.root / "cell.png").rgb, self.image().rgb)

    def test_png_reader_handles_every_filter_and_alpha(self):
        width, height = 3, 5
        pixels = [bytes((x * 60 + y, y * 50, 255 - x, 255)) for y in range(height) for x in range(width)]
        stride, rows, previous = width * 4, bytearray(), bytes(width * 4)
        for y, kind in enumerate((0, 1, 2, 3, 4)):
            line = b"".join(pixels[y * width:(y + 1) * width])
            encoded = bytearray()
            for x in range(stride):
                left = line[x - 4] if x >= 4 else 0
                up, corner = previous[x], previous[x - 4] if x >= 4 else 0
                estimate = left + up - corner
                paeth = min((abs(estimate - left), 0, left), (abs(estimate - up), 1, up), (abs(estimate - corner), 2, corner))[2]
                predictor = (0, left, up, (left + up) // 2, paeth)[kind]
                encoded.append((line[x] - predictor) & 0xFF)
            rows += bytes((kind,)) + encoded
            previous = line
        chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        raw = compare.PNG_SIGNATURE + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) \
            + chunk(b"IDAT", zlib.compress(bytes(rows))) + chunk(b"IEND", b"")
        (self.root / "rgba.png").write_bytes(raw)
        self.assertEqual(compare.read_png(self.root / "rgba.png").rgb, b"".join(pixel[:3] for pixel in pixels))
        (self.root / "rgba.png").write_bytes(raw.replace(b"IEND", b"IENX"))
        with self.assertRaisesRegex(ValueError, "corrupt"):
            compare.read_png(self.root / "rgba.png")

    def test_tolerance_accepts_antialiasing_and_rejects_real_changes(self):
        baseline = self.image(100, 100)
        noisy = bytearray(baseline.rgb)
        noisy[0] = (noisy[0] + compare.CHANNEL_TOLERANCE) & 0xFF
        report, _ = compare.compare(compare.Image(100, 100, noisy), baseline)
        self.assertTrue(report["pass"])
        self.assertEqual(report["differing_pixels"], 0)
        # 20 of 10,000 pixels (0.2%) is the limit; 21 fails.
        report, _ = compare.compare(self.image(100, 100, {(x, 0) for x in range(20)}), baseline)
        self.assertTrue(report["pass"])
        report, mask = compare.compare(self.image(100, 100, {(x, 0) for x in range(21)}), baseline)
        self.assertFalse(report["pass"])
        self.assertEqual(sum(mask), 21)
        report, mask = compare.compare(self.image(99, 100), baseline)
        self.assertFalse(report["pass"])
        self.assertIsNone(mask)

    def test_promote_then_compare_requires_reviewed_unchanged_baselines(self):
        actual, baselines = self.root / "actual", self.root / "baselines"
        actual.mkdir()
        for cell in compare.CELLS:
            bmp(actual / f"{cell}.bmp", 6, 3, lambda x, y: (x * 30, y * 60, 11))
        run = lambda allow=False: SimpleNamespace(actual=actual, baselines=baselines, report=self.root / "report",
                                                  allow_missing_baselines=allow)
        with self.assertRaisesRegex(ValueError, "No reviewed visual baselines"):
            compare.compare_command(run())
        self.assertEqual(compare.compare_command(run(allow=True)), 0)
        compare.promote_command(SimpleNamespace(actual=actual, baselines=baselines, reviewer="synthetic reviewer", source="test"))
        self.assertEqual(json.loads((baselines / "manifest.json").read_text())["reviewed_by"], "synthetic reviewer")
        self.assertEqual(compare.compare_command(run()), 0)
        bmp(actual / "dark-150pct.bmp", 6, 3, lambda x, y: (255, 0, 0))
        self.assertEqual(compare.compare_command(run()), 1)
        report = json.loads((self.root / "report/report.json").read_text())
        self.assertFalse(report["cells"]["dark-150pct"]["pass"])
        self.assertTrue((self.root / "report/dark-150pct-diff.png").is_file())
        (baselines / "light-100pct.png").write_bytes((baselines / "dark-100pct.png").read_bytes() + b"\0")
        with self.assertRaisesRegex(ValueError, "differs from its reviewed manifest"):
            compare.compare_command(run())


if __name__ == "__main__":
    unittest.main()
