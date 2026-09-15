// SPDX-License-Identifier: MPL-2.0
//! Embeds the Windows manifest, the product icon and VERSIONINFO.
//!
//! The resource script is compiled with the Windows SDK `rc.exe` rather than a
//! build dependency, so the crate graph stays as it is. If `rc.exe` cannot be
//! found the build still succeeds; only the icon and version block are missing.
use std::path::{Path, PathBuf};

#[path = "../../build-support/release_config.rs"]
mod release_config;

const ICON: &str = "docs/blueprint/mockups/logo-brand/bareline.ico";

fn main() {
    release_config::configure("editor");
    println!("cargo:rerun-if-changed=windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let manifest = manifest_dir.join("windows.manifest");
    println!("cargo:rustc-link-arg-bin=bareline=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=bareline=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!("cargo:rustc-link-arg-bin=bareline=/MANIFESTUAC:level='asInvoker' uiAccess='false'");

    let icon = manifest_dir
        .join("../..")
        .join(ICON)
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../..").join(ICON));
    println!("cargo:rerun-if-changed={}", icon.display());
    if let Some(res) = compile_resources(&icon) {
        println!("cargo:rustc-link-arg-bin=bareline={}", res.display());
    } else {
        println!("cargo:warning=rc.exe not found; bareline.exe has no icon or version resource");
    }
}

/// Writes `bareline.rc` into `OUT_DIR` and compiles it. Returns the `.res` path.
fn compile_resources(icon: &Path) -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    let version = std::env::var("CARGO_PKG_VERSION").ok()?;
    let mut parts = version
        .split(['.', '-', '+'])
        .filter_map(|part| part.parse::<u16>().ok());
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    // Backslashes are escapes inside an .rc string literal.
    let icon_literal = icon.display().to_string().replace('\\', "\\\\");
    let script = format!(
        r#"1 ICON "{icon_literal}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Bareline"
            VALUE "FileDescription", "Bareline text editor"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "bareline"
            VALUE "LegalCopyright", "Licensed under MPL-2.0"
            VALUE "OriginalFilename", "bareline.exe"
            VALUE "ProductName", "Bareline"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let script_path = out_dir.join("bareline.rc");
    std::fs::write(&script_path, script).ok()?;
    let res_path = out_dir.join("bareline.res");
    let rc = find_rc()?;
    let status = std::process::Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&script_path)
        .status()
        .ok()?;
    status.success().then_some(res_path)
}

/// Prefers `BARELINE_RC` (release builds pin the SDK), then the newest
/// `rc.exe` under the installed Windows 10/11 SDK for the host architecture.
fn find_rc() -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=BARELINE_RC");
    if let Some(explicit) = std::env::var_os("BARELINE_RC") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    let host_arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let roots = [std::env::var_os("ProgramFiles(x86)"), std::env::var_os("ProgramFiles")];
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots.into_iter().flatten() {
        let bin = PathBuf::from(root).join("Windows Kits/10/bin");
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let candidate = entry.path().join(host_arch).join("rc.exe");
            if candidate.exists() {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort();
    candidates.pop()
}
