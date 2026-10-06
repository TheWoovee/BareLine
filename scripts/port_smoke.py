# SPDX-License-Identifier: MPL-2.0
"""Launch the editor once on Linux or macOS and retain what the port smoke proves.

Run by .github/workflows/port-smoke.yml (and by hand under WSL). It starts the
editor with one sample file, waits for its window under a hard deadline,
records launch-to-window time and resident memory, takes a screenshot, then
asks the editor to quit with SIGTERM (SIGKILL after a grace period) and checks
that the exit was clean: no panic, no failed startup, no early or forced exit.

Windows are found with xdotool on X11 (Xvfb on the runner) and with the Quartz
window list on macOS, which needs neither screen-recording nor accessibility
permission; System Events is the macOS fallback. Pixel content is not judged.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/e2e"))
import environment  # noqa: E402  (the journeys' environment identity fields)

# POSIX numbers (the same on macOS). Windows' signal module has no SIGKILL, and
# the pure checks below also run in the Windows tooling job.
SIGTERM, SIGKILL = 15, 9
# The fatal panic hook prints {"event":"panic",...}; std's default hook prints
# "thread '...' panicked at"; main() prints event=startup_failed.
FAILURE_TEXT = re.compile(r'"event"\s*:\s*"panic"|\bpanicked\b|event=startup_failed')
FIRST_FRAME = re.compile(r'^\{"event":"first_frame","microseconds":(\d+)', re.MULTILINE)
READY = "event=startup_phase phase=ready"
POLL_SECONDS = 0.05
TOOL_SECONDS = 20


class Unavailable(Exception):
    """A window discovery method cannot work in this session."""


def log_problems(stdout, stderr):
    """Lines that show a panic or a failed startup, prefixed by their stream."""
    return [f"{name}: {line.strip()[:300]}" for name, text in (("stdout", stdout), ("stderr", stderr))
            for line in text.splitlines() if FAILURE_TEXT.search(line)]


def first_frame_us(stdout):
    match = FIRST_FRAME.search(stdout)
    return int(match[1]) if match else None


def parse_rss_kib(text):
    """`ps -o rss=` prints the resident set in KiB on Linux and macOS."""
    text = text.strip()
    return int(text) if text.isdigit() else None


def describe(returncode):
    if returncode is None:
        return "no exit status"
    return f"signal {-returncode}" if returncode < 0 else f"exit code {returncode}"


def exit_problems(exited_early, returncode, killed):
    """A clean exit is the one asked for: ended by SIGTERM, or exit code 0 after it."""
    if exited_early:
        return [f"the editor ended by itself ({describe(returncode)}) before it was asked to quit"]
    if killed:
        return ["the editor was still running when the SIGTERM grace period ended and was killed"]
    if returncode not in (0, -SIGTERM):
        return [f"the editor ended with {describe(returncode)} after SIGTERM"]
    return []


def build_environment(system, machine, os_build, cpus, runner):
    """The journey environment identity (tests/e2e/environment.py) for this run."""
    architecture = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(machine.lower(), machine)
    return environment.validate({
        "os_family": {"Linux": "linux", "Darwin": "macos"}.get(system, system.lower()),
        "os_build": os_build or "unknown",
        "architecture": architecture,
        "hardware": f"{runner}, {cpus} logical CPUs",
        # Nothing drives input: the window is only opened, measured and closed.
        "mode": "headless",
        "theme": "not_applicable",
        "dpi": "not_applicable",
        "renderer": "software",
        "assistive_technology": "none",
        "build_mode": "preview",
    })


def current_environment():
    system = platform.system()
    if system == "Darwin":
        build = f"macOS {platform.mac_ver()[0]}"
    else:
        try:
            release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
            build = f"{release.get('PRETTY_NAME', 'Linux').strip(chr(34))}, kernel {platform.release()}"
        except OSError:
            build = f"Linux kernel {platform.release()}"
    runner = os.environ.get("RUNNER_ENVIRONMENT", "local")
    runner = f"{runner} runner {os.environ['ImageOS']}" if os.environ.get("ImageOS") else f"{runner} machine"
    return build_environment(system, platform.machine(), build, os.cpu_count(), runner)


def run_tool(arguments):
    try:
        return subprocess.run(arguments, capture_output=True, text=True, timeout=TOOL_SECONDS, check=False)
    except FileNotFoundError as error:
        raise Unavailable(f"{arguments[0]} is not installed") from error
    except subprocess.TimeoutExpired as error:
        raise Unavailable(f"{arguments[0]} did not answer within {TOOL_SECONDS} s") from error


def xdotool_windows(pid):
    """Viewable X11 windows whose _NET_WM_PID is `pid` (winit sets it)."""
    result = run_tool(["xdotool", "search", "--onlyvisible", "--pid", str(pid)])
    if result.returncode not in (0, 1):
        raise Unavailable(f"xdotool failed: {result.stderr.strip()[:200]}")
    return result.stdout.split()


class QuartzWindows:
    """On-screen, normal-layer windows of a process from CGWindowListCopyWindowInfo."""

    ON_SCREEN_ONLY, EXCLUDE_DESKTOP, SINT64 = 1 << 0, 1 << 4, 4

    def __init__(self):
        import ctypes
        self.ctypes = ctypes
        try:
            cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
            cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
            keys = [ctypes.c_void_p.in_dll(cg, name).value for name in ("kCGWindowOwnerPID", "kCGWindowLayer", "kCGWindowNumber")]
        except (OSError, ValueError) as error:
            raise Unavailable(f"Quartz is not available: {error}") from error
        pointer = ctypes.c_void_p
        cg.CGWindowListCopyWindowInfo.argtypes, cg.CGWindowListCopyWindowInfo.restype = [ctypes.c_uint32, ctypes.c_uint32], pointer
        cf.CFArrayGetCount.argtypes, cf.CFArrayGetCount.restype = [pointer], ctypes.c_long
        cf.CFArrayGetValueAtIndex.argtypes, cf.CFArrayGetValueAtIndex.restype = [pointer, ctypes.c_long], pointer
        cf.CFDictionaryGetValue.argtypes, cf.CFDictionaryGetValue.restype = [pointer, pointer], pointer
        cf.CFNumberGetValue.argtypes, cf.CFNumberGetValue.restype = [pointer, ctypes.c_long, pointer], ctypes.c_bool
        cf.CFRelease.argtypes, cf.CFRelease.restype = [pointer], None
        self.cg, self.cf, (self.owner, self.layer, self.number) = cg, cf, keys

    def value(self, dictionary, key):
        number = self.cf.CFDictionaryGetValue(dictionary, key)
        out = self.ctypes.c_int64()
        if number and self.cf.CFNumberGetValue(number, self.SINT64, self.ctypes.byref(out)):
            return out.value
        return None

    def __call__(self, pid):
        windows = self.cg.CGWindowListCopyWindowInfo(self.ON_SCREEN_ONLY | self.EXCLUDE_DESKTOP, 0)
        if not windows:
            raise Unavailable("the window server returned no window list (no GUI session)")
        try:
            entries = (self.cf.CFArrayGetValueAtIndex(windows, index) for index in range(self.cf.CFArrayGetCount(windows)))
            return [str(self.value(entry, self.number)) for entry in entries
                    if self.value(entry, self.owner) == pid and self.value(entry, self.layer) == 0]
        finally:
            self.cf.CFRelease(windows)


def system_events_windows(pid):
    """System Events needs accessibility permission; a refusal makes it unavailable."""
    script = f'tell application "System Events" to count windows of (first process whose unix id is {pid})'
    result = run_tool(["osascript", "-e", script])
    if result.returncode != 0 or not result.stdout.strip().isdigit():
        raise Unavailable(f"System Events refused: {result.stderr.strip()[:200]}")
    return [f"system-events-{index}" for index in range(int(result.stdout.strip()))]


def window_finders(system):
    if system == "Darwin":
        finders = []
        try:
            finders.append(("quartz", QuartzWindows()))
        except Unavailable as error:
            print(f"Window discovery: {error}", file=sys.stderr)
        return finders + [("system-events", system_events_windows)]
    return [("xdotool", xdotool_windows)]


def take_screenshot(system, path):
    path.unlink(missing_ok=True)
    command = ["screencapture", "-x", str(path)] if system == "Darwin" else ["scrot", str(path)]
    try:
        result = run_tool(command)
    except Unavailable as error:
        return {"path": path.name, "ok": False, "error": str(error)}
    ok = result.returncode == 0 and path.is_file() and path.stat().st_size > 0
    return {"path": path.name, "ok": ok, "command": command[0],
            "error": None if ok else (result.stderr.strip()[:300] or f"exit code {result.returncode}")}


def resident_kib(pid):
    try:
        return parse_rss_kib(run_tool(["ps", "-o", "rss=", "-p", str(pid)]).stdout)
    except Unavailable:
        return None


def window_title(method, window):
    if method != "xdotool":
        return None
    try:
        result = run_tool(["xdotool", "getwindowname", window])
    except Unavailable:
        return None
    return (result.stdout.strip() or None) if result.returncode == 0 else None


def profile_environment(profile):
    """Keep settings, sessions and caches inside `profile`, for today's and the XDG layout."""
    variables = {"XDG_CONFIG_HOME": "config", "XDG_DATA_HOME": "data", "XDG_STATE_HOME": "state",
                 "XDG_CACHE_HOME": "cache", "APPDATA": "appdata", "LOCALAPPDATA": "localappdata"}
    env = dict(os.environ)
    for name, directory in variables.items():
        (profile / directory).mkdir(parents=True, exist_ok=True)
        env[name] = str(profile / directory)
    return env


def wait_for_window(process, finders, deadline, alive_seconds):
    """Returns (method, windows, found_at_ns); method is None when nothing could look."""
    finders = list(finders)
    while process.poll() is None:
        for name, find in list(finders):
            try:
                windows = find(process.pid)
            except Unavailable as error:
                print(f"Window discovery via {name} is unavailable: {error}", file=sys.stderr)
                finders.remove((name, find))
                continue
            if windows:
                return name, windows, time.monotonic_ns()
            break
        if not finders:
            # Nothing can see windows here: the process staying up is all that is left.
            time.sleep(alive_seconds)
            return None, [], None
        if time.monotonic() >= deadline:
            return finders[0][0], [], None
        time.sleep(POLL_SECONDS)
    return (finders[0][0] if finders else None), [], None


def quit_editor(process, grace):
    """SIGTERM, then SIGKILL after `grace` seconds. Returns (returncode, killed, quit_ms)."""
    asked = time.monotonic_ns()
    process.send_signal(SIGTERM)
    try:
        process.wait(timeout=grace)
        killed = False
    except subprocess.TimeoutExpired:
        process.send_signal(SIGKILL)
        process.wait(timeout=10)
        killed = True
    return process.returncode, killed, round((time.monotonic_ns() - asked) / 1e6, 1)


def smoke(options, finders, screenshot=take_screenshot, rss=resident_kib, system=None):
    """Launch, measure and quit once; returns the metrics record."""
    system = system or platform.system()
    evidence = Path(options.evidence)
    evidence.mkdir(parents=True, exist_ok=True)
    label = options.label
    stdout_path, stderr_path = evidence / f"{label}-editor.stdout.log", evidence / f"{label}-editor.stderr.log"
    env = profile_environment(Path(options.profile))
    record = {"label": label, "executable": str(options.executable), "sample": str(options.sample),
              "timeout_s": options.timeout, "settle_s": options.settle, "grace_s": options.grace}
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        started = time.monotonic_ns()
        process = subprocess.Popen([str(options.executable), str(options.sample)], stdin=subprocess.DEVNULL,
                                   stdout=stdout, stderr=stderr, env=env, start_new_session=True)
        try:
            method, windows, found = wait_for_window(process, finders, time.monotonic() + options.timeout,
                                                     options.alive_seconds)
            if windows:
                # Let the first frames present and memory settle before measuring.
                time.sleep(options.settle)
            record["window"] = {
                "method": method or "process-alive",
                "found": bool(windows) if method else None,
                "ids": windows,
                "title": window_title(method, windows[0]) if windows else None,
                "launch_to_window_ms": round((found - started) / 1e6, 1) if found else None,
            }
            running = process.poll() is None
            record["memory"] = {"rss_kib": rss(process.pid) if running else None, "source": "ps -o rss="}
            record["screenshot"] = screenshot(system, evidence / f"{label}-window.png") if running else None
            exited_early = process.poll() is not None
            if exited_early:
                returncode, killed, quit_ms = process.returncode, False, None
            else:
                returncode, killed, quit_ms = quit_editor(process, options.grace)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            try:
                # Anything the editor started goes with it.
                os.killpg(process.pid, getattr(signal, "SIGKILL", SIGKILL))
            except (ProcessLookupError, PermissionError, AttributeError):
                pass
    record["exit"] = {"asked": None if exited_early else "SIGTERM", "returncode": returncode,
                      "status": describe(returncode), "killed": killed, "exited_before_quit": exited_early,
                      "quit_ms": quit_ms}
    out = stdout_path.read_text(encoding="utf-8", errors="replace")
    err = stderr_path.read_text(encoding="utf-8", errors="replace")
    record["editor"] = {"first_frame_us": first_frame_us(out), "startup_ready": READY in err,
                        "stdout": stdout_path.name, "stderr": stderr_path.name}
    problems = exit_problems(exited_early, returncode, killed) + log_problems(out, err)
    window = record["window"]
    if window["found"] is False:
        problems.append(f"no window of the editor appeared within {options.timeout} s ({window['method']})")
    elif window["found"] is None and options.require_window:
        problems.append("no window discovery method worked in this session")
    shot = record["screenshot"]
    if options.require_screenshot and not (shot and shot["ok"]):
        problems.append(f"the screenshot failed: {shot['error'] if shot else 'the editor was not running'}")
    record["problems"] = problems
    record["passed"] = not problems
    return record


def summary_line(record):
    window, memory = record["window"], record["memory"]
    launch = f"{window['launch_to_window_ms']} ms" if window["launch_to_window_ms"] is not None else "not measured"
    frame = record["editor"]["first_frame_us"]
    rss = memory["rss_kib"]
    return (f"| {record['label']} | {launch} ({window['method']}) | "
            f"{f'{frame / 1000:.1f} ms' if frame is not None else 'none'} | "
            f"{f'{rss / 1024:.1f} MiB' if rss is not None else 'unknown'} | {record['exit']['status']} | "
            f"{'passed' if record['passed'] else 'FAILED'} |")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--label", required=True, help="evidence file prefix, for example linux or macos")
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--sample", type=Path, required=True, help="the file the editor opens")
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--profile", type=Path, required=True, help="scratch folder for the editor's own data")
    parser.add_argument("--timeout", type=float, default=60.0, help="seconds to wait for the window")
    parser.add_argument("--settle", type=float, default=3.0, help="seconds between the window and the measurements")
    parser.add_argument("--grace", type=float, default=10.0, help="seconds between SIGTERM and SIGKILL")
    parser.add_argument("--alive-seconds", type=float, default=8.0,
                        help="how long the editor must stay up when no window discovery works")
    parser.add_argument("--require-window", action="store_true", help="fail when windows cannot be discovered")
    parser.add_argument("--require-screenshot", action="store_true")
    parser.add_argument("--summary", type=Path, help="append a Markdown row (GITHUB_STEP_SUMMARY)")
    options = parser.parse_args(argv)
    options.sample = options.sample.resolve()
    system = platform.system()
    record = smoke(options, window_finders(system), system=system)
    try:
        record["environment"] = current_environment()
    except ValueError as error:
        # An unexpected runner must not cost the measurements already taken.
        record["environment"] = {"invalid": str(error)}
    metrics = options.evidence / f"{options.label}-metrics.json"
    metrics.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(record, indent=2))
    annotate = os.environ.get("GITHUB_ACTIONS") == "true"
    if record["window"]["found"] is None:
        message = "No window discovery worked; only the process staying up was checked."
        print(f"::warning::{message}" if annotate else message)
    shot = record["screenshot"]
    if shot and not shot["ok"] and not options.require_screenshot:
        message = f"Screenshot unavailable: {shot['error']}"
        print(f"::warning::{message}" if annotate else message)
    for problem in record["problems"]:
        print(f"::error::{problem}" if annotate else f"FAILED: {problem}", file=sys.stderr)
    if options.summary:
        fresh = not options.summary.exists() or options.summary.stat().st_size == 0
        with options.summary.open("a", encoding="utf-8") as summary:
            if fresh:
                summary.write("| OS | Launch to window | Editor first frame | Resident set | Exit | Result |\n"
                              "| --- | --- | --- | --- | --- | --- |\n")
            summary.write(summary_line(record) + "\n")
    return 0 if record["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
