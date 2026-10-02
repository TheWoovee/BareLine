# SPDX-License-Identifier: MPL-2.0
"""Readiness ledger consistency; synthetic summaries only, no editor is launched."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import journey_matrix
import readiness


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.data, self.manifest = readiness.load()

    def test_committed_ledger_matches_its_inputs(self):
        expected = readiness.render(self.data, self.manifest, journey_matrix.load_quarantine())
        self.assertEqual(readiness.LEDGER.read_text(encoding="utf-8").replace("\r\n", "\n"), expected)
        for journey in self.manifest["journeys"]:
            self.assertIn(f"| `{journey['id']}` (", expected)
            for case in journey["cases"]:
                self.assertIn(case, expected)

    def test_every_mapped_case_has_text_and_every_journey_a_last_result(self):
        for change in (lambda data: data["cases"].pop("AC-004-01"), lambda data: data["cases"].update({"AC-099-01": "x"}),
                       lambda data: data["last_results"].pop("udl"),
                       lambda data: data["last_results"]["udl"].update(result="PASS"),
                       lambda data: data["last_results"]["plain_text"].update(commit="dc1bf238")):
            changed = copy.deepcopy(self.data)
            change(changed)
            path = self.root / "readiness.json"
            path.write_text(json.dumps(changed), encoding="utf-8")
            with self.assertRaises(ValueError):
                readiness.load(path)

    def test_record_takes_last_results_from_a_matrix_summary(self):
        passed, failed = ("PASS", "", None), ("FAIL", "s1: x", "environment")
        summary = journey_matrix.summarize({"udl": [passed, failed, passed], "portable": [passed] * 3})
        data = readiness.record(copy.deepcopy(self.data), json.loads(json.dumps(summary)), "a" * 40, "2026-10-02")
        self.assertEqual(data["last_results"]["udl"]["result"], "FLAKY")
        self.assertEqual(data["last_results"]["udl"]["attempts"], "2/3")
        self.assertIn("'environment': 1", data["last_results"]["udl"]["note"])
        self.assertEqual(data["last_results"]["portable"]["result"], "PASS")
        self.assertEqual(data["last_results"]["plain_text"], self.data["last_results"]["plain_text"])
        with self.assertRaises(ValueError):
            readiness.record(copy.deepcopy(self.data), summary, "short", "2026-10-02")


if __name__ == "__main__":
    unittest.main()
