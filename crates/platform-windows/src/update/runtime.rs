// SPDX-License-Identifier: MPL-2.0
//! One authority for online/offline native extension-host runtime installation.
use super::*;
use bareline_distribution::update::{TrustPolicy, verify_manifest};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub struct InstalledRuntime {
    pub executable: PathBuf,
    pub executable_sha256: [u8; 32],
    pub publisher_certificate_sha256: [u8; 32],
    pub version: String,
    pub metadata_version: u64,
    directory: PathBuf,
}
pub struct RuntimeDownload<'a> {
    pub host: &'a str,
    pub manifest_path: &'a str,
    pub signature_path: &'a str,
    pub artifact_path: &'a str,
    pub trust: &'a TrustPolicy<'a>,
    pub publisher_certificate_sha256: [u8; 32],
}
fn io(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("runtime: {error:?}"))
}
fn digest(text: &str) -> std::io::Result<[u8; 32]> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(io("invalid runtime digest"));
    }
    let mut bytes = [0; 32];
    for (index, value) in bytes.iter_mut().enumerate() {
        *value = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(io)?;
    }
    Ok(bytes)
}
fn bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_update_read_file(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io("receipt size"));
    }
    Ok(bytes)
}
fn policy_type(policy: &TrustPolicy<'_>) -> std::io::Result<()> {
    if policy.artifact_type != "bareline-exthost-x64" || policy.platform != "windows-x64" {
        return Err(io("runtime policy identity"));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub fn install_verified_runtime(
    executable_path: &Path,
    metadata_bytes: &[u8],
    signature_text: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
    extensions_root: &Path,
    cancel: &AtomicBool,
) -> std::io::Result<InstalledRuntime> {
    let mut executable = open_update_file(executable_path)?;
    install_held(
        &mut executable,
        metadata_bytes,
        signature_text,
        trust,
        now,
        publisher,
        extensions_root,
        cancel,
    )
}
#[allow(clippy::too_many_arguments)]
fn install_held(
    executable: &mut File,
    metadata: &[u8],
    signature: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
    root: &Path,
    cancel: &AtomicBool,
) -> std::io::Result<InstalledRuntime> {
    install_held_with(
        executable,
        metadata,
        signature,
        trust,
        now,
        publisher,
        root,
        cancel,
        &verify_authenticode,
    )
}
#[allow(clippy::too_many_arguments)]
fn install_held_with(
    executable: &mut File,
    metadata: &[u8],
    signature: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
    root: &Path,
    cancel: &AtomicBool,
    verify_publisher: &impl Fn(&File, &[u8; 32]) -> Result<(), UpdateError>,
) -> std::io::Result<InstalledRuntime> {
    policy_type(trust)?;
    validate_install_root(root)?;
    let manifest = verify_manifest(metadata, signature, trust, now).map_err(io)?;
    executable.seek(SeekFrom::Start(0))?;
    manifest.verify_package(executable).map_err(io)?;
    verify_publisher(executable, publisher).map_err(io)?;
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(io("cancelled"));
    }
    let runtimes = root.join("runtimes");
    match std::fs::create_dir(&runtimes) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    validate_install_root(&runtimes)?;
    let directory = runtimes.join(&manifest.metadata().sha256);
    std::fs::create_dir(&directory)?;
    // Create-new directory and receipt-last publication: failed copies remain inert evidence.
    let destination = directory.join("bareline-extension-host.exe");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    executable.seek(SeekFrom::Start(0))?;
    let mut buffer = [0; 65536];
    loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(io("cancelled"));
        }
        let count = executable.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
    }
    output.sync_all()?;
    drop(output);
    for (name, bytes) in [("runtime.minisig", signature.as_bytes()), ("runtime.json", metadata)] {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(name))?;
        out.write_all(bytes)?;
        out.sync_all()?;
    }
    let mut accepted = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("accepted-at"))?;
    writeln!(accepted, "{now}")?;
    accepted.sync_all()?;
    drop(accepted);
    restore_verified_runtime_with(
        root,
        &manifest.metadata().sha256,
        trust,
        now,
        publisher,
        verify_publisher,
    )
}

/// NONSHIPPING fixture boundary. Metadata signature, hash, cancellation and
/// path checks are production code; only Authenticode is replaced because the
/// generated local executable has no owner certificate.
#[cfg(feature = "fixture-release")]
#[allow(clippy::too_many_arguments)]
pub fn install_verified_runtime_nonshipping_fixture(
    executable_path: &Path,
    metadata_bytes: &[u8],
    signature_text: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
    extensions_root: &Path,
    cancel: &AtomicBool,
) -> std::io::Result<InstalledRuntime> {
    let mut executable = open_update_file(executable_path)?;
    install_held_with(
        &mut executable,
        metadata_bytes,
        signature_text,
        trust,
        now,
        publisher,
        extensions_root,
        cancel,
        &|_, observed| {
            if observed == publisher && observed == &[7; 32] {
                Ok(())
            } else {
                Err(UpdateError::Publisher)
            }
        },
    )
}
pub fn fetch_verified_runtime(
    download: &RuntimeDownload<'_>,
    extensions_root: &Path,
    now: u64,
    cancel: &AtomicBool,
) -> std::io::Result<InstalledRuntime> {
    policy_type(download.trust)?;
    let mut prepared = fetch_verified_update(
        download.host,
        download.manifest_path,
        download.signature_path,
        download.artifact_path,
        download.trust,
        now,
        &download.publisher_certificate_sha256,
        &std::env::temp_dir(),
        cancel,
    )
    .map_err(io)?;
    let result = install_held(
        &mut prepared.file,
        &prepared.metadata_bytes,
        &prepared.signature_text,
        download.trust,
        now,
        &download.publisher_certificate_sha256,
        extensions_root,
        cancel,
    );
    drop(prepared.file);
    let _ = std::fs::remove_file(prepared.directory.join("package.exe"));
    let _ = std::fs::remove_dir(prepared.directory);
    result
}
pub fn restore_verified_runtime(
    extensions_root: &Path,
    hash: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
) -> std::io::Result<InstalledRuntime> {
    restore_verified_runtime_with(extensions_root, hash, trust, now, publisher, &verify_authenticode)
}
fn restore_verified_runtime_with(
    extensions_root: &Path,
    hash: &str,
    trust: &TrustPolicy<'_>,
    now: u64,
    publisher: &[u8; 32],
    verify_publisher: &impl Fn(&File, &[u8; 32]) -> Result<(), UpdateError>,
) -> std::io::Result<InstalledRuntime> {
    policy_type(trust)?;
    let executable_sha256 = digest(hash)?;
    let directory = extensions_root.join("runtimes").join(hash);
    validate_install_root(&directory)?;
    let metadata = bounded(&directory.join("runtime.json"), 65536)?;
    let signature = bounded(&directory.join("runtime.minisig"), 8192)?;
    // An installed runtime remains usable offline. Freshness was required at acceptance;
    // current publisher/root pins and the caller's monotonic metadata floor still apply.
    let accepted = bounded(&directory.join("accepted-at"), 32)?;
    let accepted = std::str::from_utf8(&accepted)
        .map_err(io)?
        .trim()
        .parse::<u64>()
        .map_err(io)?;
    if accepted == 0 || accepted > now {
        return Err(io("invalid runtime acceptance time"));
    }
    let manifest =
        verify_manifest(&metadata, std::str::from_utf8(&signature).map_err(io)?, trust, accepted).map_err(io)?;
    if manifest.metadata().sha256 != hash {
        return Err(io("receipt directory mismatch"));
    }
    let executable = directory.join("bareline-extension-host.exe");
    let mut held = open_update_file(&executable)?;
    manifest.verify_package(&mut held).map_err(io)?;
    verify_publisher(&held, publisher).map_err(io)?;
    Ok(InstalledRuntime {
        executable,
        executable_sha256,
        publisher_certificate_sha256: *publisher,
        version: manifest.metadata().version.clone(),
        metadata_version: manifest.metadata().metadata_version,
        directory,
    })
}
pub fn remove_verified_runtime(runtime: &InstalledRuntime) -> std::io::Result<()> {
    validate_install_root(&runtime.directory)?;
    let expected = runtime.directory.join("bareline-extension-host.exe");
    let mut held = open_update_file(&expected)?;
    if update_file_sha256(&mut held)?
        != runtime
            .executable_sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    {
        return Err(io("runtime changed"));
    }
    verify_authenticode(&held, &runtime.publisher_certificate_sha256).map_err(io)?;
    drop(held);
    for name in [
        "runtime.json",
        "runtime.minisig",
        "accepted-at",
        "bareline-extension-host.exe",
    ] {
        let path = runtime.directory.join(name);
        let held = open_update_file(&path)?;
        drop(held);
        std::fs::remove_file(path)?;
    }
    std::fs::remove_dir(&runtime.directory)
}
