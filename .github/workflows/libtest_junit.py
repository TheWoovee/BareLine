# SPDX-License-Identifier: MPL-2.0
"""Convert retained `cargo test` logs to JUnit XML without rerunning anything.

libtest writes per-test results to stdout and Cargo writes one `Running` or
`Doc-tests` line per test binary to stderr, in the same order. Tests that
re-run their own binary as a child (`--exact`) interleave the child's
`running 1 test` block with the parent's results; a block therefore closes only
on the summary whose counts match its `running N tests` line, and its failures
are the ones listed right before that summary. A binary that exits without its
summary (for example an abort) is reported as an error case.
"""

import argparse
import pathlib
import re
import sys
import tempfile
import xml.etree.ElementTree as ET

RUNNING = re.compile(r"^running (\d+) tests?$")
RESULT = re.compile(r"^test (?P<name>.+?) \.\.\. (?P<outcome>ok|FAILED|ignored)(?:, (?P<reason>.*))?$")
SUMMARY = re.compile(r"^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured;")
LISTED = re.compile(r"^    (?P<name>\S.*)$")
CAPTURE = re.compile(r"^---- (?P<name>.+?) std(?:out|err) ----$")
BINARY = re.compile(r"^\s*Running (?P<what>.+?) \((?P<path>[^()]+)\)\s*$")
DOCTESTS = re.compile(r"^\s*Doc-tests (?P<crate>\S+)\s*$")
HASH_SUFFIX = re.compile(r"-[0-9a-f]{16}$")
# Characters XML 1.0 cannot carry (terminal escapes, NUL) are dropped.
INVALID_XML = re.compile("[^" + "".join(map(chr, (9, 10, 13, 0x20, 0x2D, 0xD7FF, 0xE000, 0x2D, 0xFFFD, 0x10000, 0x2D, 0x10FFFF))) + "]")


def suite_labels(stderr_lines):
    labels = []
    for line in stderr_lines:
        if match := BINARY.match(line):
            stem = HASH_SUFFIX.sub("", pathlib.PureWindowsPath(match["path"]).stem)
            labels.append(f"{stem}: {match['what']}")
        elif match := DOCTESTS.match(line):
            labels.append(f"{match['crate']}: doctests")
    return labels


def parse(stdout_lines):
    suites, current, capture, listed = [], None, None, None
    for line in stdout_lines:
        if match := RUNNING.match(line):
            # A single-test block inside an open binary is its child re-run.
            if current is None or current["complete"] or int(match[1]) > 1:
                current = {"expected": int(match[1]), "cases": {}, "outputs": {}, "failed": set(), "complete": False}
                suites.append(current)
            capture = listed = None
        elif current is None:
            continue
        elif match := SUMMARY.match(line):
            if not current["complete"] and sum(int(value) for value in match.groups()) == current["expected"]:
                current["complete"] = True
                current["failed"] = listed or set()
            capture = listed = None
        elif match := CAPTURE.match(line):
            capture = current["outputs"].setdefault(match["name"], [])
            listed = None
        elif line == "failures:":
            capture, listed = None, set()
        elif capture is not None:
            capture.append(line)
        elif listed is not None and (match := LISTED.match(line)):
            listed.add(match["name"])
        elif match := RESULT.match(line):
            # Child re-runs repeat names; keep the first result per test.
            current["cases"].setdefault(match["name"], {"outcome": match["outcome"], "reason": match["reason"] or ""})
    for suite in suites:
        for name, case in suite["cases"].items():
            if suite["complete"]:
                # The listed failures are authoritative for a completed binary.
                case["outcome"] = "FAILED" if name in suite["failed"] else (
                    "ignored" if case["outcome"] == "ignored" else "ok")
            case["output"] = suite["outputs"].get(name, [])
    return suites


def clean(text):
    return INVALID_XML.sub("", text)


def junit(stdout_lines, stderr_lines):
    suites = parse(stdout_lines)
    labels = suite_labels(stderr_lines)
    if len(labels) != len(suites):
        labels = [f"test binary {index + 1}" for index in range(len(suites))]
    root = ET.Element("testsuites")
    totals = dict(tests=0, failures=0, errors=0, skipped=0)
    for label, suite in zip(labels, suites):
        counts = dict(tests=0, failures=0, errors=0, skipped=0)
        element = ET.SubElement(root, "testsuite", name=clean(label))
        for name, case in suite["cases"].items():
            counts["tests"] += 1
            node = ET.SubElement(element, "testcase", classname=clean(label), name=clean(name), time="0")
            if case["outcome"] == "FAILED":
                counts["failures"] += 1
                failure = ET.SubElement(node, "failure", message="test failed")
                failure.text = clean("\n".join(case["output"]).strip())
            elif case["outcome"] == "ignored":
                counts["skipped"] += 1
                ET.SubElement(node, "skipped", message=clean(case["reason"] or "ignored"))
        if not suite["complete"]:
            counts["tests"] += 1
            counts["errors"] += 1
            node = ET.SubElement(element, "testcase", classname=clean(label), name="(test binary result)", time="0")
            ET.SubElement(node, "error", message=(
                f"test binary exited after {len(suite['cases'])} of {suite['expected']} results without a summary"))
        for key, value in counts.items():
            element.set(key, str(value))
            totals[key] += value
    for key, value in totals.items():
        root.set(key, str(value))
    ET.indent(root)
    return ET.tostring(root, encoding="unicode", xml_declaration=True) + "\n", totals


def read_lines(path):
    return path.read_bytes().decode("utf-8", "replace").splitlines() if path else []


def self_test():
    stdout = [
        "running 5 tests", "test a::passes ... ok", "test a::fails ... FAILED", "",
        # a::spawns_child re-runs this binary; its child deliberately fails a::probe.
        "running 1 test", "test a::probe ... FAILED", "test a::skipped ... ignored, needs a desktop", "",
        "failures:", "", "---- a::probe stdout ----", "child-only failure", "", "failures:", "    a::probe", "",
        "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s", "",
        "test a::spawns_child ... ok", "test a::probe ... ok", "", "failures:", "",
        "---- a::fails stdout ----", "thread 'a::fails' panicked at src/lib.rs:3:5:", "boom \x1b[31m", "",
        "failures:", "    a::fails", "",
        "test result: FAILED. 3 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s", "",
        # b aborts without a summary; the next binary has more than one test.
        "running 2 tests", "test b::first ... ok",
        "running 2 tests", "test crates\\c\\src\\lib.rs - f (line 3) ... ok", "test crates\\c\\src\\lib.rs - g (line 9) ... ok",
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s",
    ]
    stderr = [
        "   Compiling a v0.1.0", "     Running unittests src\\lib.rs (target\\debug\\deps\\a-0123456789abcdef.exe)",
        "     Running tests\\b.rs (target\\debug\\deps\\b-fedcba9876543210.exe)",
        "error: test failed, to rerun pass `-p b --test b`", "   Doc-tests c",
    ]
    text, totals = junit(stdout, stderr)
    assert totals == dict(tests=9, failures=1, errors=1, skipped=1), totals
    root = ET.fromstring(text.encode("utf-8"))
    assert [suite.get("name") for suite in root] == ["a: unittests src\\lib.rs", "b: tests\\b.rs", "c: doctests"]
    failure = root.find("./testsuite/testcase[@name='a::fails']/failure")
    assert failure is not None and "panicked" in failure.text and "\x1b" not in failure.text
    assert root.find("./testsuite/testcase[@name='a::probe']/failure") is None, "a child re-run is not the parent result"
    assert root.find("./testsuite/testcase[@name='a::skipped']/skipped").get("message") == "needs a desktop"
    assert root[1].find("./testcase/error") is not None, "an aborted binary must be an error"
    # Unmatched stderr never mislabels suites.
    unlabeled, _ = junit(stdout, stderr[:2])
    assert [suite.get("name") for suite in ET.fromstring(unlabeled.encode("utf-8"))][0] == "test binary 1"
    empty, totals = junit([], ["error: could not compile `a`"])
    assert totals["tests"] == 0 and ET.fromstring(empty.encode("utf-8")).tag == "testsuites"
    with tempfile.TemporaryDirectory() as directory:
        log = pathlib.Path(directory) / "stdout.log"
        log.write_bytes("running 1 test\r\ntest é ... ok\r\n\r\ntest result: ok. 1 passed; 0 failed; 0 ignored; "
                        "0 measured; 0 filtered out; finished in 0.00s\r\n".encode("utf-8"))
        assert junit(read_lines(log), [])[1] == dict(tests=1, failures=0, errors=0, skipped=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stdout", type=pathlib.Path)
    parser.add_argument("--stderr", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    self_test()
    if arguments.self_test:
        print("libtest JUnit self-test passed.")
        return 0
    if not arguments.stdout or not arguments.output:
        parser.error("--stdout and --output are required")
    text, totals = junit(read_lines(arguments.stdout), read_lines(arguments.stderr))
    arguments.output.write_text(text, encoding="utf-8")
    print(f"JUnit report: {arguments.output} ({totals['tests']} tests, {totals['failures']} failed, "
          f"{totals['errors']} errors, {totals['skipped']} skipped)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
