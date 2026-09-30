# SPDX-License-Identifier: MPL-2.0
"""Scheduled journey matrix summaries from synthetic results; no editor is launched."""
import json
from pathlib import Path
import re
import tempfile
import unittest

import journey_matrix
import native_adapter
import runner

WORKFLOW = runner.ROOT / ".github/workflows/native-journeys.yml"


def result(status, error=None, steps=()):
    value = {"status": status, "steps": [dict(id=f"s{n}", status=s, observed=o) for n, (s, o) in enumerate(steps, 1)]}
    if error:
        value["error"] = error
    return value


class JourneyMatrixTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_scheduled_journeys_are_implemented_ordinary_procedures(self):
        ordinary = journey_matrix.ordinary_journeys()
        self.assertTrue(set(ordinary) <= native_adapter.IMPLEMENTED)
        self.assertFalse({"crash_recovery", "extension_isolation", "install_update_rollback"} & set(ordinary))
        # The workflow matrix lists exactly the ordinary tier, and never runs on pull requests.
        text = WORKFLOW.read_text(encoding="utf-8")
        listed = re.search(r"journey: \[([^\]]*)\]", text)
        self.assertEqual([name.strip() for name in listed[1].split(",")], ordinary)
        self.assertNotIn("pull_request", text)

    def test_only_an_explicit_pass_result_counts(self):
        self.assertEqual(journey_matrix.attempt_status(self.root / "missing"), ("FAIL", "no readable result.json", "harness"))
        (self.root / "result.json").write_text("{truncated", encoding="utf-8")
        self.assertEqual(journey_matrix.attempt_status(self.root)[0], "FAIL")
        (self.root / "result.json").write_text(json.dumps({"status": "FAIL", "error": "Adapter deadline exceeded"}), encoding="utf-8")
        self.assertEqual(journey_matrix.attempt_status(self.root), ("FAIL", "Adapter deadline exceeded", "harness_timeout"))
        (self.root / "result.json").write_text(json.dumps({"status": "PASS"}), encoding="utf-8")
        self.assertEqual(journey_matrix.attempt_status(self.root), ("PASS", "", None))

    def test_failures_are_classified_product_timeout_environment_or_harness(self):
        cases = {
            "product": result("FAIL", steps=[("PASS", "ok"), ("FAIL", "Exact text mismatch: one Undo"), ("NOT_RUN", "Prerequisite")]),
            "environment": result("FAIL", steps=[("FAIL", 'Exception calling "Guard" with "1" argument(s): "Lost foreground; automation stopped"')]),
            "harness_timeout": result("FAIL", steps=[("FAIL", "Native file dialog did not appear")]),
            "harness": result("FAIL", steps=[("FAIL", "Native command missing, ambiguous or disabled: Replace")]),
            "not_run": result("FAIL", steps=[("PASS", "ok"), ("NOT_RUN", "Only keyboard procedures are implemented")]),
        }
        for expected, value in cases.items():
            with self.subTest(expected=expected):
                self.assertEqual(journey_matrix.classify(value), expected)
        self.assertEqual(journey_matrix.classify(result("FAIL", "Adapter exited unsuccessfully with code 7")), "harness")
        self.assertEqual(journey_matrix.classify(result("FAIL", steps=[("FAIL", "Busy precondition not established: the save completed before Close")])), "harness")
        self.assertEqual(journey_matrix.classify(["not", "a", "result"]), "harness")
        self.assertIsNone(journey_matrix.classify(result("PASS")))

    def test_summary_separates_stable_flaky_failing_and_quarantined_journeys(self):
        passed, product, timeout = ("PASS", "", None), ("FAIL", "s2: mismatch", "product"), ("FAIL", "deadline", "harness_timeout")
        quarantine = {"udl": {"reason": "never passed natively yet", "ticket": "QA-03", "since": "2026-10-01"}}
        summary = journey_matrix.summarize({
            "plain_text": [passed] * 10,
            "code_config": [passed, product, passed, timeout, passed, passed, passed, passed, passed, passed],
            "regex_transform": [product] * 3,
            "udl": [timeout, product, timeout],
        }, quarantine)
        rows = summary["journeys"]
        self.assertEqual([rows[name]["classification"] for name in rows], ["pass", "flaky", "fail", "fail"])
        self.assertEqual(rows["code_config"]["failure_classes"], {"product": 1, "harness_timeout": 1, "environment": 0, "harness": 0, "not_run": 0})
        self.assertEqual((rows["code_config"]["passed"], rows["code_config"]["failure_rate"]), (8, 0.2))
        self.assertEqual((summary["flaky_journeys"], summary["flake_rate"]), (1, 0.25))
        self.assertEqual(summary["failing_journeys"], ["regex_transform"])
        self.assertEqual(summary["quarantined_failing_journeys"], ["udl"])
        self.assertEqual(journey_matrix.exit_status(summary), 1)
        report = journey_matrix.markdown(summary, "0123456789abcdef" * 2 + "01234567")
        self.assertIn("| code_config | 8/10 | 1 | 1 | 0 | 0 | 0 | 20% | flaky |", report)
        self.assertIn("| udl | 0/3 | 1 | 2 | 0 | 0 | 0 | 100% | fail | QA-03: never passed natively yet |", report)
        # A quarantined journey that never passes is a warning only.
        del rows["regex_transform"]
        summary["failing_journeys"] = []
        self.assertEqual(journey_matrix.exit_status(summary), 0)

    def test_quarantine_entries_need_reason_ticket_and_ordinary_journey(self):
        self.assertIn("ui_regressions", journey_matrix.load_quarantine())
        good = {"journey": "udl", "reason": "never passed natively yet", "ticket": "QA-03", "since": "2026-10-01"}
        path = self.root / "quarantine.json"
        path.write_text(json.dumps({"schema_version": 1, "quarantined": [good]}), encoding="utf-8")
        self.assertEqual(journey_matrix.load_quarantine(path), {"udl": {k: good[k] for k in ("reason", "ticket", "since")}})
        for change in ({"reason": ""}, {"ticket": "soon"}, {"journey": "crash_recovery"}, {"since": "01/10/2026"}, {"owner": "x"}):
            path.write_text(json.dumps({"schema_version": 1, "quarantined": [dict(good, **change)]}), encoding="utf-8")
            with self.subTest(change=change), self.assertRaises(ValueError):
                journey_matrix.load_quarantine(path)
        path.write_text(json.dumps({"schema_version": 1, "quarantined": [good, good]}), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            journey_matrix.load_quarantine(path)

    def test_collect_combines_matrix_outputs_in_attempt_order(self):
        for artifact, attempt, status in [("native-journey-udl", 10, "PASS"), ("native-journey-udl", 2, "FAIL"),
                                          ("native-journey-plain_text", 1, "PASS")]:
            journey = artifact.removeprefix("native-journey-")
            directory = self.root / artifact / f"{journey}-{attempt}"
            directory.mkdir(parents=True)
            (directory / "result.json").write_text(json.dumps(result(status, steps=[(status, "observed")])), encoding="utf-8")
        found = journey_matrix.collect([self.root], ["plain_text", "udl"])
        self.assertEqual([row[0] for row in found["udl"]], ["FAIL", "PASS"])
        self.assertEqual(found["plain_text"], [("PASS", "", None)])


if __name__ == "__main__":
    unittest.main()
