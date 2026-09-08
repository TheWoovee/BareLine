# SPDX-License-Identifier: MPL-2.0
"""Source-only invariants; these tests never launch an application."""
import copy
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
import runner


class RunnerTests(unittest.TestCase):
    def test_empty_evidence_reports_missing_cases_and_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            report = Path(temporary) / "report.json"
            status = runner.resolve(SimpleNamespace(
                index=runner.ROOT / "docs/parity/index.json", inventory=None, output=report))
            data = runner.read_json(report)
            self.assertEqual(status, 1)
            self.assertFalse(data["evidence_complete"])
            self.assertIn("Missing runtime command inventory", data["unresolved"])
            self.assertIn("AC-021-01", data["unresolved"])

    def test_deleted_parity_family_rejected(self):
        data = runner.read_json(runner.ROOT / "docs/parity/index.json")
        data["features"].pop()
        with tempfile.TemporaryDirectory() as temporary:
            index = Path(temporary) / "index.json"
            runner.write_new(index, data)
            with self.assertRaisesRegex(ValueError, "silently omits"):
                runner.resolve(SimpleNamespace(index=index, inventory=None, output=None))

    def test_missing_observation_cannot_pass(self):
        journey = {"id": "fixture", "steps": [{"id": "a"}, {"id": "b"}]}
        response = {"schema_version": 1, "journey": "fixture", "steps": [
            {"id": "a", "status": "PASS", "observed": "value"}]}
        with self.assertRaises(ValueError):
            runner.observations(response, journey)
        response["steps"].append({"id": "b", "status": "NOT_RUN", "observed": "Unavailable"})
        self.assertEqual(runner.observations(response, journey), "FAIL")

    def test_duplicate_step_cannot_substitute_missing_step(self):
        journey = {"id": "fixture", "steps": [{"id": "a"}, {"id": "b"}]}
        step = {"id": "a", "status": "PASS", "observed": "value"}
        with self.assertRaises(ValueError):
            runner.observations({"schema_version": 1, "journey": "fixture",
                                 "steps": [step, copy.copy(step)]}, journey)

    def test_evidence_path_escape_rejected(self):
        with self.assertRaises(ValueError):
            runner.local_path("../outside-result.json")

    def test_duplicate_journey_rejected(self):
        data = runner.read_json(runner.ROOT / "tests/e2e/journeys.json")
        data["journeys"][-1] = copy.deepcopy(data["journeys"][0])
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            runner.write_new(path, data)
            with self.assertRaises(ValueError):
                runner.manifest(path)


if __name__ == "__main__":
    unittest.main()
