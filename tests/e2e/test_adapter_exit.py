# SPDX-License-Identifier: MPL-2.0
"""Focused adapter completion regressions; no editor or desktop is launched."""
import copy
from contextlib import redirect_stdout
import ctypes as C
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import runner
from windows_process_metrics import OwnedProcessTree
from ctypes import wintypes as W


class ProcessExitTests(unittest.TestCase):
    def tree(self, waited=0, code=0):
        tree = OwnedProcessTree.__new__(OwnedProcessTree)
        tree.process = 123
        tree.api = Mock()
        tree.api.wait.return_value = waited

        def require(value):
            if not value:
                raise OSError("synthetic Win32 failure")
            return value

        def exit_code(handle, output):
            C.cast(output, C.POINTER(W.DWORD))[0] = code
            return True

        tree.api.require.side_effect = require
        tree.api.exit_code.side_effect = exit_code
        return tree

    def test_running_is_not_success_and_does_not_query_exit_code(self):
        tree = self.tree(waited=258)
        self.assertIsNone(tree.poll_exit_code())
        tree.api.exit_code.assert_not_called()

    def test_terminal_codes_preserve_failure_and_still_active_value(self):
        for code in (0, 7, 259, 0xC0000005, 0xFFFFFFFF):
            with self.subTest(code=code):
                self.assertEqual(self.tree(code=code).poll_exit_code(), code)

    def test_wait_failure_cannot_look_like_completion(self):
        tree = self.tree(waited=0xFFFFFFFF)
        with self.assertRaises(OSError):
            tree.poll_exit_code()
        tree.api.exit_code.assert_not_called()

    def test_unexpected_wait_cannot_look_like_completion(self):
        tree = self.tree(waited=128)
        with self.assertRaisesRegex(OSError, "Unexpected process wait"):
            tree.poll_exit_code()
        tree.api.exit_code.assert_not_called()

    def test_exit_query_failure_cannot_look_like_success(self):
        tree = self.tree()
        tree.api.exit_code.side_effect = lambda *_: False
        with self.assertRaises(OSError):
            tree.poll_exit_code()

    def test_closed_handle_has_no_terminal_status(self):
        tree = self.tree()
        tree.process = None
        with self.assertRaisesRegex(OSError, "handle is closed"):
            tree.poll_exit_code()
        tree.api.wait.assert_not_called()


class AdapterRunTests(unittest.TestCase):
    def setUp(self):
        self.journey = {"id": "plain_text", "timeout_seconds": 1,
                        "steps": [{"id": "first"}, {"id": "second"}]}
        self.response = {"schema_version": 1, "journey": "plain_text", "steps": [
            {"id": step["id"], "status": "PASS", "observed": "synthetic fixture"}
            for step in self.journey["steps"]]}

    def run_fixture(self, response, polls=(0,), clock=(0, 0)):
        child = Mock()
        child.poll_exit_code.side_effect = polls
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(
                manifest=root / "manifest.json", journey="plain_text", commit="a" * 40,
                reviewer="synthetic-only", os_build="synthetic", hardware="synthetic",
                executable=sys.executable, adapter=[str(Path(sys.executable).resolve())],
                mode="keyboard", theme="light", dpi=100)

            def launch(argv, cwd):
                self.assertEqual(argv[:-1], args.adapter)
                self.assertEqual(Path(argv[-1]), cwd / "request.json")
                request = runner.read_json(Path(argv[-1]))
                self.assertEqual(request["binary_sha256"], runner.digest(Path(sys.executable)))
                if response is not None:
                    payload = response if isinstance(response, str) else json.dumps(response)
                    Path(request["response"]).write_text(payload, encoding="utf-8")
                return child

            output = io.StringIO()
            with patch.object(runner, "ROOT", root), \
                    patch.object(runner, "os", SimpleNamespace(name="nt")), \
                    patch.object(runner, "manifest", return_value={"journeys": [self.journey]}), \
                    patch("windows_process_metrics.OwnedProcessTree", side_effect=launch), \
                    patch.object(runner.time, "monotonic", side_effect=clock), \
                    patch.object(runner.time, "sleep"), redirect_stdout(output):
                status = runner.run(args)
            child.close.assert_called_once_with()
            binding = json.loads(output.getvalue())
            path = Path(binding["evidence_result"])
            self.assertEqual(binding["sha256"], runner.digest(path))
            result = runner.read_json(path)
            self.assertEqual(result["adapter"], args.adapter)
            self.assertEqual(result["request"], runner.read_json(path.parent / "request.json"))
            return status, result

    def test_success_requires_terminal_zero_and_complete_response(self):
        status, result = self.run_fixture(self.response, polls=(None, 0))
        self.assertEqual((status, result["status"], result["adapter_exit_code"]), (0, "PASS", 0))
        self.assertEqual(result["steps"], self.response["steps"])

    def test_pass_response_cannot_hide_exit_failure_or_crash_status(self):
        for code in (7, 259, 0xC0000005):
            with self.subTest(code=code):
                status, result = self.run_fixture(self.response, polls=(code,))
                self.assertEqual((status, result["status"]), (1, "FAIL"))
                self.assertEqual(result["adapter_exit_code"], code)
                self.assertIn(str(code), result["error"])

    def test_zero_exit_still_rejects_absent_malformed_or_oversized_response(self):
        for response in (None, "{malformed", "x" * (256 * 1024 + 1)):
            with self.subTest(kind="missing" if response is None else len(response)):
                status, result = self.run_fixture(response)
                self.assertEqual((status, result["status"], result["adapter_exit_code"]),
                                 (1, "FAIL", 0))
                self.assertTrue(result["error"])

    def test_running_adapter_response_remains_bounded(self):
        status, result = self.run_fixture("x" * (256 * 1024 + 1), polls=(None,))
        self.assertEqual((status, result["status"]), (1, "FAIL"))
        self.assertIsNone(result["adapter_exit_code"])
        self.assertIn("exceeds 256 KiB", result["error"])

    def test_partial_failed_or_unexecuted_steps_cannot_pass(self):
        partial = copy.deepcopy(self.response)
        partial["steps"].pop()
        failed = copy.deepcopy(self.response)
        failed["steps"][0]["status"] = "FAIL"
        unexecuted = copy.deepcopy(self.response)
        unexecuted["steps"][0]["status"] = "NOT_RUN"
        for response in (partial, failed, unexecuted):
            with self.subTest(response=response):
                status, result = self.run_fixture(response)
                self.assertEqual((status, result["status"]), (1, "FAIL"))

    def test_timeout_cannot_accept_a_pass_response(self):
        status, result = self.run_fixture(self.response, polls=(None,), clock=(0, 2))
        self.assertEqual((status, result["status"]), (1, "FAIL"))
        self.assertIsNone(result["adapter_exit_code"])
        self.assertIn("deadline exceeded", result["error"])

    def test_unavailable_terminal_status_cannot_accept_a_pass_response(self):
        status, result = self.run_fixture(self.response, polls=(OSError("status query failed"),))
        self.assertEqual((status, result["status"]), (1, "FAIL"))
        self.assertIsNone(result["adapter_exit_code"])
        self.assertEqual(result["error"], "status query failed")


if __name__ == "__main__":
    unittest.main()
