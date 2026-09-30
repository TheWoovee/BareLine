# SPDX-License-Identifier: MPL-2.0
"""Compare offscreen whole-window render cells with reviewed PNG baselines (QA-13).

The capture (tests/visual/capture.ps1) writes one 32-bit BMP per cell; this
tool compares each with tests/visual/baselines/<cell>.png under a tolerance
and writes a report plus a diff image for every failing cell. Baselines are
reviewed like code: `promote` converts a capture into baselines and records the
reviewer, the tolerance and each file's SHA-256 in manifest.json.

Usage:
  python tests/visual/compare.py compare --actual DIR [--report DIR] [--allow-missing-baselines]
  python tests/visual/compare.py promote --actual DIR --reviewer NAME
Only the Python standard library is used; nothing is downloaded or launched.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys
import zlib

HERE = Path(__file__).resolve().parent
BASELINES = HERE / "baselines"
CELLS = tuple(f"{theme}-{percent}pct" for theme in ("light", "dark") for percent in (100, 150, 200))
# A pixel differs when any channel moves by more than CHANNEL_TOLERANCE; a cell
# fails when more than MAX_DIFF_FRACTION of its pixels differ. Anti-aliasing and
# caret timing stay inside these bounds; a moved control or a changed color does not.
CHANNEL_TOLERANCE = 16
MAX_DIFF_FRACTION = 0.002
MAX_PIXELS = 16 * 1024 * 1024
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


class Image:
    """8-bit RGB pixels, row-major without padding."""

    def __init__(self, width, height, rgb):
        if width <= 0 or height <= 0 or width * height > MAX_PIXELS or len(rgb) != width * height * 3:
            raise ValueError("Invalid image geometry")
        self.width, self.height, self.rgb = width, height, bytes(rgb)


def read_bmp(path):
    raw = Path(path).read_bytes()
    if raw[:2] != b"BM" or len(raw) < 54:
        raise ValueError(f"{path}: not a BMP")
    offset, header, width, height, planes, bits, compression = struct.unpack_from("<I I i i H H I", raw, 10)
    if header < 40 or planes != 1 or bits != 32 or compression != 0:
        raise ValueError(f"{path}: only uncompressed 32-bit BMP captures are supported")
    rows = abs(height)
    if width <= 0 or rows == 0 or width * rows > MAX_PIXELS or offset + width * rows * 4 > len(raw):
        raise ValueError(f"{path}: truncated or oversized BMP")
    rgb = bytearray(width * rows * 3)
    for y in range(rows):
        source = offset + (y if height < 0 else rows - 1 - y) * width * 4
        line = raw[source:source + width * 4]
        target = y * width * 3
        rgb[target:target + width * 3:3] = line[2::4]
        rgb[target + 1:target + width * 3:3] = line[1::4]
        rgb[target + 2:target + width * 3:3] = line[0::4]
    return Image(width, rows, rgb)


def _chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def write_png(path, image):
    """Lossless RGB PNG; each row uses the None or Up filter, whichever is smaller."""
    stride = image.width * 3
    rows, previous = bytearray(), bytes(stride)
    for y in range(image.height):
        line = image.rgb[y * stride:(y + 1) * stride]
        up = bytes((a - b) & 0xFF for a, b in zip(line, previous))
        cost = lambda data: sum(value if value < 128 else 256 - value for value in data)
        rows += b"\x02" + up if cost(up) < cost(line) else b"\x00" + line
        previous = line
    header = struct.pack(">IIBBBBB", image.width, image.height, 8, 2, 0, 0, 0)
    Path(path).write_bytes(PNG_SIGNATURE + _chunk(b"IHDR", header) + _chunk(b"IDAT", zlib.compress(bytes(rows), 9))
                           + _chunk(b"IEND", b""))


def read_png(path):
    raw = Path(path).read_bytes()
    if raw[:8] != PNG_SIGNATURE:
        raise ValueError(f"{path}: not a PNG")
    position, header, data = 8, None, bytearray()
    while position < len(raw):
        length, kind = struct.unpack_from(">I4s", raw, position)
        body = raw[position + 8:position + 8 + length]
        if len(body) != length or zlib.crc32(kind + body) & 0xFFFFFFFF != struct.unpack_from(">I", raw, position + 8 + length)[0]:
            raise ValueError(f"{path}: corrupt PNG chunk")
        position += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            data += body
        elif kind == b"IEND":
            break
    if not header:
        raise ValueError(f"{path}: PNG header missing")
    width, height, depth, color, _, _, interlace = header
    if depth != 8 or color not in (2, 6) or interlace or width * height > MAX_PIXELS:
        raise ValueError(f"{path}: only 8-bit non-interlaced RGB/RGBA PNG baselines are supported")
    channels = 3 if color == 2 else 4
    stride = width * channels
    packed = zlib.decompress(bytes(data))
    if len(packed) != height * (stride + 1):
        raise ValueError(f"{path}: PNG data length differs from its header")
    pixels, previous = bytearray(), bytearray(stride)
    for y in range(height):
        kind, line = packed[y * (stride + 1)], bytearray(packed[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride if kind else 0):
            left = line[x - channels] if x >= channels else 0
            up, corner = previous[x], previous[x - channels] if x >= channels else 0
            if kind == 1:
                line[x] = (line[x] + left) & 0xFF
            elif kind == 2:
                line[x] = (line[x] + up) & 0xFF
            elif kind == 3:
                line[x] = (line[x] + (left + up) // 2) & 0xFF
            elif kind == 4:
                estimate = left + up - corner
                distances = (abs(estimate - left), abs(estimate - up), abs(estimate - corner))
                predictor = left if distances[0] <= distances[1] and distances[0] <= distances[2] else up if distances[1] <= distances[2] else corner
                line[x] = (line[x] + predictor) & 0xFF
            elif kind != 0:
                raise ValueError(f"{path}: unknown PNG filter {kind}")
        pixels += line
        previous = line
    if channels == 4:
        pixels = bytearray(value for index, value in enumerate(pixels) if index % 4 != 3)
    return Image(width, height, pixels)


def compare(actual, baseline, tolerance=CHANNEL_TOLERANCE, max_fraction=MAX_DIFF_FRACTION):
    """Tolerance comparison; returns the report and a diff mask (None when sizes differ)."""
    if (actual.width, actual.height) != (baseline.width, baseline.height):
        return {"pass": False, "reason": f"size {actual.width}x{actual.height} differs from baseline {baseline.width}x{baseline.height}"}, None
    differing, largest, mask = 0, 0, bytearray(actual.width * actual.height)
    stride = actual.width * 3
    for y in range(actual.height):
        start = y * stride
        if actual.rgb[start:start + stride] == baseline.rgb[start:start + stride]:
            continue
        for index in range(y * actual.width, (y + 1) * actual.width):
            delta = max(abs(actual.rgb[index * 3 + channel] - baseline.rgb[index * 3 + channel]) for channel in range(3))
            largest = max(largest, delta)
            if delta > tolerance:
                differing += 1
                mask[index] = 1
    fraction = differing / (actual.width * actual.height)
    report = {"pass": fraction <= max_fraction, "differing_pixels": differing, "fraction": round(fraction, 6),
              "max_channel_delta": largest, "channel_tolerance": tolerance, "max_fraction": max_fraction}
    if not report["pass"]:
        report["reason"] = f"{fraction:.3%} of pixels differ by more than {tolerance} (limit {max_fraction:.3%})"
    return report, mask


def diff_image(baseline, mask):
    """The baseline dimmed, with differing pixels in magenta."""
    rgb = bytearray(value // 3 for value in baseline.rgb)
    for index, differs in enumerate(mask):
        if differs:
            rgb[index * 3:index * 3 + 3] = b"\xff\x00\xff"
    return Image(baseline.width, baseline.height, rgb)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_manifest(baselines):
    path = baselines / "manifest.json"
    if not path.is_file():
        return None
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or set(manifest.get("cells", {})) != set(CELLS):
        raise ValueError("Baseline manifest must list exactly the six cells")
    if not isinstance(manifest.get("reviewed_by"), str) or not manifest["reviewed_by"].strip():
        raise ValueError("Baseline manifest has no reviewer")
    for cell, row in manifest["cells"].items():
        if digest(baselines / f"{cell}.png") != row.get("sha256"):
            raise ValueError(f"Baseline {cell}.png differs from its reviewed manifest entry")
    return manifest


def compare_command(arguments):
    manifest = load_manifest(arguments.baselines)
    if manifest is None:
        message = f"No reviewed visual baselines in {arguments.baselines}; run tests/visual/regenerate.ps1 and review them."
        if arguments.allow_missing_baselines:
            print(f"::warning::{message}")
            return 0
        raise ValueError(message)
    tolerance = manifest.get("channel_tolerance", CHANNEL_TOLERANCE)
    max_fraction = manifest.get("max_fraction", MAX_DIFF_FRACTION)
    results, failed = {}, []
    if arguments.report:
        arguments.report.mkdir(parents=True, exist_ok=True)
    for cell in CELLS:
        actual_path = arguments.actual / f"{cell}.bmp"
        if not actual_path.is_file():
            results[cell] = {"pass": False, "reason": "capture missing"}
            failed.append(cell)
            continue
        baseline = read_png(arguments.baselines / f"{cell}.png")
        report, mask = compare(read_bmp(actual_path), baseline, tolerance, max_fraction)
        results[cell] = report
        if not report["pass"]:
            failed.append(cell)
            if arguments.report and mask is not None:
                write_png(arguments.report / f"{cell}-diff.png", diff_image(baseline, mask))
    if arguments.report:
        (arguments.report / "report.json").write_text(json.dumps({"schema_version": 1, "cells": results}, indent=2) + "\n", encoding="utf-8")
    for cell in CELLS:
        row = results[cell]
        print(f"{cell}: {'PASS' if row['pass'] else 'FAIL'} " + (row.get("reason") or f"{row['differing_pixels']} pixels differ"))
    return 1 if failed else 0


def promote_command(arguments):
    arguments.baselines.mkdir(parents=True, exist_ok=True)
    cells = {}
    for cell in CELLS:
        image = read_bmp(arguments.actual / f"{cell}.bmp")
        target = arguments.baselines / f"{cell}.png"
        write_png(target, image)
        cells[cell] = {"sha256": digest(target), "width": image.width, "height": image.height}
    manifest = {"schema_version": 1, "reviewed_by": arguments.reviewer, "channel_tolerance": CHANNEL_TOLERANCE,
                "max_fraction": MAX_DIFF_FRACTION, "source": arguments.source, "cells": cells}
    (arguments.baselines / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("Baselines written. Review every changed PNG before committing them; they are reviewed like code.")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="operation", required=True)
    check = sub.add_parser("compare")
    check.add_argument("--actual", type=Path, required=True)
    check.add_argument("--baselines", type=Path, default=BASELINES)
    check.add_argument("--report", type=Path)
    check.add_argument("--allow-missing-baselines", action="store_true",
                       help="warn instead of failing while no reviewed baselines are committed")
    promote = sub.add_parser("promote")
    promote.add_argument("--actual", type=Path, required=True)
    promote.add_argument("--baselines", type=Path, default=BASELINES)
    promote.add_argument("--reviewer", required=True)
    promote.add_argument("--source", default="tests/visual/capture.ps1 on windows-2022")
    arguments = parser.parse_args()
    try:
        return compare_command(arguments) if arguments.operation == "compare" else promote_command(arguments)
    except (ValueError, OSError, zlib.error, struct.error) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
