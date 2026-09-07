# SPDX-License-Identifier: MPL-2.0
"""PR-022 architectural guardrail; run from any directory using Python 3.11+."""
import pathlib
import re
import sys
import tomllib
import tempfile

FORBIDDEN = {"windows", "windows-sys", "windows-core", "windows-numerics", "winapi", "accesskit_windows", "bareline-platform-windows", "bareline-renderer-d2d"}
ADAPTERS = {"crates/platform-windows", "crates/renderer-d2d"}
# Executable composition roots may wire cfg-gated adapters. Their portability is
# checked by the real native compiler jobs, rather than this lexical core rule.
COMPOSITION_ROOTS = {"apps/bareline", "apps/extension-host", "apps/update-helper", "xtask"}
SOURCE = re.compile(r'\b(?:windows|windows_sys|windows_core|windows_numerics|winapi|bareline_platform_windows)\s*::|\b(?:HWND|HANDLE|IDWrite\w*|ID2D1\w*)\b|extern\s+"system"')

def dependencies(table, workspace=None, context=()):
    for key, value in table.items():
        if key in {"dependencies", "dev-dependencies", "build-dependencies"}:
            for alias, spec in value.items():
                if isinstance(spec, dict) and spec.get("workspace"):
                    spec = (workspace or {}).get(alias, {})
                yield alias, spec.get("package", alias) if isinstance(spec, dict) else alias, context + (key,)
        elif isinstance(value, dict):
            yield from dependencies(value, workspace, context + (key,))

def inspect(root):
    errors = []
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8-sig"))["workspace"]
    members = {path for pattern in workspace["members"] for path in root.glob(pattern)}
    excluded = {path for pattern in workspace.get("exclude", []) for path in root.glob(pattern)}
    for crate in sorted(members - excluded):
        relative = crate.relative_to(root).as_posix()
        if relative in ADAPTERS | COMPOSITION_ROOTS:
            continue
        manifest = crate / "Cargo.toml"
        if not manifest.exists():
            continue
        for alias, package, context in dependencies(tomllib.loads(manifest.read_text(encoding="utf-8-sig")), workspace.get("dependencies", {})):
            if (relative == "crates/search" and alias == package == "bareline-platform-windows"
                    and context == ("target", "cfg(windows)", "dev-dependencies")):
                continue
            if package in FORBIDDEN:
                errors.append(f"{manifest}: forbidden dependency {alias} ({package})")
        for source in crate.rglob("*.rs"):
            contents = source.read_text(encoding="utf-8-sig")
            # Exact existing native test fixture only. The compiler enforces the
            # test/windows cfg; a production dependency never receives this waiver.
            if source == root / "crates/search/src/replace_disk.rs":
                contents = contents.replace(
                    "#[cfg(all(test, windows))]\nmod tests {\n    use super::*;\n    use bareline_platform_windows::{WindowsFileSystem, WindowsPathTrustProvider};",
                    "#[cfg(all(test, windows))]\nmod tests {\n    use super::*;")
            # This script is a conservative import/type guard, not a Rust parser.
            for number, line in enumerate(contents.splitlines(), 1):
                code = line.split("//", 1)[0]
                if SOURCE.search(code.replace("std::os::windows::ffi::", "std_path_ffi::")):
                    errors.append(f"{source}:{number}: Windows import/type in neutral crate")
                if "std::os::windows" in code and source != root / "crates/platform/src/paths.rs":
                    errors.append(f"{source}:{number}: Windows std API outside lossless path codec")
    return errors

if __name__ == "__main__":
    # Exercise alias bypass and representative source leaks without modifying the checkout.
    assert list(dependencies({"target": {"cfg(windows)": {"dependencies": {"alias": {"package": "windows"}}}}})) == [("alias", "windows", ("target", "cfg(windows)", "dependencies"))]
    assert list(dependencies({"dependencies": {"alias": {"workspace": True}}}, {"alias": {"package": "windows"}}))[0][1] == "windows"
    for leak in ["use windows::Win32;", "pub handle: HWND,", 'extern "system" {}', "windows_sys :: Win32"]:
        assert SOURCE.search(leak), leak
    with tempfile.TemporaryDirectory() as temp:
        root = pathlib.Path(temp)
        (root / "Cargo.toml").write_text('[workspace]\nmembers=["crates/*", "extensions/*"]\n[workspace.dependencies]\nalias={package="windows",version="1"}\n')
        crate = root / "crates" / "neutral-fixture"
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text('[package]\nname="neutral-fixture"\n[target."cfg(windows)".dependencies]\nalias={package="windows", version="1"}\n')
        (crate / "src/lib.rs").write_text("pub struct Leak { pub handle: HWND }\n")
        failures = inspect(root)
        assert len(failures) == 2, failures
        (crate / "Cargo.toml").write_text('[package]\nname="neutral-fixture"\n[dependencies]\nalias.workspace=true\n')
        assert len(inspect(root)) == 2
        extension = root / "extensions" / "fixture"
        extension.mkdir(parents=True)
        (extension / "Cargo.toml").write_text('[package]\nname="extension-fixture"\n[build-dependencies]\nwindows="1"\n')
        assert len(inspect(root)) == 3
    errors = inspect(pathlib.Path(__file__).resolve().parents[2])
    print("\n".join(errors) if errors else "Neutral architecture guard passed (including negative fixtures).")
    sys.exit(bool(errors))


