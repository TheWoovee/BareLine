# SPDX-License-Identifier: MPL-2.0
"""Check workflow hygiene and the protected-branch check contract without YAML tooling.

The workflows use one plain 2-space block style, which this line-based reader
relies on; actionlint separately validates full YAML and expression syntax.
"""

import pathlib
import re
import sys

# Protected default-branch checks: job ids (and the neutral matrix, which forms
# the check names) must stay stable and must run on pull requests.
REQUIRED_JOBS = {"ci.yml": {"tooling", "native", "neutral"}, "supply-chain.yml": {"dependencies", "reproducibility"}}
NEUTRAL_MATRIX = "[ubuntu-latest, macos-latest]"
# Everything else must not be able to block or flake a pull request.
PULL_REQUEST_WORKFLOWS = {"ci.yml", "supply-chain.yml", "preview-release.yml"}
# Non-required workflows that also follow pushes to exactly this inline branch
# list. Pull requests and every other branch filter stay forbidden for them.
BRANCH_PUSH_WORKFLOWS = {"port-smoke.yml": "[port-integration]"}
# perf-nightly.yml is owned and re-pinned on its own branch; its jobs are
# opt-in and informational. Timeouts and action pins still apply to it.
FLOATING_RUNNER_EXEMPT = {"perf-nightly.yml"}
TOP_KEY = re.compile(r"^([A-Za-z_-]+):(.*)$")
JOB_ID = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
CHILD_KEY = re.compile(r"^  ([A-Za-z_-]+):")
USES = re.compile(r"^\s*(?:-\s+)?uses:\s*([^\s#]+)")
PINNED = re.compile(r"^[^@\s]+@[0-9a-f]{40}$")


def blocks(lines, pattern):
    """Split lines into {key: (inline value, body lines)} at one indentation level."""
    result, current = {}, None
    for line in lines:
        match = pattern.match(line)
        if match:
            current = match[1]
            result[current] = ((match[2] if match.lastindex and match.lastindex > 1 else "").strip(), [])
        elif current is not None:
            result[current][1].append(line)
    return result


def events(top):
    inline, body = top.get("on", top.get('"on"', ("", [])))
    if inline:
        return set(re.findall(r"[a-z_]+", inline))
    return {match[1] for line in body if (match := CHILD_KEY.match(line))}


def push_has_branches(top, allowed=None):
    """Whether push follows branches, other than an exact allowed `branches:` list."""
    _, body = top.get("on", ("", []))
    inside = False
    for line in body:
        if CHILD_KEY.match(line):
            inside = line.startswith("  push:")
        elif inside and (match := re.match(r"^    (branches(?:-ignore)?):(.*)$", line)):
            if (match[1], match[2].strip()) != ("branches", allowed):
                return True
    return False


def inspect(name, text):
    errors = []
    lines = [line for line in text.splitlines() if line.strip() and not line.lstrip().startswith("#")]
    top = blocks(lines, TOP_KEY)
    triggers = events(top)
    jobs = blocks(top.get("jobs", ("", []))[1], JOB_ID)
    if not jobs:
        errors.append(f"{name}: no jobs found")
    for job, (_, body) in jobs.items():
        if not any(line.startswith("    timeout-minutes:") for line in body):
            errors.append(f"{name}: job {job} has no timeout-minutes")
    if name not in FLOATING_RUNNER_EXEMPT and "windows-latest" in text:
        errors.append(f"{name}: pin windows-2022 instead of the rolling windows-latest image")
    for line in lines:
        if (match := USES.match(line)) and not match[1].startswith(("./", "docker://")) and not PINNED.match(match[1]):
            errors.append(f"{name}: action {match[1]} is not pinned to a full commit SHA")
    permissions = top.get("permissions", ("", []))
    if re.search(r"(id-token|attestations):\s*write", permissions[0] + "\n".join(permissions[1])):
        errors.append(f"{name}: OIDC/attestation write permission must be scoped to the attesting job")
    if name in REQUIRED_JOBS:
        missing = REQUIRED_JOBS[name] - set(jobs)
        if missing:
            errors.append(f"{name}: required check job(s) missing: {', '.join(sorted(missing))}")
        if "pull_request" not in triggers:
            errors.append(f"{name}: required checks must run on pull_request")
    if name == "ci.yml" and "neutral" in jobs:
        if not any(line.strip() == f"os: {NEUTRAL_MATRIX}" for line in jobs["neutral"][1]):
            errors.append(f"{name}: neutral matrix must stay {NEUTRAL_MATRIX} (required check names)")
    if name not in PULL_REQUEST_WORKFLOWS and (triggers & {"pull_request", "pull_request_target"} or push_has_branches(top, BRANCH_PUSH_WORKFLOWS.get(name))):
        errors.append(f"{name}: only schedule, workflow_dispatch or tag pushes may start this non-required workflow")
    return errors


def self_test():
    valid = """name: Fixture
on:
  pull_request:
  push:
    branches: [master]
permissions:
  contents: read
jobs:
  tooling:
    runs-on: windows-2022
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - uses: ./local-action
  native:
    runs-on: windows-2022
    timeout-minutes: 5
    steps:
      - run: cargo test
  neutral:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 5
    steps:
      - run: cargo test
"""
    assert inspect("ci.yml", valid) == [], inspect("ci.yml", valid)
    cases = {
        "no timeout-minutes": valid.replace("    timeout-minutes: 5\n    steps:\n      - run: cargo test\n  neutral", "    steps:\n      - run: cargo test\n  neutral"),
        "windows-latest": valid.replace("runs-on: windows-2022\n    timeout-minutes: 5\n    steps:\n      - run", "runs-on: windows-latest\n    timeout-minutes: 5\n    steps:\n      - run"),
        "not pinned": valid.replace("checkout@11d5960a326750d5838078e36cf38b85af677262", "checkout@v4"),
        "attestation write": valid.replace("  contents: read\n", "  contents: read\n  id-token: write\n"),
        "missing: native": valid.replace("  native:\n", "  native-renamed:\n"),
        "must run on pull_request": valid.replace("  pull_request:\n", ""),
        "neutral matrix": valid.replace("[ubuntu-latest, macos-latest]", "[ubuntu-24.04, macos-latest]"),
    }
    for expected, text in cases.items():
        assert any(expected in error for error in inspect("ci.yml", text)), expected
    scheduled = valid.replace("  pull_request:\n  push:\n    branches: [master]\n", "  schedule:\n    - cron: '0 3 * * 1'\n  push:\n    tags: ['v*']\n")
    assert inspect("extra.yml", scheduled) == [], inspect("extra.yml", scheduled)
    assert any("non-required" in error for error in inspect("extra.yml", valid))
    assert any("non-required" in error for error in inspect("extra.yml", "on: [pull_request]\n" + valid.split("permissions:\n", 1)[1]))
    branch = scheduled.replace("    tags: ['v*']\n", "    branches: [port-integration]\n")
    assert inspect("port-smoke.yml", branch) == [], inspect("port-smoke.yml", branch)
    widened = {
        "other workflow": ("extra.yml", branch),
        "extra branch": ("port-smoke.yml", branch.replace("[port-integration]", "[port-integration, master]")),
        "branches-ignore": ("port-smoke.yml", branch.replace("    branches:", "    branches-ignore:")),
        "pull request": ("port-smoke.yml", branch.replace("  schedule:\n", "  pull_request:\n  schedule:\n")),
    }
    for expected, (name, text) in widened.items():
        assert any("non-required" in error for error in inspect(name, text)), expected


if __name__ == "__main__":
    self_test()
    directory = pathlib.Path(__file__).resolve().parent
    failures = []
    for path in sorted([*directory.glob("*.yml"), *directory.glob("*.yaml")]):
        failures.extend(inspect(path.name, path.read_text(encoding="utf-8-sig")))
    print("\n".join(failures) if failures else "Workflow contract passed: timeouts, pinned runners and actions, "
          "stable required checks, scoped attestation permissions; negative fixtures passed.")
    sys.exit(bool(failures))
