# SPDX-License-Identifier: MPL-2.0
"""Scheduled journey matrix summaries from synthetic results; no editor is launched."""
import json
from pathlib import Path
import tempfile
import unittest

import journey_matrix
import native_adapter
import runner


class JourneyMatrixTests(unittest.TestCase):
    def test_known_good_journeys_are_implemented_native_procedures(self):
        self.assertTrue(set(journey_matrix.KNOWN_GOOD) <= set(runner.JOURNEYS))
        self.assertTrue(set(journey_matrix.KNOWN_GOOD) <= native_adapter.IMPLEMENTED)

    def test_only_an_explicit_pass_result_counts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.assertEqual(journey_matrix.attempt_status(root / "missing")[0], "FAIL")
            (root / "result.json").write_text("{truncated", encoding="utf-8")
            self.assertEqual(journey_matrix.attempt_status(root), ("FAIL", "no readable result.json"))
            (root / "result.json").write_text(json.dumps({"status": "FAIL", "error": "Adapter deadline exceeded"}), encoding="utf-8")
            self.assertEqual(journey_matrix.attempt_status(root), ("FAIL", "Adapter deadline exceeded"))
            (root / "result.json").write_text(json.dumps({"status": "PASS"}), encoding="utf-8")
            self.assertEqual(journey_matrix.attempt_status(root), ("PASS", ""))

    def test_summary_separates_stable_flaky_and_failing_journeys(self):
        passed, failed = ("PASS", ""), ("FAIL", "step did not pass")
        summary = journey_matrix.summarize({
            "plain_text": [passed, passed, passed],
            "code_config": [passed, failed, passed],
            "regex_transform": [failed, failed, failed],
            "column_multi_cursor": [failed, passed, passed],
        })
        rows = summary["journeys"]
        self.assertEqual([rows[name]["classification"] for name in rows], ["pass", "flaky", "fail", "flaky"])
        self.assertEqual((rows["code_config"]["passed"], rows["code_config"]["failed"]), (2, 1))
        self.assertEqual(rows["regex_transform"]["failures"], ["step did not pass"] * 3)
        self.assertEqual((summary["flaky_journeys"], summary["flake_rate"]), (2, 0.5))
        self.assertEqual(summary["failing_journeys"], ["regex_transform"])
        self.assertEqual(journey_matrix.exit_status(summary), 1)
        report = journey_matrix.markdown(summary, "0123456789abcdef" * 2 + "01234567")
        self.assertIn("| code_config | 2/3 | 1 | flaky |", report)
        self.assertIn("flake rate 50%", report)

    def test_flaky_but_never_failing_journeys_do_not_fail_the_run(self):
        passed, failed = ("PASS", ""), ("FAIL", "x")
        summary = journey_matrix.summarize({"plain_text": [passed, failed, passed], "code_config": [passed] * 3})
        self.assertEqual(journey_matrix.exit_status(summary), 0)
        self.assertEqual(summary["flaky_journeys"], 1)


if __name__ == "__main__":
    unittest.main()
