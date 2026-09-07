# SPDX-License-Identifier: MPL-2.0
"""PR-022 architectural guardrail; run from any directory using Python 3.11+."""
import pathlib
import re
import sys
import tomllib
import tempfile

FORBIDDEN = {"windows", "windows-sys", "windows-core", "windows-numerics", "winapi", "bareline-platform-windows"}
ADAPTERS = {"platform-windows", "renderer-d2d"}
SOURCE = re.compile(r'\b(?:windows|windows_sys|windows_core|winapi)\s*::|\b(?:HWND|HANDLE|IDWrite\w*|ID2D1\w*)\b|extern\s+"system"')

def dependencies(table):
    for key, value in table.items():
        if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
            for alias, spec in value.items():
                yield alias, spec.get("package", alias) if isinstance(spec, dict) else alias
        elif isinstance(value, dict):
            yield from dependencies(value)

def inspect(root):
    errors = []
    for crate in sorted((root / "crates").iterdir()):
        if not crate.is_dir() or crate.name in ADAPTERS:
            continue
        manifest = crate / "Cargo.toml"
        if not manifest.exists():
            continue
        for alias, package in dependencies(tomllib.loads(manifest.read_text(encoding="utf-8-sig"))):
            if package in FORBIDDEN:
                errors.append(f"{manifest}: forbidden dependency {alias} ({package})")
        for source in crate.rglob("*.rs"):
            # This script is a conservative import/type guard, not a Rust parser.
            for number, line in enumerate(source.read_text(encoding="utf-8-sig").splitlines(), 1):
                code = line.split("//", 1)[0]
                if SOURCE.search(code.replace("std::os::windows::ffi::", "std_path_ffi::")):
                    errors.append(f"{source}:{number}: Windows import/type in neutral crate")
                if "std::os::windows" in code and source != root / "crates/platform/src/paths.rs":
                    errors.append(f"{source}:{number}: Windows std API outside lossless path codec")
    return errors

if __name__ == "__main__":
    # Exercise alias bypass and representative source leaks without modifying the checkout.
    assert list(dependencies({"target": {"cfg(windows)": {"dependencies": {"alias": {"package": "windows"}}}}})) == [("alias", "windows")]
    for leak in ["use windows::Win32;", "pub handle: HWND,", 'extern "system" {}', "windows_sys :: Win32"]:
        assert SOURCE.search(leak), leak
    with tempfile.TemporaryDirectory() as temp:
        root = pathlib.Path(temp)
        crate = root / "crates" / "neutral-fixture"
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text('[package]\nname="neutral-fixture"\n[target."cfg(windows)".dependencies]\nalias={package="windows", version="1"}\n')
        (crate / "src/lib.rs").write_text("pub struct Leak { pub handle: HWND }\n")
        failures = inspect(root)
        assert len(failures) == 2, failures
    errors = inspect(pathlib.Path(__file__).resolve().parents[2])
    print("\n".join(errors) if errors else "Neutral architecture guard passed (including negative fixtures).")
    sys.exit(bool(errors))


