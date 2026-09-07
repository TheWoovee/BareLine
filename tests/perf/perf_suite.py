# SPDX-License-Identifier: MPL-2.0
"""Opt-in paired measurement orchestration. Never manufactures missing metrics.

Drivers emit one JSON line: {"event":"measurement","metrics":{name:number}}.
An adapter must measure the real application; process duration is not editable time.
All subprocesses use argument arrays, bounded output, deadlines and retained evidence.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import signal
import threading
import time
import uuid

SCENARIOS = (
    "cold_launch", "warm_launch", "empty_idle", "open_10mb", "open_100mb",
    "open_1gb", "open_5gb", "long_line", "scroll", "edit_to_paint",
    "syntax_viewport", "literal_search", "regex_search", "search_cancel",
    "result_jump", "save", "save_as", "workspace_scan", "tail_append",
    "tabs_100", "tabs_500", "extensions_memory",
)
LIMIT = 256 * 1024


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            value.update(chunk)
    return value.hexdigest()


def read_json(path):
    if Path(path).stat().st_size > 16 * 1024 * 1024:
        raise ValueError("JSON input exceeds 16 MiB")
    with open(path, encoding="utf-8") as stream:
        return json.load(stream)


def write_new(path, value):
    with open(path, "x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


def capture(argv, cwd, timeout):
    """Drain both pipes concurrently, including after output quota is reached."""
    started = time.monotonic()
    result = {"argv": argv, "exit_code": None, "status": "spawn_error"}
    try:
        child = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 shell=False, start_new_session=os.name != "nt")
    except OSError as error:
        return {**result, "error": str(error), "duration_us": 0}
    buffers = {"stdout": bytearray(), "stderr": bytearray()}
    overflow = threading.Event()

    def drain(stream, name):
        try:
            while True:
                block = stream.read(65536)
                if not block:
                    break
                remaining = LIMIT - len(buffers[name])
                buffers[name].extend(block[:max(remaining, 0)])
                if len(block) > remaining:
                    overflow.set()
        finally:
            stream.close()

    readers = [threading.Thread(target=drain, args=(getattr(child, n), n), daemon=True)
               for n in buffers]
    for reader in readers:
        reader.start()
    status = None
    while child.poll() is None:
        if overflow.is_set():
            status = "output_limit"
            break
        if time.monotonic() - started >= timeout:
            status = "timeout"
            break
        time.sleep(0.02)
    if status:
        if os.name == "nt":
            # Target only the still-live process we spawned and its descendants.
            taskkill = str(Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "taskkill.exe")
            try:
                subprocess.run([taskkill, "/PID", str(child.pid), "/T", "/F"],
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=5, check=False)
            except (OSError, subprocess.TimeoutExpired):
                pass
            if child.poll() is None:
                child.kill()
        else:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    child.wait()
    for reader in readers:
        reader.join(timeout=1)
    # Drivers must reap their own descendants. Never terminate unrelated applications.
    if any(reader.is_alive() for reader in readers):
        status = "inherited_pipe_open"
    elif overflow.is_set():
        status = "output_limit"
    result.update(status=status or ("ok" if child.returncode == 0 else "failed"),
                  exit_code=child.returncode,
                  duration_us=int((time.monotonic() - started) * 1_000_000))
    for name, buffer in buffers.items():
        result[name] = buffer.decode("utf-8", errors="replace")
    return result


def measurement(output):
    events = []
    for line in output.splitlines():
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        if isinstance(event, dict) and event.get("event") == "measurement":
            events.append(event)
    if len(events) != 1 or not isinstance(events[0].get("metrics"), dict):
        raise ValueError("driver must emit exactly one measurement metrics object")
    metrics = events[0]["metrics"]
    if not metrics or any(not isinstance(k, str) or isinstance(v, bool)
                          or not isinstance(v, (int, float)) or not math.isfinite(v)
                          or v < 0 for k, v in metrics.items()):
        raise ValueError("metrics must be finite nonnegative numbers; omit unmeasured metrics")
    return metrics


def run(manifest_path, destination):
    manifest = read_json(manifest_path)
    base = Path(manifest_path).resolve().parent
    repetitions = manifest["repetitions"]
    if isinstance(repetitions, bool) or not isinstance(repetitions, int) or not 1 <= repetitions <= 100:
        raise ValueError("repetitions must be 1..100")
    if manifest["series"] not in ("local", "hosted", "profiled"):
        raise ValueError("series must be local, hosted or profiled")
    # A common configuration identity is explicit, never inferred from OS alone.
    if not manifest.get("configuration") or not manifest.get("machine_id"):
        raise ValueError("configuration and machine_id are required")
    apps = manifest["applications"]
    if set(apps) != {"bareline", "notepadpp"}:
        raise ValueError("paired runs require bareline and notepadpp")
    for app in apps.values():
        app["executable"] = str((base / app["executable"]).resolve())
        if digest(app["executable"]) != app["sha256"].lower():
            raise ValueError("application executable hash mismatch")
        if not app.get("version") or not app.get("settings"):
            raise ValueError("pin application version and settings")
    if apps["notepadpp"]["settings"].get("plugins_disabled") is not True:
        raise ValueError("Notepad++ plugins_disabled must be true")
    scenarios = manifest["scenarios"]
    names = [s["name"] for s in scenarios]
    if len(names) != len(set(names)) or any(n not in SCENARIOS for n in names):
        raise ValueError("unknown or duplicate scenario")
    for scenario in scenarios:
        if not isinstance(scenario.get("comparable_metrics", []), list) or any(
                not isinstance(metric, str) for metric in scenario.get("comparable_metrics", [])):
            raise ValueError("comparable_metrics must explicitly list reviewed matching endpoints")
        if not scenario.get("cache_state") or not scenario.get("renderer"):
            raise ValueError("explicit cache_state and renderer required")
        if not 1 <= scenario["timeout_seconds"] <= 3600:
            raise ValueError("scenario timeout must be 1..3600 seconds")
        for fixture in scenario.get("fixtures", []):
            if digest(base / fixture["path"]) != fixture["sha256"].lower():
                raise ValueError("fixture hash mismatch")
        for app_id in apps:
            driver = scenario["drivers"][app_id]
            driver["argv"][0] = str((base / driver["argv"][0]).resolve())
            if digest(driver["argv"][0]) != driver["sha256"].lower():
                raise ValueError("driver executable hash mismatch")
            if any(not isinstance(arg, str) for arg in driver["argv"]):
                raise ValueError("driver argv must be strings")
            for pinned in driver.get("pinned_files", []):
                if digest(base / pinned["path"]) != pinned["sha256"].lower():
                    raise ValueError("driver support file hash mismatch")
    destination = Path(destination).resolve() / (time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex)
    destination.mkdir(parents=True)
    envelope = {"schema_version": 1, "kind": "paired_performance", "manifest": manifest,
                "manifest_sha256": digest(manifest_path), "environment": {
                    "os": platform.platform(), "architecture": platform.machine(),
                    "logical_cpus": os.cpu_count(), "python": platform.python_version()},
                "missing_scenarios": sorted(set(SCENARIOS) - set(names))}
    write_new(destination / "provenance.json", envelope)
    output_bytes = 0
    for scenario in scenarios:
        for pair in range(repetitions):
            order = ["bareline", "notepadpp"] if pair % 2 == 0 else ["notepadpp", "bareline"]
            for position, app_id in enumerate(order):
                argv = [arg.replace("{application}", apps[app_id]["executable"])
                        for arg in scenario["drivers"][app_id]["argv"]]
                record = {"scenario": scenario["name"], "pair": pair, "position": position,
                          "application": app_id, "metrics": None,
                          **capture(argv, str(base), scenario["timeout_seconds"])}
                if record["status"] == "ok":
                    try:
                        record["metrics"] = measurement(record["stdout"])
                    except ValueError as error:
                        record.update(status="invalid_measurement", error=str(error))
                write_new(destination / f'{scenario["name"]}-{pair}-{app_id}.json', record)
                output_bytes += len(record.get("stdout", "").encode()) + len(record.get("stderr", "").encode())
                if output_bytes > 256 * 1024 * 1024:
                    write_new(destination / "interrupted.json", {"status": "run_output_quota", "unexecuted_trials": "See missing pairs in report"})
                    print(destination)
                    return
    print(destination)


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)] if ordered else None


def report(directory, destination):
    directory = Path(directory)
    provenance = read_json(directory / "provenance.json")
    manifest = provenance["manifest"]
    records = []
    for scenario in manifest["scenarios"]:
        for pair in range(manifest["repetitions"]):
            for app in ("bareline", "notepadpp"):
                path = directory / f'{scenario["name"]}-{pair}-{app}.json'
                if not path.exists():
                    continue
                raw = read_json(path)
                if (raw.get("scenario"), raw.get("pair"), raw.get("application")) != (scenario["name"], pair, app):
                    raise ValueError("trial identity does not match its artifact name")
                values = raw.get("metrics") or {}
                if not isinstance(values, dict) or any(isinstance(v, bool) or not isinstance(v, (int, float))
                        or not math.isfinite(v) or v < 0 for v in values.values()):
                    raise ValueError("invalid persisted metric")
                records.append({k: raw.get(k) for k in ("scenario", "pair", "position", "application", "metrics", "status")})
    rows = []
    observations = []
    for scenario in manifest["scenarios"]:
        selected = [r for r in records if r["scenario"] == scenario["name"]]
        metrics = sorted({m for r in selected for m in (r.get("metrics") or {})})
        for metric in metrics:
            for application in ("bareline", "notepadpp"):
                values = [r["metrics"][metric] for r in selected if r["application"] == application
                          and r["status"] == "ok" and metric in (r.get("metrics") or {})]
                observations.append({"scenario": scenario["name"], "application": application,
                    "metric": metric, "sample_count": len(values), "p50": percentile(values, .5),
                    "p95": percentile(values, .95), "comparison_eligible": False})
            if metric not in scenario.get("comparable_metrics", []):
                continue
            pairs = []
            excluded = []
            for pair in range(manifest["repetitions"]):
                members = [r for r in selected if r["pair"] == pair]
                expected = ["bareline", "notepadpp"] if pair % 2 == 0 else ["notepadpp", "bareline"]
                if (len(members) != 2 or sorted((r["position"], r["application"]) for r in members)
                        != list(enumerate(expected)) or any(r["status"] != "ok" or metric not in
                                                           (r.get("metrics") or {}) for r in members)):
                    excluded.append(pair)
                    continue
                pairs.append({r["application"]: r["metrics"][metric] for r in members})
            stats = {a: {"p50": percentile([p[a] for p in pairs], .5),
                         "p95": percentile([p[a] for p in pairs], .95)} for a in ("bareline", "notepadpp")}
            denominator = stats["notepadpp"]["p50"]
            rows.append({"scenario": scenario["name"], "metric": metric, "paired_samples": len(pairs),
                         "excluded_pairs": excluded, **stats,
                         "p50_ratio": stats["bareline"]["p50"] / denominator if denominator else None})
    write_new(destination, {"schema_version": 1, "provenance": provenance, "rows": rows, "observations": observations,
                            "failures": [r for r in records if r["status"] != "ok"],
                            "missing_trials": len(manifest["scenarios"]) * manifest["repetitions"] * 2 - len(records),
                            "claims_eligible": False,
                            "claim_review": "Review pinned comparable settings, actual driver semantics and complete evidence before any claim",
                            "percentile_method": "nearest rank; complete matched pairs only"})


def regress(candidate_path, baselines, destination):
    candidate = read_json(candidate_path)
    manifest = candidate["provenance"]["manifest"]
    history = [read_json(path) for path in baselines]
    if len(history) != 7:
        raise ValueError("rolling baseline requires exactly the previous seven reports")
    identity = ("series", "configuration", "machine_id")
    if any(any(report["provenance"]["manifest"][k] != manifest[k] for k in identity) for report in history):
        raise ValueError("cannot mix machines, configurations or hosted/profiled/local series")
    def scenarios(config):
        return [{k: value for k, value in scenario.items() if k != "drivers"}
                for scenario in config.get("scenarios", [])]
    if any(scenarios(old["provenance"]["manifest"]) != scenarios(manifest) for old in history):
        raise ValueError("cannot mix fixtures, cache states or scenario settings")
    def application_settings(config):
        apps = config.get("applications", {})
        return ({name: app.get("settings") for name, app in apps.items()},
                apps.get("notepadpp", {}).get("sha256"), apps.get("notepadpp", {}).get("version"))
    if any(application_settings(old["provenance"]["manifest"]) != application_settings(manifest) for old in history):
        raise ValueError("cannot mix application settings or comparator versions")
    findings = []
    for row in candidate["rows"]:
        if row.get("paired_samples", row.get("sample_count", 0)) < 3 or row.get("excluded_pairs"):
            continue
        previous = [old["bareline"]["p50"] for report in history for old in report["rows"]
                    if (old["scenario"], old["metric"]) == (row["scenario"], row["metric"])
                    and old.get("paired_samples", old.get("sample_count", 0)) >= 3 and not old.get("excluded_pairs")]
        if len(previous) != len(history):
            continue
        if any(isinstance(value, bool) or not isinstance(value, (int, float))
               or not math.isfinite(value) or value < 0 for value in previous):
            raise ValueError("invalid baseline P50")
        baseline = percentile(previous, .5)
        noise = max(abs(value - baseline) for value in previous) / baseline if baseline else 0
        threshold = max(.10, noise)
        current = row["bareline"]["p50"]
        if isinstance(current, bool) or not isinstance(current, (int, float)) or not math.isfinite(current) or current < 0:
            raise ValueError("invalid candidate P50")
        # Only time/byte costs have an established lower-is-better meaning here.
        if not row["metric"].endswith(("_us", "_bytes", "_bytes_point")):
            continue
        if baseline and current is not None and current > baseline * (1 + threshold):
            findings.append({"scenario": row["scenario"], "metric": row["metric"],
                             "baseline_p50": baseline, "current_p50": current,
                             "increase_fraction": current / baseline - 1,
                             "noise_tolerance_fraction": threshold})
    write_new(destination, {"schema_version": 1, "informational_only": True,
                            "candidate": str(candidate_path), "baselines": list(baselines),
                            "candidate_commit": manifest.get("commit"),
                            "baseline_commits": [old["provenance"]["manifest"].get("commit") for old in history],
                            "findings": findings, "ci_failure_from_numbers": False})


def summarize_legacy(directory, destination):
    """Hosted Bareline-only history; never a paired Notepad++ comparison."""
    groups = {}
    provenance = []
    failures = []
    for path in sorted(Path(directory).glob("*.json")):
        raw = read_json(path)
        if raw.get("kind") not in ("visible_launch_idle", "hidden_render_smoke", "headless_document_source"):
            continue
        provenance.append({"file": path.name, "sha256": digest(path),
                           "binary_sha256": raw.get("binary_sha256"), "profile": raw.get("profile"),
                           "os_build": raw.get("os_build"), "fixture": raw.get("fixture")})
        for sample in raw.get("samples", []):
            if sample.get("status", "ok") != "ok":
                failures.append({"file": path.name, "sample": sample})
                continue
            if raw["kind"] == "headless_document_source":
                scenario = "document-" + sample["source"]
                values = {k: v for k, v in sample.items() if k.endswith("_us") or k == "process_private_bytes"}
            else:
                scenario = raw["kind"] + ("-software" if sample["requested_software"] else "-hardware")
                values = {"first_frame_us": (sample.get("frame") or {}).get("microseconds"),
                          "idle_private_bytes": (sample.get("idle") or {}).get("private_bytes")}
            for metric, value in values.items():
                if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0:
                    groups.setdefault((scenario, metric), []).append(value)
    rows = [{"scenario": scenario, "metric": metric, "sample_count": len(values),
             "sample_scope": "unpaired Bareline-only hosted samples",
             "bareline": {"p50": percentile(values, .5), "p95": percentile(values, .95)},
             "notepadpp": {"p50": None, "p95": None}, "p50_ratio": None}
            for (scenario, metric), values in sorted(groups.items())]
    configuration = [{k: v for k, v in p.items() if k not in ("file", "sha256", "binary_sha256")} for p in provenance]
    write_new(destination, {"schema_version": 1, "provenance": {"manifest": {
        "series": "hosted", "machine_id": "github-windows-latest-ephemeral",
        "configuration": configuration, "commit": os.environ.get("GITHUB_SHA")},
        "raw_files": provenance}, "rows": rows, "claims_eligible": False,
        "failures": failures, "limitations": "Hosted noise series; unpaired, warm/uncontrolled cache, not marketing evidence"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    runner = sub.add_parser("run")
    runner.add_argument("manifest")
    runner.add_argument("destination")
    aggregator = sub.add_parser("report")
    aggregator.add_argument("directory")
    aggregator.add_argument("destination")
    regression = sub.add_parser("regress")
    regression.add_argument("candidate")
    regression.add_argument("destination")
    regression.add_argument("baselines", nargs="+")
    legacy = sub.add_parser("summarize-legacy")
    legacy.add_argument("directory")
    legacy.add_argument("destination")
    args = parser.parse_args()
    if args.command == "run":
        run(args.manifest, args.destination)
    elif args.command == "report":
        report(args.directory, args.destination)
    elif args.command == "regress":
        regress(args.candidate, args.baselines, args.destination)
    else:
        summarize_legacy(args.directory, args.destination)


if __name__ == "__main__":
    main()
