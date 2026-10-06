# SPDX-License-Identifier: MPL-2.0
"""Port smoke oracles: log and exit verdicts everywhere, a fake editor on POSIX."""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
import textwrap
import unittest

import port_smoke as smoke

CLEAN_STDOUT = '{"event":"first_frame","microseconds":48213,"software":true,"version":"0.2.0"}\n'
CLEAN_STDERR = "event=accessibility_unavailable reason=no_platform_adapter\nevent=startup_phase phase=ready\n"


class VerdictTests(unittest.TestCase):
    def test_clean_logs_pass_and_report_the_first_frame(self):
        self.assertEqual(smoke.log_problems(CLEAN_STDOUT, CLEAN_STDERR + "event=operation_failed details=x\n"), [])
        self.assertEqual(smoke.first_frame_us(CLEAN_STDOUT), 48213)
        self.assertIsNone(smoke.first_frame_us("event=startup_phase phase=ready\n"))

    def test_panics_and_failed_startups_are_reported(self):
        panic_hook = '{"event":"panic","version":"0.2.0","file":"apps/bareline/src/shell.rs","line":1,"column":1}'
        default_hook = "thread 'main' panicked at apps/bareline/src/main.rs:3:5:"
        failed = "event=startup_failed error=no display"
        problems = smoke.log_problems(CLEAN_STDOUT + panic_hook + "\n", "\n".join([default_hook, failed]))
        self.assertEqual(len(problems), 3)
        self.assertTrue(problems[0].startswith("stdout: ") and problems[1].startswith("stderr: thread"))

    def test_only_the_requested_quit_counts_as_clean(self):
        self.assertEqual(smoke.exit_problems(False, -smoke.SIGTERM, False), [])
        self.assertEqual(smoke.exit_problems(False, 0, False), [])
        self.assertIn("signal 6", smoke.exit_problems(False, -6, False)[0])
        self.assertIn("killed", smoke.exit_problems(False, -smoke.SIGKILL, True)[0])
        # Ending before the quit request is a failure even with exit code 0.
        self.assertIn("by itself (exit code 0)", smoke.exit_problems(True, 0, False)[0])

    def test_resident_set_and_environment(self):
        self.assertEqual(smoke.parse_rss_kib(" 81234\n"), 81234)
        self.assertIsNone(smoke.parse_rss_kib(""))
        linux = smoke.build_environment("Linux", "x86_64", "Ubuntu 24.04.3 LTS, kernel 6.8", 4, "github-hosted runner")
        self.assertEqual((linux["os_family"], linux["architecture"]), ("linux", "x64"))
        mac = smoke.build_environment("Darwin", "arm64", "macOS 15.6", 3, "github-hosted runner")
        self.assertEqual((mac["os_family"], mac["architecture"]), ("macos", "arm64"))
        with self.assertRaises(ValueError):
            smoke.build_environment("Linux", "riscv64", "Linux", 4, "local machine")


# A stand-in editor: prints the first-frame event, then behaves as `MODE` says.
FAKE_EDITOR = textwrap.dedent("""\
    import os, signal, sys, time
    mode = os.environ["FAKE_MODE"]
    print('{"event":"first_frame","microseconds":1200,"software":true,"version":"0"}', flush=True)
    print("event=startup_phase phase=ready", file=sys.stderr, flush=True)
    if mode == "panic":
        print('{"event":"panic","version":"0","file":"x.rs","line":1,"column":1}', file=sys.stderr, flush=True)
        # An abnormal exit without SIGABRT: macOS shows a crash-reporter dialog
        # for aborted processes, and it would sit on top of the smoke screenshot.
        os._exit(101)
    if mode == "stubborn":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(60)
""")


@unittest.skipUnless(os.name == "posix", "needs POSIX signals and process groups; runs in the port smoke workflow")
class FakeEditorTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bareline-port-smoke-")
        self.root = Path(self.temporary.name)
        editor = self.root / "editor"
        editor.write_text(f"#!{sys.executable}\n" + FAKE_EDITOR, encoding="utf-8")
        editor.chmod(0o755)
        self.sample = self.root / "sample.rs"
        self.sample.write_text("fn main() {}\n", encoding="utf-8")
        self.options = argparse.Namespace(label="fake", executable=editor, sample=self.sample,
                                          evidence=self.root / "evidence", profile=self.root / "profile",
                                          timeout=5.0, settle=0.1, grace=1.0, alive_seconds=0.2,
                                          require_window=True, require_screenshot=True)

    def tearDown(self):
        os.environ.pop("FAKE_MODE", None)
        self.temporary.cleanup()

    def run_fake(self, mode, finders=None):
        os.environ["FAKE_MODE"] = mode
        polls = []
        stdout_log = Path(self.options.evidence) / f"{self.options.label}-editor.stdout.log"

        def window_after_three_polls(pid):
            # Synchronise on the fake editor's state, not on time: the window
            # "appears" only once the editor has logged its first frame, so a
            # slow interpreter start-up cannot make SIGTERM arrive first.
            polls.append(pid)
            try:
                logged = "first_frame" in stdout_log.read_text(encoding="utf-8", errors="replace")
            except OSError:
                logged = False
            return ["4242"] if logged and len(polls) >= 3 else []

        def screenshot(_system, path):
            path.write_bytes(b"png")
            return {"path": path.name, "ok": True, "error": None}

        finders = [("fake", window_after_three_polls)] if finders is None else finders
        return smoke.smoke(self.options, finders, screenshot=screenshot, system="Linux")

    def test_a_window_that_appears_and_quits_on_sigterm_passes(self):
        record = self.run_fake("well-behaved")
        self.assertTrue(record["passed"], record["problems"])
        self.assertEqual(record["window"]["ids"], ["4242"])
        self.assertGreater(record["window"]["launch_to_window_ms"], 0)
        self.assertGreater(record["memory"]["rss_kib"], 0)
        self.assertEqual((record["exit"]["returncode"], record["exit"]["killed"]), (-smoke.SIGTERM, False))
        self.assertEqual(record["editor"]["first_frame_us"], 1200)
        self.assertTrue(record["editor"]["startup_ready"])
        # The editor's data stays inside the scratch profile.
        self.assertTrue((self.root / "profile/config").is_dir())
        json.dumps(record)

    def test_a_crash_fails_with_the_panic_line(self):
        record = self.run_fake("panic")
        self.assertFalse(record["passed"])
        self.assertTrue(record["exit"]["exited_before_quit"])
        self.assertTrue(any('"event":"panic"' in problem for problem in record["problems"]))

    def test_ignoring_sigterm_is_killed_and_fails(self):
        record = self.run_fake("stubborn")
        self.assertTrue(record["exit"]["killed"])
        self.assertIn("killed", " ".join(record["problems"]))

    def test_a_window_that_never_appears_fails_and_unusable_discovery_is_reported(self):
        self.options.timeout = 0.3
        record = self.run_fake("well-behaved", [("fake", lambda pid: [])])
        self.assertIn("no window of the editor appeared", " ".join(record["problems"]))

        def unavailable(pid):
            raise smoke.Unavailable("no display")

        record = self.run_fake("well-behaved", [("fake", unavailable)])
        self.assertEqual((record["window"]["method"], record["window"]["found"]), ("process-alive", None))
        self.assertEqual(record["problems"], ["no window discovery method worked in this session"])
        self.options.require_window = False
        self.assertTrue(self.run_fake("well-behaved", [("fake", unavailable)])["passed"])


if __name__ == "__main__":
    unittest.main()
