# SPDX-License-Identifier: MPL-2.0
"""Focused harness regressions; run only at the coordinated verification gate."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

import perf_suite as suite


class EvidenceTests(unittest.TestCase):
    def test_timeout_and_failing_child_retain_diagnostics(self):
        result = suite.capture([sys.executable, "-c", "import sys; print('receipt', flush=True); sys.exit(3)"], None, 2)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 3)
        self.assertIn("receipt", result["stdout"])
        result = suite.capture([sys.executable, "-c", "import time; print('started', flush=True); time.sleep(10)"], None, .1)
        self.assertEqual(result["status"], "timeout")
        self.assertIn("started", result["stdout"])

    def test_output_flood_is_bounded_and_does_not_deadlock(self):
        result = suite.capture([sys.executable, "-c", "import sys; sys.stderr.write('x'*2000000); sys.stderr.flush()"], None, 3)
        self.assertLessEqual(len(result["stderr"]), suite.LIMIT)
        self.assertNotEqual(result["status"], "timeout")

    def test_incomplete_pair_cannot_improve_reported_ratio(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            suite.write_new(root / "provenance.json", {"manifest": {"repetitions": 2, "series": "local", "scenarios": [{"name": "edit_to_paint"}]}, "missing_scenarios": []})
            for pair, app, position, value in [(0, "bareline", 0, 10), (0, "notepadpp", 1, 20), (1, "bareline", 1, 1)]:
                suite.write_new(root / f"edit_to_paint-{pair}-{app}.json", {"scenario": "edit_to_paint", "pair": pair, "application": app, "position": position, "metrics": {"latency_us": value}, "status": "ok"})
            suite.report(root, root / "report.out")
            report = suite.read_json(root / "report.out")
            self.assertEqual(report["rows"][0]["p50_ratio"], .5)
            self.assertEqual(report["rows"][0]["excluded_pairs"], [1])
            self.assertFalse(report["claims_eligible"])

    def test_invalid_marker_never_becomes_zero_measurement(self):
        for value in ({}, {"event": "measurement", "metrics": {"time": -1}}, {"event": "measurement", "metrics": {"time": True}}):
            with self.assertRaises(ValueError):
                suite.measurement(json.dumps(value))


if __name__ == "__main__":
    unittest.main()
