# SPDX-License-Identifier: MPL-2.0
"""Source-only invariants; these tests never launch an application."""
import copy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
import runner


class RunnerTests(unittest.TestCase):
    def _write_receipt(self, root, name, raw, identity, command=None):
        stdout = root / f"{name}.stdout.log"
        stderr = root / f"{name}.stderr.log"
        stdout.write_text(raw, encoding="utf-8")
        stderr.write_bytes(b"")
        receipt = root / f"{name}.json"
        document = {
            "schema_version": 1, "status": "completed", "exit_code": 0,
            "top_level_total": 1, "top_level_passed": 1, "top_level_failed": 0,
            "summary_lines_are_not_aggregate_counts": True,
            "source_changed_during_run": False,
            "source_before": identity, "source_after": copy.deepcopy(identity),
            "command": command or ["synthetic-parser-fixture"],
            "stdout": {"path": str(stdout), "sha256": runner.digest(stdout)},
            "stderr": {"path": str(stderr), "sha256": runner.digest(stderr)},
        }
        runner.write_new(receipt, document)
        return receipt

    def _qualification_fixture(self, root):
        binary = root / "bareline.exe"
        binary.write_bytes(b"synthetic parser-only binary")
        fixture = root / "column-input.txt"
        fixture.write_text("a\tb\n界\n", encoding="utf-8")
        manifest = runner.manifest(runner.ROOT / "tests/e2e/journeys.json")
        journey = next(row for row in manifest["journeys"]
                       if row["id"] == "column_multi_cursor")
        result = root / "result.json"
        runner.write_new(result, {
            "schema_version": 1, "journey": journey["id"], "commit": "a" * 40,
            "reviewer": "reviewer", "status": "PASS", "adapter": ["fixture"],
            "request": {"executable": str(binary), "binary_sha256": runner.digest(binary),
                        "os_build": "synthetic-os", "hardware": "synthetic-hardware"},
            "steps": [{"id": step["id"], "status": "PASS",
                       "observed": "synthetic parser observation " + step["id"]}
                      for step in journey["steps"]],
        })
        binding = json.dumps({"evidence_result": str(result.resolve()),
                              "sha256": runner.digest(result)}, separators=(",", ":")) + "\n"
        identity = {"available": True, "head": "a" * 40, "working_tree_dirty": True,
                    "source_manifest_sha256": "b" * 64}
        receipt = self._write_receipt(root, "journey-receipt", binding, identity)
        bundle = root / "bundle.json"
        args = SimpleNamespace(journey_result=result, test_receipt=receipt,
                               output=bundle, journey=None,
                               evidence_id=["AC-006-01"], implementer="implementer",
                               reviewer="reviewer", os_build=None, hardware=None,
                               fixture=[fixture])
        return args, identity, binary

    def test_current_ledger_covers_all_families_with_actionable_metadata(self):
        data = runner.read_json(runner.ROOT / "docs/parity/index.json")
        families = {row["id"] for row in data["features"]}
        ledger = data["current_ledger"]
        self.assertEqual({row["id"] for row in ledger}, families)
        self.assertEqual(len(ledger), len(families))
        for row in ledger:
            self.assertIn(row["state"], runner.CURRENT_STATES)
            for field in ("owner", "next_step", "code", "tests", "limitation",
                          "source_binary_identity"):
                self.assertTrue(row[field].strip(), f'{row["id"]}: {field}')

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

    def test_t09_receipt_rejects_nested_counts_as_top_level(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdout = root / "stdout.log"
            stderr = root / "stderr.log"
            stdout.write_text("test result: ok. 1 passed\n", encoding="utf-8")
            stderr.write_bytes(b"")
            identity = {"available": True, "head": "a" * 40,
                        "working_tree_dirty": True,
                        "source_manifest_sha256": "b" * 64}
            receipt = root / "receipt.json"
            runner.write_new(receipt, {
                "schema_version": 1, "status": "completed", "exit_code": 0,
                "top_level_total": 2, "top_level_passed": 2, "top_level_failed": 0,
                "summary_lines_are_not_aggregate_counts": True,
                "source_changed_during_run": False,
                "source_before": identity, "source_after": identity,
                "stdout": {"path": str(stdout), "sha256": runner.digest(stdout)},
                "stderr": {"path": str(stderr), "sha256": runner.digest(stderr)},
            })
            with self.assertRaisesRegex(ValueError, "one successful top-level"):
                runner.checked_test_receipt(receipt)

    def test_xtask_journey_identity_is_checked_without_counting_fixture_summaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "bareline.exe"
            executable.write_bytes(b"actual binary")
            result_path = root / "journey.json"
            runner.write_new(result_path, {
                "identity": {"schema_version": 1, "run_id": "run-1", "retry_of": None,
                             "head": "a" * 40, "working_tree_dirty": True,
                             "source_manifest_sha256": "b" * 64,
                             "executable": str(executable),
                             "executable_sha256": runner.digest(executable)},
                "top_level_total": 1, "top_level_completed": 1,
                "top_level_passed": 1, "top_level_failed": 0,
                "results": [{"name": "p0-3", "status": "passed"}],
            })
            result, binary, observed, identity = runner.checked_journey_result(
                result_path, "p0-3")
            self.assertEqual(result["commit"], "a" * 40)
            self.assertEqual(binary, runner.digest(executable))
            self.assertIn('"status": "passed"', observed)
            self.assertEqual(identity["source_manifest_sha256"], "b" * 64)

    def test_checked_adapter_and_resolver_accept_only_the_reviewed_mapping(self):
        (runner.ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=runner.ROOT / "target") as temporary:
            root = Path(temporary)
            args, identity, binary = self._qualification_fixture(root)
            self.assertEqual(runner.adapt(args), 0)
            inventory = root / "inventory.json"
            runner.write_new(inventory, {"schema_version": 2, "commit": "a" * 40,
                                         "binary_sha256": runner.digest(binary),
                                         "commands": ["edit.column"]})
            raw = (f"command inventory published={inventory.resolve()}\n"
                   f"command inventory sha256={runner.digest(inventory)}\n")
            inventory_receipt = self._write_receipt(
                root, "inventory-receipt", raw, identity, [str(binary)])
            report = root / "report.json"
            status = runner.resolve(SimpleNamespace(
                index=runner.ROOT / "docs/parity/index.json", inventory=inventory,
                inventory_receipt=inventory_receipt, evidence=[args.output], output=report))
            self.assertEqual(status, 1)
            self.assertEqual(runner.read_json(report)["resolved"]["AC-006-01"], "PASS")

    def test_adapter_rejects_unrelated_case_and_altered_result(self):
        (runner.ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=runner.ROOT / "target") as temporary:
            root = Path(temporary)
            args, _, _ = self._qualification_fixture(root)
            args.evidence_id = ["AC-005-01"]
            with self.assertRaisesRegex(ValueError, "No reviewed full-result mapping"):
                runner.adapt(args)
            args.evidence_id = ["AC-006-01"]
            args.fixture = []
            with self.assertRaisesRegex(ValueError, "actual generated fixture"):
                runner.adapt(args)
            args.fixture = [root / "column-input.txt"]
            result = runner.read_json(args.journey_result)
            result["extra"] = "mutated after capture"
            args.journey_result.write_text(json.dumps(result), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "does not bind"):
                runner.adapt(args)

    def test_resolver_rejects_failed_result_claimed_as_pass_and_binary_mismatch(self):
        (runner.ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=runner.ROOT / "target") as temporary:
            root = Path(temporary)
            args, _, _ = self._qualification_fixture(root)
            runner.adapt(args)
            bundle = runner.read_json(args.output)
            mismatch = root / "binary-mismatch.json"
            changed = copy.deepcopy(bundle)
            changed["evidence"][0]["binary_sha256"] = "c" * 64
            runner.write_new(mismatch, changed)
            with self.assertRaisesRegex(ValueError, "revalidated journey"):
                runner.resolve(SimpleNamespace(index=runner.ROOT / "docs/parity/index.json",
                                                inventory=None, evidence=[mismatch], output=None))

            result = runner.read_json(args.journey_result)
            result["status"] = "FAIL"
            args.journey_result.write_text(json.dumps(result), encoding="utf-8")
            stdout = Path(runner.read_json(args.test_receipt)["stdout"]["path"])
            stdout.write_text(json.dumps({"evidence_result": str(args.journey_result.resolve()),
                                          "sha256": runner.digest(args.journey_result)}) + "\n",
                              encoding="utf-8")
            receipt = runner.read_json(args.test_receipt)
            receipt["stdout"]["sha256"] = runner.digest(stdout)
            args.test_receipt.write_text(json.dumps(receipt), encoding="utf-8")
            forged = copy.deepcopy(bundle)
            forged["evidence"][0]["result_sha256"] = runner.digest(args.journey_result)
            forged["evidence"][0]["test_receipt_sha256"] = runner.digest(args.test_receipt)
            forged_path = root / "failed-as-pass.json"
            runner.write_new(forged_path, forged)
            with self.assertRaisesRegex(ValueError, "failed or unexecuted"):
                runner.resolve(SimpleNamespace(index=runner.ROOT / "docs/parity/index.json",
                                                inventory=None, evidence=[forged_path], output=None))

    def test_resolver_rejects_inventory_mutation_after_capture(self):
        (runner.ROOT / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=runner.ROOT / "target") as temporary:
            root = Path(temporary)
            _, identity, binary = self._qualification_fixture(root)
            inventory = root / "inventory.json"
            runner.write_new(inventory, {"schema_version": 2, "commit": "a" * 40,
                                         "binary_sha256": runner.digest(binary),
                                         "commands": ["file.open"]})
            raw = (f"command inventory published={inventory.resolve()}\n"
                   f"command inventory sha256={runner.digest(inventory)}\n")
            receipt = self._write_receipt(root, "inventory-receipt", raw, identity)
            document = runner.read_json(inventory)
            document["commands"].append("file.save")
            inventory.write_text(json.dumps(document), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "does not bind"):
                runner.resolve(SimpleNamespace(index=runner.ROOT / "docs/parity/index.json",
                                                inventory=inventory,
                                                inventory_receipt=receipt,
                                                evidence=[], output=None))


if __name__ == "__main__":
    unittest.main()
