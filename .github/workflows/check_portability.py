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
# ADR-B: the application shell is platform-neutral; it reaches Windows only
# through its native seam, the single file below.
SHELL_ROOT = "apps/bareline/src/shell"
SHELL_WINDOWS_SEAM = "apps/bareline/src/shell/native/windows.rs"
# Nor may the shell pull a Win32 window handle out of winit itself: no HWND in
# any spelling and no Win32 raw window handle. Text inside string literals (a
# trace's JSON key, say) is data, not code, so it is ignored for this rule.
SHELL_SOURCE = re.compile(r'(?i:\bhwnd\b)|\bRawWindowHandle\s*::\s*Win32\b')
STRING_LITERAL = re.compile(r'\br(#*)".*?"\1|"(?:\\.|[^"\\])*"')

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
    return errors + inspect_shell(root)

def inspect_shell(root):
    """No Windows import or type in the shell (shell.rs and shell/**) outside its Windows seam."""
    errors = []
    shell = root / SHELL_ROOT
    for source in [shell.with_suffix(".rs"), *sorted(shell.rglob("*.rs"))]:
        if not source.is_file() or source == root / SHELL_WINDOWS_SEAM:
            continue
        for number, line in enumerate(source.read_text(encoding="utf-8-sig").splitlines(), 1):
            code = STRING_LITERAL.sub('""', line).split("//", 1)[0]
            if SOURCE.search(line.split("//", 1)[0]) or SHELL_SOURCE.search(code):
                errors.append(f"{source}:{number}: Windows import/type in the application shell outside {SHELL_WINDOWS_SEAM}")
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
        # The shell may name Windows only in its seam; comments are not code.
        seam = root / SHELL_WINDOWS_SEAM
        seam.parent.mkdir(parents=True)
        seam.write_text("pub use bareline_platform_windows::WindowsFileSystem as FileSystem;\n")
        (seam.parent / "unix.rs").write_text("// Unlike windows::core::Error, this error is portable.\n")
        assert len(inspect(root)) == 3
        (root / SHELL_ROOT / "views.rs").write_text("fn paint(window: HWND) {}\n")
        (root / SHELL_ROOT).with_suffix(".rs").write_text("use winit::platform::windows::EventLoopBuilderExtWindows;\n")
        failures = inspect(root)
        assert len(failures) == 5 and sum("application shell" in failure for failure in failures) == 2, failures
        # Window handles: lowercase hwnd and the Win32 raw handle are code;
        # the same words inside string literals are not.
        (root / SHELL_ROOT / "tray.rs").write_text(r'''let owner = handle.hwnd.get();
if let RawWindowHandle :: Win32(handle) = raw {}
let json = r#"{"hwnd":1}"#; let key = "\"hwnd\""; // HWND in a comment
''')
        failures = inspect(root)
        assert len(failures) == 7 and sum("application shell" in failure for failure in failures) == 4, failures
    errors = inspect(pathlib.Path(__file__).resolve().parents[2])
    print("\n".join(errors) if errors else "Neutral architecture guard passed (including negative fixtures).")
    sys.exit(bool(errors))


