# SPDX-License-Identifier: MPL-2.0
"""Validate the compiler contract without invoking Cargo."""

import argparse
import json
import pathlib
import re
import sys
import tomllib


VERSION = re.compile(r"^(\d+)\.(\d+)(?:\.(\d+))?$")
TOOLCHAIN_DECLARATION = re.compile(
    r"^\s*toolchain:\s*['\"]?(\d+\.\d+(?:\.\d+)?)['\"]?\s*$",
    re.MULTILINE,
)


def version(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f"invalid Rust version {value!r}")
    return tuple(int(part or 0) for part in match.groups())


def inherited(package, field):
    value = package.get(field)
    return isinstance(value, dict) and value.get("workspace") is True


def member_errors(package, relative, edition):
    errors = []
    if not inherited(package, "rust-version"):
        errors.append(f"{relative}: package.rust-version must inherit workspace policy")
    if not inherited(package, "edition"):
        errors.append(
            f"{relative}: package.edition must inherit workspace edition {edition}"
        )
    return errors


def dependency_errors(packages, supported):
    errors = []
    for package in packages:
        requirement = package.get("rust_version")
        if requirement and version(requirement) > version(supported):
            errors.append(
                f"resolved dependency {package['name']} {package['version']} requires "
                f"Rust {requirement}, newer than supported {supported}"
            )
    return errors


def workspace_members(root, workspace):
    members = {
        path
        for pattern in workspace["members"]
        for path in root.glob(pattern)
        if (path / "Cargo.toml").is_file()
    }
    excluded = {
        path
        for pattern in workspace.get("exclude", [])
        for path in root.glob(pattern)
    }
    return sorted(members - excluded)


def inspect(root, metadata=None):
    errors = []
    root_manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8-sig"))
    workspace = root_manifest["workspace"]
    package_policy = workspace["package"]
    supported = package_policy["rust-version"]
    edition = package_policy["edition"]
    pinned = tomllib.loads(
        (root / "rust-toolchain.toml").read_text(encoding="utf-8-sig")
    )["toolchain"]["channel"]

    if version(supported) != version(pinned):
        errors.append(
            f"workspace rust-version {supported} differs from pinned toolchain {pinned}"
        )

    members = workspace_members(root, workspace)
    for member in members:
        manifest = member / "Cargo.toml"
        package = tomllib.loads(manifest.read_text(encoding="utf-8-sig"))["package"]
        relative = manifest.relative_to(root).as_posix()
        errors.extend(member_errors(package, relative, edition))

    workflows = [
        *(root / ".github/workflows").glob("*.yml"),
        *(root / ".github/workflows").glob("*.yaml"),
    ]
    for workflow in sorted(workflows):
        for declared in TOOLCHAIN_DECLARATION.findall(
            workflow.read_text(encoding="utf-8-sig")
        ):
            if version(declared) != version(supported):
                errors.append(
                    f"{workflow.relative_to(root).as_posix()}: explicit toolchain "
                    f"{declared} differs from supported {supported}"
                )

    if metadata is not None:
        resolved = json.loads(metadata.read_text(encoding="utf-8-sig"))
        errors.extend(dependency_errors(resolved["packages"], supported))

    return errors, len(members), supported, edition


if __name__ == "__main__":
    assert member_errors({"edition": {"workspace": True}}, "missing/Cargo.toml", "2024")
    assert member_errors(
        {"edition": "2024", "rust-version": {"workspace": True}},
        "explicit/Cargo.toml",
        "2024",
    )
    assert dependency_errors(
        [{"name": "future", "version": "1.0.0", "rust_version": "1.99"}],
        "1.98.1",
    )
    assert not dependency_errors(
        [{"name": "supported", "version": "1.0.0", "rust_version": "1.98.1"}],
        "1.98.1",
    )
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--metadata",
        type=pathlib.Path,
        help="optional cargo metadata JSON used to check resolved dependency requirements",
    )
    arguments = parser.parse_args()
    repository = pathlib.Path(__file__).resolve().parents[2]
    failures, count, supported, edition = inspect(repository, arguments.metadata)
    if failures:
        print("\n".join(failures))
    else:
        suffix = " and resolved dependency requirements" if arguments.metadata else ""
        print(
            f"Toolchain contract passed: {count} members inherit Rust {supported} "
            f"and edition {edition}{suffix}; negative fixtures passed."
        )
    sys.exit(bool(failures))
