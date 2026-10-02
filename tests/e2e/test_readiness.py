# SPDX-License-Identifier: MPL-2.0
"""Readiness ledger consistency; synthetic summaries only, no editor is launched."""
import contextlib
import copy
import io
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

    def test_rendered_ledger_covers_every_journey_and_case(self):
        expected = readiness.render(self.data, self.manifest, journey_matrix.load_quarantine())
        self.assertEqual(readiness.check(), expected)
        for journey in self.manifest["journeys"]:
            self.assertIn(f"| `{journey['id']}` (", expected)
            for case in journey["cases"]:
                self.assertIn(case, expected)
        for case, text in self.data["cases"].items():
            self.assertIn(f"| {case} | {text} |", expected)

    def test_render_writes_only_to_the_requested_path(self):
        self.assertEqual(readiness.DEFAULT_LEDGER.relative_to(readiness.ROOT).parts[0], "target")
        out = self.root / "nested" / "READINESS.md"
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(readiness.main(["render", "--out", str(out)]), 0)
        self.assertEqual(out.read_text(encoding="utf-8"),
                         readiness.render(self.data, self.manifest, journey_matrix.load_quarantine()))
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            self.assertEqual(readiness.main(["render", "--out", "-"]), 0)
        self.assertEqual(stdout.getvalue(), out.read_text(encoding="utf-8"))

    def test_check_validates_the_inputs_without_a_committed_ledger(self):
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(readiness.main(["check"]), 0)
        quarantine = self.root / "quarantine.json"
        quarantine.write_text(json.dumps({"schema_version": 1, "quarantined": [
            {"journey": "no_such_journey", "reason": "synthetic entry for the test", "ticket": "QA-14",
             "since": "2026-10-02"}]}), encoding="utf-8")
        with self.assertRaises(ValueError):
            readiness.check(quarantine_path=quarantine)

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
