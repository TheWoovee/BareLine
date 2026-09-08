# SPDX-License-Identifier: MPL-2.0
"""PR021 opt-in journey execution and strict parity evidence resolution.

No application is launched by validate/resolve. Run requires an explicit reviewed
adapter. Capture/deadlines/output bounds reuse PR019, not a second supervisor.
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
from perf_suite import digest, read_json, write_new

STATES = {"Implemented", "Equivalent Different UX", "Excluded With Reason",
          "v1.1", "Extension-provided (first-party)"}
COMMIT = re.compile(r"[0-9a-f]{40}")
JOURNEYS = ("plain_text", "code_config", "regex_transform", "column_multi_cursor",
            "huge_log_tail", "workspace", "udl", "macro_external", "split_clone_sync",
            "crash_recovery", "extension_isolation", "portable", "install_update_rollback")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def local_path(name):
    path = (ROOT / name).resolve()
    require(path.is_relative_to(ROOT), "Evidence path escapes repository")
    return path


def manifest(path):
    data = read_json(path)
    require(data.get("schema_version") == 1, "Unsupported journey schema")
    rows = data.get("journeys", [])
    require(len(rows) == 13 and {r["id"] for r in rows} == set(JOURNEYS),
            "Manifest must contain each of the thirteen journeys exactly once")
    for row in rows:
        require(type(row["timeout_seconds"]) is int and 1 <= row["timeout_seconds"] <= 600,
                "Journey deadline must be 1..600 seconds")
        steps = row["steps"]
        require(1 <= len(steps) <= 32 and len({s["id"] for s in steps}) == len(steps),
                "Missing, duplicate or excessive journey steps")
        require(all(s.get("action") and s.get("expected") for s in steps),
                "Each step needs an action and observable expectation")
    return data


def observations(response, journey):
    require(response.get("schema_version") == 1 and response.get("journey") == journey["id"],
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
    directory = ROOT / "tests/e2e/results" / str(uuid.uuid4())
    directory.mkdir(parents=True)
    request = {"schema_version": 1, "journey": journey, "executable": str(executable),
               "binary_sha256": digest(executable), "scratch": str(directory / "scratch"),
               "os_build": args.os_build, "hardware": args.hardware,
               "mode": args.mode, "theme": args.theme, "dpi": args.dpi,
               "response": str(directory / "response.json")}
    (directory / "scratch").mkdir()
    write_new(directory / "request.json", request)
    result = {"schema_version": 1, "journey": args.journey, "commit": args.commit,
              "reviewer": args.reviewer, "request": request, "adapter": args.adapter,
              "status": "FAIL", "steps": []}
    child = None
    try:
        child = OwnedProcessTree(args.adapter + [str(directory / "request.json")], directory)
        deadline = time.monotonic() + journey["timeout_seconds"]
        while child.alive() and time.monotonic() < deadline:
            response_path = directory / "response.json"
            require(not response_path.exists() or response_path.stat().st_size <= 256 * 1024,
                    "Adapter response exceeds 256 KiB")
            time.sleep(0.05)
        require(not child.alive(), "Adapter deadline exceeded")
        require((directory / "response.json").stat().st_size <= 256 * 1024,
                "Adapter response exceeds 256 KiB")
        response = read_json(directory / "response.json")
        result["status"] = observations(response, journey)
        result["steps"] = response["steps"]
    except (ValueError, KeyError, TypeError, OSError) as error:
        result["error"] = str(error)
    finally:
        if child:
            child.close()
    write_new(directory / "result.json", result)
    print(directory / "result.json")
    return 0 if result["status"] == "PASS" else 1


def resolve(args):
    index = read_json(args.index)
    require(index.get("schema_version") == 1, "Unsupported parity index")
    atomic = set(re.findall(r"AC-\d{3}-\d{2}",
                 (ROOT / "docs/blueprint/10_ACCEPTANCE_AND_TRACEABILITY.md").read_text(encoding="utf-8")))
    inventory = read_json(args.inventory) if args.inventory else None
    unresolved = []
    required = set(atomic)
    if inventory is None:
        unresolved.append("Missing runtime command inventory")
    else:
        require(inventory.get("schema_version") == 1 and COMMIT.fullmatch(inventory.get("commit", "")),
                "Runtime inventory needs schema_version 1 and full commit")
        commands = inventory.get("commands", [])
        require(commands and len(commands) <= 10000 and len(set(commands)) == len(commands),
                "Runtime inventory empty, duplicated or too large")
        for command in commands:
            require(re.fullmatch(r"[a-zA-Z0-9_.-]{1,160}", command), "Invalid command ID")
            required.update(f"command:{command}:{kind}" for kind in ("success", "disabled", "failure"))
    records = index.get("evidence", [])
    require(len(records) <= 40000, "Too many evidence records")
    resolved = {}
    for record in records:
        key = record["id"]
        require(key not in resolved, "Duplicate evidence ID: " + key)
        require(key in required, "Unknown evidence ID: " + key)
        if record.get("excluded"):
            require(record.get("reason") and record.get("authority"), "Exclusion requires reason/authority")
            require(local_path(record["authority"]).is_file(), "Missing exclusion authority")
            resolved[key] = "EXCLUDED"
            continue
        require(COMMIT.fullmatch(record.get("commit", "")), "Missing implementation commit: " + key)
        require(record.get("reviewer") and record.get("reviewer") != record.get("implementer")
                and record.get("implementer"), "Independent reviewer required: " + key)
        require(record.get("fixture_sha256") and re.fullmatch(r"[0-9a-f]{64}", record["fixture_sha256"]),
                "Fixture hash required: " + key)
        require(all(record.get(k) for k in ("os_build", "hardware", "command", "observed")),
                "Incomplete execution metadata: " + key)
        artifact = local_path(record["result"])
        require(digest(artifact) == record.get("result_sha256"), "Result hash mismatch: " + key)
        require(record.get("status") in {"PASS", "FAIL", "NOT_RUN"}, "Invalid status")
        if inventory and record["commit"] != inventory["commit"]:
            unresolved.append(key + ": result commit differs from inventory")
        resolved[key] = record["status"]
    unresolved.extend(key for key in sorted(required) if resolved.get(key) not in {"PASS", "EXCLUDED"})
    rows = index.get("features", [])
    require(rows and len({r["id"] for r in rows}) == len(rows), "Missing/duplicate parity features")
    blueprint = (ROOT / "docs/blueprint/05_NOTEPADPP_PARITY_MATRIX.md").read_text(encoding="utf-8")
    table = blueprint.split("## Feature families", 1)[1].split("## Rules", 1)[0]
    families = {line.split("|")[1].strip() for line in table.splitlines()
                if line.startswith("|") and not line.startswith(("|---", "| Notepad++ capability"))}
    require(families.issubset({r["capability"] for r in rows}), "Parity index silently omits blueprint rows")
    for row in rows:
        if row.get("state") not in STATES:
            unresolved.append(row["id"] + ": delivery state unresolved")
        elif row["state"] in {"Excluded With Reason", "v1.1"}:
            if not row.get("reason") or not row.get("authority") or not local_path(row["authority"]).is_file():
                unresolved.append(row["id"] + ": exclusion/deferral authority missing")
        elif not row.get("cases") or any(resolved.get(c) != "PASS" for c in row["cases"]):
            unresolved.append(row["id"] + ": independent parity evidence missing")
    report = {"schema_version": 1, "required_count": len(required), "resolved": resolved,
              "inventory_commit": inventory["commit"] if inventory else None,
              "inventory_sha256": digest(args.inventory) if inventory else None,
              "index_sha256": digest(args.index),
              "unresolved": sorted(set(unresolved)), "evidence_complete": not unresolved,
              "performance": "Reuse PR019 comparison/raw data; no timing release gate",
              "scope": "Evidence index completeness only; does not attest signing or release reliability"}
    if args.output:
        write_new(args.output, report)
    else:
        print(json.dumps(report, indent=2))
    return 1 if unresolved else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    validate = sub.add_parser("validate")
    validate.add_argument("--manifest", type=Path, default=ROOT / "tests/e2e/journeys.json")
    resolve_parser = sub.add_parser("resolve")
    resolve_parser.add_argument("--index", type=Path, default=ROOT / "docs/parity/index.json")
    resolve_parser.add_argument("--inventory", type=Path)
    resolve_parser.add_argument("--output", type=Path)
    execute = sub.add_parser("run")
    execute.add_argument("journey", choices=JOURNEYS)
    execute.add_argument("--manifest", type=Path, default=ROOT / "tests/e2e/journeys.json")
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
        return resolve(args) if args.operation == "resolve" else run(args)
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
