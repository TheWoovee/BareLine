# SPDX-License-Identifier: MPL-2.0
"""Repeat the known-good native journeys and report pass/fail and flakiness per journey.

Used by the scheduled native-journeys workflow on a disposable Windows runner.
Each attempt is an ordinary `runner.py run` with its own fresh output directory;
this script only schedules attempts and summarizes their result.json files.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

HERE = Path(__file__).resolve().parent
# The journeys that have passed natively on released builds (QA-03).
KNOWN_GOOD = ("plain_text", "code_config", "regex_transform", "column_multi_cursor")
# Runner deadline is 300 s per journey; allow process startup and cleanup.
ATTEMPT_TIMEOUT_SECONDS = 420


def attempt_status(directory):
    """PASS only for a runner result.json that says PASS; anything else is a failure."""
    try:
        result = json.loads((directory / "result.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return "FAIL", "no readable result.json"
    status = result.get("status") if isinstance(result, dict) else None
    if status == "PASS":
        return "PASS", ""
    return "FAIL", str(result.get("error") or "one or more steps did not pass")[:300]


def summarize(results):
    """results: {journey: [(status, detail), ...]} in attempt order."""
    journeys = {}
    for journey, attempts in results.items():
        passed = sum(status == "PASS" for status, _ in attempts)
        failed = len(attempts) - passed
        classification = "pass" if failed == 0 else "fail" if passed == 0 else "flaky"
        journeys[journey] = {"attempts": len(attempts), "passed": passed, "failed": failed,
                             "classification": classification,
                             "failures": [detail for status, detail in attempts if status != "PASS"]}
    flaky = sum(row["classification"] == "flaky" for row in journeys.values())
    return {"schema_version": 1, "journeys": journeys,
            "flaky_journeys": flaky, "flake_rate": round(flaky / len(journeys), 3) if journeys else 0.0,
            "failing_journeys": sorted(name for name, row in journeys.items() if row["classification"] == "fail")}


def markdown(summary, commit):
    lines = [f"### Native journeys on `{commit[:12]}`", "",
             "| Journey | Passed | Failed | Result |", "| --- | ---: | ---: | --- |"]
    for journey, row in summary["journeys"].items():
        lines.append(f"| {journey} | {row['passed']}/{row['attempts']} | {row['failed']} | {row['classification']} |")
    lines += ["", f"Flaky journeys: {summary['flaky_journeys']} of {len(summary['journeys'])} "
                  f"(flake rate {summary['flake_rate']:.0%}). A journey that never passes fails the run; "
                  "a flaky journey is reported as a warning.", ""]
    return "\n".join(lines)


def exit_status(summary):
    return 1 if summary["failing_journeys"] else 0


def run(arguments):
    executable = arguments.executable.resolve(strict=True)
    output = arguments.output.absolute()
    output.mkdir(parents=True)
    adapter = [sys.executable, str(HERE / "native_adapter.py")]
    results = {journey: [] for journey in arguments.journeys}
    for attempt in range(1, arguments.attempts + 1):
        for journey in arguments.journeys:
            directory = output / f"{journey}-{attempt}"
            argv = [sys.executable, str(HERE / "runner.py"), "run", journey, "--output", str(directory),
                    "--commit", arguments.commit, "--reviewer", "scheduled-ci",
                    "--os-build", platform.platform(), "--hardware", arguments.hardware,
                    "--executable", str(executable), "--dpi", "100", "--mode", "keyboard",
                    "--theme", "dark", "--adapter", *adapter]
            try:
                completed = subprocess.run(argv, cwd=HERE.parents[1], capture_output=True, text=True,
                                           timeout=ATTEMPT_TIMEOUT_SECONDS)
                (output / f"{journey}-{attempt}.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
                results[journey].append(attempt_status(directory))
            except subprocess.TimeoutExpired:
                results[journey].append(("FAIL", f"runner exceeded {ATTEMPT_TIMEOUT_SECONDS} s"))
    summary = summarize(results)
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    report = markdown(summary, arguments.commit)
    print(report)
    if arguments.summary:
        with open(arguments.summary, "a", encoding="utf-8") as stream:
            stream.write(report + "\n")
    for journey, row in summary["journeys"].items():
        if row["classification"] == "flaky":
            print(f"::warning::Native journey {journey} is flaky: {row['passed']}/{row['attempts']} attempts passed.")
    return exit_status(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    execute = sub.add_parser("run")
    execute.add_argument("--executable", type=Path, required=True)
    execute.add_argument("--commit", required=True)
    execute.add_argument("--output", type=Path, required=True)
    execute.add_argument("--attempts", type=int, default=3, choices=range(1, 11))
    execute.add_argument("--journeys", nargs="+", default=list(KNOWN_GOOD), choices=KNOWN_GOOD)
    execute.add_argument("--hardware", default=os.environ.get("RUNNER_NAME", "local") + " (GitHub-hosted windows-2022)")
    execute.add_argument("--summary", help="Markdown file to append the report to, e.g. $GITHUB_STEP_SUMMARY")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
