# SPDX-License-Identifier: MPL-2.0
"""Opt-in native regression journeys with bounded process and output lifetimes.

Validate checks the manifest without launching an application. Run requires an
explicit adapter and records each observed step in a fresh scratch directory.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests/perf"))
from perf_suite import digest, write_new
from evidence_json import loads, read_bounded_bytes, read_json
import environment

COMMIT = re.compile(r"[0-9a-f]{40}")
# Blueprint acceptance cases (10_ACCEPTANCE_AND_TRACEABILITY) and manual-QA issue IDs.
CASE = re.compile(r"AC-[0-9]{3}-[0-9]{2}")
ISSUE = re.compile(r"ISSUE-[0-9]{3}|PR-[A-Z][0-9]{2}|U[0-9]{2}")
# The thirteen product journeys plus the focused manual-QA regression procedure.
JOURNEYS = ("plain_text", "code_config", "regex_transform", "column_multi_cursor",
            "huge_log_tail", "workspace", "udl", "macro_external", "split_clone_sync",
            "crash_recovery", "extension_isolation", "portable", "install_update_rollback",
            "ui_regressions")
# "vm-only" journeys terminate, install or uninstall owned software. They run
# only on a disposable VM with --lab-config; scheduled CI never selects them.
TIERS = ("ordinary", "vm-only")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def versioned(value, version=1):
    return isinstance(value, dict) and type(value.get("schema_version")) is int and value["schema_version"] == version


def manifest(path):
    data = read_json(path)
    require(versioned(data), "Unsupported journey schema")
    rows = data.get("journeys", [])
    require(len(rows) == len(JOURNEYS) and {r["id"] for r in rows} == set(JOURNEYS),
            "Manifest must contain each journey exactly once")
    for row in rows:
        require(type(row["timeout_seconds"]) is int and 1 <= row["timeout_seconds"] <= 600,
                "Journey deadline must be 1..600 seconds")
        require(row.get("tier") in TIERS, "Journey tier must be ordinary or vm-only")
        cases = row.get("cases")
        require(isinstance(cases, list) and cases and len(set(cases)) == len(cases)
                and all(isinstance(case, str) and CASE.fullmatch(case) for case in cases),
                "Journey needs unique acceptance-case IDs (AC-NNN-NN)")
        steps = row["steps"]
        require(1 <= len(steps) <= 32 and len({s["id"] for s in steps}) == len(steps),
                "Missing, duplicate or excessive journey steps")
        require(all(s.get("action") and s.get("expected") for s in steps),
                "Each step needs an action and observable expectation")
        require(all("issue" not in s or (isinstance(s["issue"], str) and ISSUE.fullmatch(s["issue"])) for s in steps),
                "Step issue must be a manual-QA ID such as ISSUE-005, PR-T05 or U08")
    return data


def tier(data, name):
    """Journey IDs of one tier, in manifest order."""
    return [row["id"] for row in data["journeys"] if row["tier"] == name]


def observations(response, journey):
    require(versioned(response) and response.get("journey") == journey["id"],
            "Adapter response identity mismatch")
    steps = response.get("steps", [])
    expected = {s["id"] for s in journey["steps"]}
    require(len(steps) == len(expected) and {s["id"] for s in steps} == expected,
            "Adapter must report every step exactly once")
    for step in steps:
        require(step.get("status") in {"PASS", "FAIL", "NOT_RUN"} and
                isinstance(step.get("observed"), str) and step["observed"].strip(),
                "Step needs explicit status and observed output")
    return "PASS" if all(s["status"] == "PASS" for s in steps) else "FAIL"


def journey_environment(request):
    fields = ("os_build", "hardware", "mode", "theme", "dpi")
    require(all(isinstance(request.get(key), str) and request[key].strip() for key in fields),
            "Captured journey environment is incomplete")
    captured = {key: request[key] for key in fields}
    if 'qualification_environment' in request:
        complete = environment.validate(request['qualification_environment'])
        require(all(complete[key] == value for key, value in captured.items()),
                "Captured qualification environment contradicts the journey")
        return complete
    return captured


def run(args):
    require(os.name == "nt", "Native journeys require Windows")
    from windows_process_metrics import OwnedProcessTree
    data = manifest(args.manifest)
    journey = next(r for r in data["journeys"] if r["id"] == args.journey)
    require(COMMIT.fullmatch(args.commit), "Full implementation commit required")
    require(args.adapter and Path(args.adapter[0]).is_absolute(),
            "Explicit adapter argv must start with an absolute executable path")
    require(args.reviewer.strip() and args.os_build.strip() and args.hardware.strip(),
            "Reviewer, OS build and hardware are required")
    executable = Path(args.executable).resolve(strict=True)
    destination = getattr(args, "output", None)
    directory = (Path(destination).absolute() if destination else
                 ROOT / "tests/e2e/results" / str(uuid.uuid4()))
    directory.mkdir(parents=True)
    request = {"schema_version": 1, "journey": journey, "executable": str(executable),
               "binary_sha256": digest(executable), "scratch": str(directory / "scratch"),
               "os_build": args.os_build, "hardware": args.hardware,
               "mode": args.mode, "theme": args.theme, "dpi": args.dpi,
               "response": str(directory / "response.json")}
    if getattr(args, 'environment', None):
        request['qualification_environment'] = environment.validate(read_json(args.environment))
        journey_environment(request)
    (directory / "scratch").mkdir()
    write_new(directory / "request.json", request)
    result = {"schema_version": 1, "journey": args.journey, "commit": args.commit,
              "reviewer": args.reviewer, "request": request, "adapter": args.adapter,
              "adapter_exit_code": None, "status": "FAIL", "steps": []}
    child = None
    try:
        child = OwnedProcessTree(args.adapter + [str(directory / "request.json")], directory)
        deadline = time.monotonic() + journey["timeout_seconds"]
        exit_code = child.poll_exit_code()
        while exit_code is None:
            require(time.monotonic() < deadline, "Adapter deadline exceeded")
            response_path = directory / "response.json"
            require(not response_path.exists() or response_path.stat().st_size <= 256 * 1024,
                    "Adapter response exceeds 256 KiB")
            time.sleep(0.05)
            exit_code = child.poll_exit_code()
        result["adapter_exit_code"] = exit_code
        require(exit_code == 0, f"Adapter exited unsuccessfully with code {exit_code}")
        require((directory / "response.json").stat().st_size <= 256 * 1024,
                "Adapter response exceeds 256 KiB")
        response = loads(read_bounded_bytes(directory / "response.json", 256 * 1024))
        result["status"] = observations(response, journey)
        result["steps"] = response["steps"]
    except (ValueError, KeyError, TypeError, OSError) as error:
        result["error"] = str(error)
    finally:
        if child:
            child.close()
    # Check after Job cleanup as well: a surviving producer descendant must
    # not change the pinned executable after its PASS response was read.
    result["binary_sha256_after"] = None
    try:
        result["binary_sha256_after"] = digest(executable)
        require(result["binary_sha256_after"] == request["binary_sha256"],
                "Pinned executable changed during the journey")
    except (ValueError, OSError) as error:
        result.update(status="FAIL", error=str(error))
    result_path = directory / "result.json"
    write_new(result_path, result)
    print(json.dumps({"evidence_result": str(result_path.resolve()),
                      "sha256": digest(result_path)}, separators=(",", ":")))
    return 0 if result["status"] == "PASS" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    validate = sub.add_parser("validate")
    validate.add_argument("--manifest", type=Path, default=ROOT / "tests/e2e/journeys.json")
    execute = sub.add_parser("run")
    execute.add_argument("journey", choices=JOURNEYS)
    execute.add_argument("--manifest", type=Path, default=ROOT / "tests/e2e/journeys.json")
    execute.add_argument('--output', type=Path, help='Fresh output directory; defaults to tests/e2e/results/<run-id>')
    execute.add_argument('--environment', type=Path, help='Complete test environment declaration')
    for name in ("commit", "reviewer", "os-build", "hardware", "executable", "dpi"):
        execute.add_argument("--" + name, required=True)
    execute.add_argument("--mode", choices=["keyboard", "screen_reader", "pointer"], required=True)
    execute.add_argument("--theme", choices=["dark", "light", "high_contrast"], required=True)
    execute.add_argument("--adapter", nargs=argparse.REMAINDER, required=True)
    args = parser.parse_args()
    try:
        if args.operation == "validate":
            manifest(args.manifest)
            print("Journey source manifest valid; no journey executed")
            return 0
        return run(args)
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
