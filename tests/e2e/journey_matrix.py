# SPDX-License-Identifier: MPL-2.0
"""Repeat native journeys N times; classify failures and report flakiness per journey.

Used by the scheduled native-journeys workflow on disposable Windows runners.
Each attempt is an ordinary `runner.py run` with its own fresh output directory;
this script only schedules attempts, classifies their result.json files and
summarizes them. Only "ordinary" journeys are scheduled; "vm-only" journeys need
a disposable VM and --lab-config (see WINDOWS-LAB.md).

Failure classes, most specific first:
  environment      the desktop, foreground, keyboard layout, DPI or high-contrast
                   state was not the requested cell; nothing is known about the product
  harness_timeout  a runner, adapter or driver deadline expired before an
                   expected state was observed
  harness          the harness itself failed: no result, a crashed adapter, a
                   changed selector or fixture, or an unmet test precondition
  product          a step observed product behavior that differs from its oracle
  not_run          no step failed, but a required step was not run
Quarantined journeys still run and are reported; they cannot fail the run.
"""
import argparse
from datetime import date
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys

import runner

HERE = Path(__file__).resolve().parent
MANIFEST = HERE / "journeys.json"
QUARANTINE = HERE / "quarantine.json"
# The runner applies each journey's own deadline; allow process startup and cleanup.
ATTEMPT_GRACE_SECONDS = 120
CLASSES = ("product", "harness_timeout", "environment", "harness", "not_run")
ENVIRONMENT = re.compile(r"Lost foreground|foreground changed|Input desktop|desktop is not Default|Physical Escape"
                         r"|High contrast is active|keyboard layout|en-US layout|window DPI differs"
                         r"|require Windows|unlocked Windows desktop", re.I)
TIMEOUT = re.compile(r"deadline exceeded|timed out|runner exceeded|did not appear|not published within"
                     r"|did not dismiss|not ready before the startup deadline|traversal exceeded", re.I)
HARNESS = re.compile(r"no readable result\.json|exited unsuccessfully|Adapter response|Adapter sources changed"
                     r"|Native (?:schema|driver binary|fixture configuration|artifacts?|observations artifact)"
                     r"|Request journey differs|Pinned executable|Unable to find type|null-valued expression"
                     r"|Native command missing, ambiguous or disabled|precondition not established", re.I)
TICKET = re.compile(r"[A-Z][A-Z0-9]*-[0-9]+|#[0-9]+|https://github\.com/[\w.-]+/[\w.-]+/issues/[0-9]+")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def ordinary_journeys(manifest_path=MANIFEST):
    return runner.tier(runner.manifest(manifest_path), "ordinary")


def load_quarantine(path=QUARANTINE, manifest_path=MANIFEST):
    """{journey: {reason, ticket, since}}; every entry needs a reason and a ticket."""
    data = runner.read_json(path)
    require(runner.versioned(data) and isinstance(data.get("quarantined"), list), "Unsupported quarantine schema")
    allowed = set(ordinary_journeys(manifest_path))
    entries = {}
    for row in data["quarantined"]:
        require(isinstance(row, dict) and set(row) == {"journey", "reason", "ticket", "since"},
                "Quarantine entries need exactly journey, reason, ticket and since")
        require(row["journey"] in allowed, f"Quarantined journey is not an ordinary journey: {row['journey']}")
        require(row["journey"] not in entries, f"Duplicate quarantine entry: {row['journey']}")
        require(isinstance(row["reason"], str) and 10 <= len(row["reason"].strip()) <= 300,
                "Quarantine reason must say why (10..300 characters)")
        require(isinstance(row["ticket"], str) and TICKET.fullmatch(row["ticket"]),
                "Quarantine ticket must be a finding ID, #issue or GitHub issue URL")
        require(isinstance(row["since"], str) and date.fromisoformat(row["since"]).isoformat() == row["since"],
                "Quarantine since must be an ISO date")
        entries[row["journey"]] = {key: row[key] for key in ("reason", "ticket", "since")}
    return entries


def classify(result):
    """Failure class of one runner result.json, or None when it passed."""
    if not isinstance(result, dict):
        return "harness"
    if result.get("status") == "PASS":
        return None
    steps = result.get("steps") if isinstance(result.get("steps"), list) else []
    failed = [step for step in steps if isinstance(step, dict) and step.get("status") == "FAIL"]
    text = "\n".join([str(result.get("error") or "")] + [str(step.get("observed", "")) for step in failed])
    if ENVIRONMENT.search(text):
        return "environment"
    if TIMEOUT.search(text):
        return "harness_timeout"
    if HARNESS.search(text) or result.get("error"):
        return "harness"
    return "product" if failed else "not_run"


def attempt_status(directory):
    """(status, detail, class). PASS only for a runner result.json that says PASS."""
    try:
        result = json.loads((directory / "result.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return "FAIL", "no readable result.json", "harness"
    klass = classify(result)
    if klass is None:
        return "PASS", "", None
    detail = result.get("error") if isinstance(result, dict) else None
    if not detail and isinstance(result, dict) and isinstance(result.get("steps"), list):
        detail = next((f"{step.get('id')}: {step.get('observed')}" for step in result["steps"]
                       if isinstance(step, dict) and step.get("status") == "FAIL"), None)
    return "FAIL", str(detail or "one or more steps did not pass")[:300], klass


def summarize(results, quarantine=None):
    """results: {journey: [(status, detail, class), ...]} in attempt order."""
    quarantine = quarantine or {}
    journeys = {}
    for journey, attempts in results.items():
        passed = sum(status == "PASS" for status, _, _ in attempts)
        failed = len(attempts) - passed
        classification = "pass" if failed == 0 else "fail" if passed == 0 else "flaky"
        journeys[journey] = {"attempts": len(attempts), "passed": passed, "failed": failed,
                             "classification": classification,
                             "failure_rate": round(failed / len(attempts), 3) if attempts else 0.0,
                             "failure_classes": {name: sum(klass == name for _, _, klass in attempts) for name in CLASSES},
                             "failures": [{"class": klass, "detail": detail} for status, detail, klass in attempts if status != "PASS"],
                             "quarantine": quarantine.get(journey)}
    flaky = sum(row["classification"] == "flaky" for row in journeys.values())
    failing = sorted(name for name, row in journeys.items() if row["classification"] == "fail")
    return {"schema_version": 2, "journeys": journeys,
            "flaky_journeys": flaky, "flake_rate": round(flaky / len(journeys), 3) if journeys else 0.0,
            "failing_journeys": [name for name in failing if not journeys[name]["quarantine"]],
            "quarantined_failing_journeys": [name for name in failing if journeys[name]["quarantine"]]}


def markdown(summary, commit):
    lines = [f"### Native journeys on `{commit[:12]}`", "",
             "| Journey | Passed | Product | Timeout | Environment | Harness | Not run | Failure rate | Result | Quarantine |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |"]
    for journey, row in summary["journeys"].items():
        classes = row["failure_classes"]
        quarantine = row["quarantine"]
        lines.append(f"| {journey} | {row['passed']}/{row['attempts']} | {classes['product']} | {classes['harness_timeout']} "
                     f"| {classes['environment']} | {classes['harness']} | {classes['not_run']} | {row['failure_rate']:.0%} "
                     f"| {row['classification']} | {quarantine['ticket'] + ': ' + quarantine['reason'] if quarantine else ''} |")
    lines += ["", f"Flaky journeys: {summary['flaky_journeys']} of {len(summary['journeys'])} "
                  f"(flake rate {summary['flake_rate']:.0%}). A journey that never passes fails the run unless it is "
                  "quarantined; a flaky or quarantined journey is reported as a warning.", ""]
    return "\n".join(lines)


def exit_status(summary):
    return 1 if summary["failing_journeys"] else 0


def report(summary, commit, summary_path=None, output=None):
    if output:
        (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    text = markdown(summary, commit)
    print(text)
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as stream:
            stream.write(text + "\n")
    for journey, row in summary["journeys"].items():
        if row["classification"] == "flaky":
            print(f"::warning::Native journey {journey} is flaky: {row['passed']}/{row['attempts']} attempts passed.")
        elif row["classification"] == "fail" and row["quarantine"]:
            print(f"::warning::Quarantined native journey {journey} failed ({row['quarantine']['ticket']}).")
    return exit_status(summary)


def collect(roots, journeys):
    """Read <journey>-<attempt>/result.json below each root, attempts in numeric order."""
    pattern = re.compile(r"(" + "|".join(map(re.escape, journeys)) + r")-([0-9]+)")
    found = {journey: {} for journey in journeys}
    for root in roots:
        for directory in sorted(path for path in Path(root).rglob("*") if path.is_dir()):
            match = pattern.fullmatch(directory.name)
            if match:
                attempt = int(match[2])
                require(attempt not in found[match[1]], f"Duplicate attempt {directory.name}")
                found[match[1]][attempt] = attempt_status(directory)
    return {journey: [rows[n] for n in sorted(rows)] for journey, rows in found.items() if rows}


def run(arguments):
    executable = arguments.executable.resolve(strict=True)
    quarantine = load_quarantine()
    output = arguments.output.absolute()
    output.mkdir(parents=True)
    rows = {row["id"]: row for row in runner.manifest(MANIFEST)["journeys"]}
    adapter = [sys.executable, str(HERE / "native_adapter.py")]
    results = {journey: [] for journey in arguments.journeys}
    for attempt in range(1, arguments.attempts + 1):
        for journey in arguments.journeys:
            directory = output / f"{journey}-{attempt}"
            timeout = rows[journey]["timeout_seconds"] + ATTEMPT_GRACE_SECONDS
            argv = [sys.executable, str(HERE / "runner.py"), "run", journey, "--output", str(directory),
                    "--commit", arguments.commit, "--reviewer", "scheduled-ci",
                    "--os-build", platform.platform(), "--hardware", arguments.hardware,
                    "--executable", str(executable), "--dpi", "100", "--mode", "keyboard",
                    "--theme", "dark", "--adapter", *adapter]
            try:
                completed = subprocess.run(argv, cwd=HERE.parents[1], capture_output=True, text=True, timeout=timeout)
                (output / f"{journey}-{attempt}.log").write_text(completed.stdout + completed.stderr, encoding="utf-8")
                results[journey].append(attempt_status(directory))
            except subprocess.TimeoutExpired:
                results[journey].append(("FAIL", f"runner exceeded {timeout} s", "harness_timeout"))
    code = report(summarize(results, quarantine), arguments.commit, arguments.summary, output)
    return 0 if arguments.no_fail else code


def summarize_command(arguments):
    summary = summarize(collect(arguments.results, ordinary_journeys()), load_quarantine())
    require(summary["journeys"], "No journey attempts found")
    if arguments.output:
        arguments.output.mkdir(parents=True, exist_ok=True)
    return report(summary, arguments.commit, arguments.summary, arguments.output)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="operation", required=True)
    journeys = ordinary_journeys()
    execute = sub.add_parser("run", help="run each journey N times into a fresh output directory")
    execute.add_argument("--executable", type=Path, required=True)
    execute.add_argument("--commit", required=True)
    execute.add_argument("--output", type=Path, required=True)
    execute.add_argument("--attempts", type=int, default=3, choices=range(1, 11))
    execute.add_argument("--journeys", nargs="+", default=journeys, choices=journeys)
    execute.add_argument("--hardware", default=os.environ.get("RUNNER_NAME", "local") + " (GitHub-hosted windows-2022)")
    execute.add_argument("--summary", help="Markdown file to append the report to, e.g. $GITHUB_STEP_SUMMARY")
    execute.add_argument("--no-fail", action="store_true", help="exit 0 after reporting; a later summarize decides")
    combine = sub.add_parser("summarize", help="combine attempt results from one or more run output directories")
    combine.add_argument("--results", type=Path, nargs="+", required=True)
    combine.add_argument("--commit", required=True)
    combine.add_argument("--output", type=Path, help="directory for summary.json")
    combine.add_argument("--summary", help="Markdown file to append the report to, e.g. $GITHUB_STEP_SUMMARY")
    sub.add_parser("validate", help="check the manifest tiers and the quarantine list")
    arguments = parser.parse_args()
    try:
        if arguments.operation == "validate":
            load_quarantine()
            print(f"{len(journeys)} ordinary journeys; quarantine list valid; no journey executed")
            return 0
        return run(arguments) if arguments.operation == "run" else summarize_command(arguments)
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
