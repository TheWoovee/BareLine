// SPDX-License-Identifier: MPL-2.0
//! Explicit local helper. Trust pins are compiled by owner-controlled release builds.
mod build_capabilities {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/capability_assertion.rs"
    ));
}
fn main() {
    build_capabilities::retain();
    #[cfg(windows)]
    let result = run();
    #[cfg(not(windows))]
    let result: Result<(), String> = Err("Windows update helper is unavailable on this platform".into());
    if let Err(error) = result {
        eprintln!("update refused: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    use bareline_distribution::update::{TrustPolicy, verify_manifest};
    use bareline_platform_windows::update::{
        open_update_file, open_update_read_file, rename_update_handle, replace_with_rollback, update_file_sha256,
        verify_authenticode,
    };
    use std::io::{Read, Write};
    let action = std::env::args_os().skip(1).collect::<Vec<_>>();
    if action.is_empty()
        || !["--apply", "--recover", "--acknowledge"]
            .iter()
            .any(|a| action[0] == *a)
        || !((action.len() == 1 && action[0] != "--acknowledge")
            || (action.len() == 5
                && action[0] == "--acknowledge"
                && action[1] == "--healthy-pid"
                && action[3] == "--ready-event")
            || (action.len() == 5
                && action[0] == "--apply"
                && action[1] == "--wait-pid"
                && action[3] == "--ready-event"))
    {
        return Err("usage: bareline-update-helper --apply | --recover (editor must be closed)".into());
    }
    if env!("BARELINE_BUILD_MODE") == "preview" {
        return Err(
            "updates are disabled in this unsigned preview build; use a schema-validated configured release build"
                .into(),
        );
    }
    let key = env!("BARELINE_RELEASE_PUBLIC_KEY");
    let publisher = env!("BARELINE_PUBLISHER_CERT_SHA256");
    let channel = env!("BARELINE_RELEASE_CHANNEL");
    let embedded_floor = env!("BARELINE_METADATA_FLOOR")
        .parse::<u64>()
        .map_err(|_| "invalid embedded metadata floor")?;
    let mut certificate = [0_u8; 32];
    if publisher.len() != 64 || !publisher.is_ascii() {
        return Err("invalid compiled publisher fingerprint".into());
    }
    for (index, byte) in certificate.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&publisher[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid compiled publisher fingerprint")?;
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("helper has no installation directory")?;
    bareline_platform_windows::update::validate_install_root(root).map_err(|e| e.to_string())?;
    // Refuse reparse ancestors and network/device roots before reading installation files.
    use std::os::windows::fs::MetadataExt;
    if root.to_string_lossy().starts_with("\\\\") {
        return Err("network/device installation roots are not eligible for updates".into());
    }
    for ancestor in root.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("reparse installation root refused".into());
        }
    }
    let target = root.join("bareline.exe");
    let authority_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let authority = bareline_platform_windows::update::resolve_release_authority(
        root,
        key,
        publisher,
        embedded_floor,
        Some(bareline_platform_windows::update::OfflineRootPolicy {
            public_key: env!("BARELINE_OFFLINE_ROOT_PUBLIC_KEY"),
            minimum_version: env!("BARELINE_ROOT_VERSION_FLOOR")
                .parse::<u64>()
                .map_err(|_| "invalid offline root floor")?,
        }),
        authority_now,
    )
    .map_err(|e| e.to_string())?;
    let key = authority.release_public_key.as_str();
    let publisher = authority.publisher.as_str();
    let certificate = authority.certificate;
    let embedded_floor = authority.minimum_metadata_version;
    if action.len() == 5 && action[0] == "--apply" {
        let pid = action[2]
            .to_str()
            .ok_or("invalid parent PID")?
            .parse::<u32>()
            .map_err(|_| "invalid parent PID")?;
        bareline_platform_windows::update::wait_for_update_parent(
            pid,
            &target,
            action[4].to_str().ok_or("invalid ready event")?,
        )
        .map_err(|e| e.to_string())?;
    }
    let backup = root.join("bareline.rollback.exe");
    let _installation_lock =
        bareline_platform_windows::update::lock_update_installation(root).map_err(|e| e.to_string())?;
    let journal_path = root.join("bareline.update-journal");
    if action[0] == "--acknowledge" {
        let pid = action[2]
            .to_str()
            .ok_or("invalid healthy PID")?
            .parse::<u32>()
            .map_err(|_| "invalid healthy PID")?;
        let _healthy_process =
            bareline_platform_windows::update::hold_healthy_update_process(pid, &target).map_err(|e| e.to_string())?;
        bareline_platform_windows::update::signal_update_parent_ready(
            pid,
            action[4].to_str().ok_or("invalid ready event")?,
        )
        .map_err(|e| e.to_string())?;
        if !journal_path.try_exists().map_err(|e| e.to_string())? {
            return Ok(());
        }
        let receipt = bounded(&journal_path, 1024)?;
        let receipt = std::str::from_utf8(&receipt).map_err(|_| "invalid receipt")?;
        let new_hash = receipt
            .lines()
            .find_map(|l| l.strip_prefix("new_sha256="))
            .ok_or("missing new hash")?;
        let mut current = open_update_read_file(&target).map_err(|e| e.to_string())?;
        if update_file_sha256(&mut current).map_err(|e| e.to_string())? != new_hash {
            return Err("healthy target differs from receipt".into());
        }
        verify_authenticode(&current, &certificate).map_err(|e| format!("healthy publisher: {e:?}"))?;
        // Retain every generation by default; never delete old binaries or receipts.
        // Journal is archived last, making interrupted acknowledgement retryable.
        bareline_platform_windows::update::retain_update_evidence(root).map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Reconcile a durable intent after helper crash without changing executable layout.
    if journal_path.try_exists().map_err(|e| e.to_string())? {
        let receipt = bounded(&journal_path, 1024)?;
        let receipt = std::str::from_utf8(&receipt).map_err(|_| "invalid update receipt")?;
        let old_hash = receipt
            .lines()
            .find_map(|line| line.strip_prefix("old_sha256="))
            .ok_or("receipt has no old hash")?;
        let new_hash = receipt
            .lines()
            .find_map(|line| line.strip_prefix("new_sha256="))
            .ok_or("receipt has no new hash")?;
        if target.try_exists().map_err(|e| e.to_string())? {
            let mut current = open_update_file(&target).map_err(|e| e.to_string())?;
            let hash = update_file_sha256(&mut current).map_err(|e| e.to_string())?;
            if hash != old_hash && hash != new_hash {
                return Err("receipt target differs; retain backup for review".into());
            }
            if hash == new_hash {
                verify_authenticode(&current, &certificate).map_err(|e| format!("recovery trust: {e:?}"))?;
                if action[0] == "--recover" {
                    let mut old = open_update_file(&backup).map_err(|e| e.to_string())?;
                    if update_file_sha256(&mut old).map_err(|e| e.to_string())? != old_hash {
                        return Err("backup differs from receipt".into());
                    }
                    verify_authenticode(&old, &certificate).map_err(|e| format!("backup trust: {e:?}"))?;
                    replace_with_rollback(&old, current, &target, &root.join("bareline.failed.exe"))
                        .map_err(|e| e.to_string())?;
                    drop(old);
                    bareline_platform_windows::update::retain_update_evidence(root).map_err(|e| e.to_string())?;
                }
            } else {
                // Original target survived an interrupted apply. Authenticate it,
                // preserve the failed attempt and allow a fresh explicit check.
                verify_authenticode(&current, &certificate).map_err(|e| format!("original trust: {e:?}"))?;
                drop(current);
                bareline_platform_windows::update::retain_update_evidence(root).map_err(|e| e.to_string())?;
            }
        } else {
            let mut old = open_update_file(&backup).map_err(|e| e.to_string())?;
            if update_file_sha256(&mut old).map_err(|e| e.to_string())? != old_hash {
                return Err("backup differs from receipt".into());
            }
            verify_authenticode(&old, &certificate).map_err(|e| format!("recovery trust: {e:?}"))?;
            rename_update_handle(&old, &target).map_err(|e| e.to_string())?;
            drop(old);
            bareline_platform_windows::update::retain_update_evidence(root).map_err(|e| e.to_string())?;
        }
        // Retain the journal until the new app explicitly acknowledges healthy startup.
        return Ok(());
    }
    if action[0] == "--recover" {
        let old = open_update_file(&backup).map_err(|e| e.to_string())?;
        verify_authenticode(&old, &certificate).map_err(|e| format!("backup trust: {e:?}"))?;
        if target.try_exists().map_err(|e| e.to_string())? {
            let current = open_update_file(&target).map_err(|e| e.to_string())?;
            replace_with_rollback(&old, current, &target, &root.join("bareline.failed.exe"))
                .map_err(|e| e.to_string())?;
        } else {
            rename_update_handle(&old, &target).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    fn bounded(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
        let file = open_update_file(path).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit {
            return Err("metadata limit".into());
        }
        Ok(bytes)
    }
    let ledger_path = root.join("bareline.update-versions");
    let mut highest = embedded_floor;
    match bounded(&ledger_path, 64 * 1024) {
        Ok(bytes) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| "invalid version ledger")?;
            for line in text.lines() {
                highest = highest.max(line.parse::<u64>().map_err(|_| "invalid version ledger")?);
            }
        }
        Err(_) if !ledger_path.try_exists().map_err(|e| e.to_string())? => (),
        Err(error) => return Err(error),
    }
    let manifest = bounded(&root.join("bareline.update.json"), 64 * 1024)?;
    let signature = bounded(&root.join("bareline.update.minisig"), 8192)?;
    let signature = std::str::from_utf8(&signature).map_err(|_| "invalid signature encoding")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "clock unavailable")?
        .as_secs();
    let policy = TrustPolicy {
        release_public_key: key,
        channel,
        artifact_type: "bareline-executable-x64",
        platform: "windows-x64",
        publisher,
        protocol: 1,
        highest_metadata_version: highest,
        maximum_package_bytes: 256 * 1024 * 1024,
    };
    let verified = verify_manifest(&manifest, signature, &policy, now).map_err(|e| format!("manifest: {e:?}"))?;
    let mut staged = open_update_file(&root.join("bareline.pending.exe")).map_err(|e| e.to_string())?;
    verified
        .verify_package(&mut staged)
        .map_err(|e| format!("package: {e:?}"))?;
    verify_authenticode(&staged, &certificate).map_err(|e| format!("publisher: {e:?}"))?;
    let mut current = open_update_file(&target).map_err(|e| e.to_string())?;
    let old_hash = update_file_sha256(&mut current).map_err(|e| e.to_string())?;
    // Durable monotonic floor before application. A failed apply may retry equal metadata.
    use std::os::windows::fs::OpenOptionsExt;
    let mut ledger = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .share_mode(0)
        .custom_flags(0x00200000)
        .open(&ledger_path)
        .map_err(|e| e.to_string())?;
    if ledger.metadata().map_err(|e| e.to_string())?.file_attributes() & 0x400 != 0 {
        return Err("reparse ledger refused".into());
    }
    writeln!(ledger, "{}", verified.metadata().metadata_version).map_err(|e| e.to_string())?;
    ledger.sync_all().map_err(|e| e.to_string())?;
    // Durable intent before backup copy and atomic replacement. Existing intent is reviewed/recovered,
    // never overwritten; a crash after apply retains backup and intent evidence.
    let mut journal = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&journal_path)
        .map_err(|e| e.to_string())?;
    writeln!(
        journal,
        "schema_version=1\nold_sha256={old_hash}\nnew_sha256={}\nmetadata_version={}",
        verified.metadata().sha256,
        verified.metadata().metadata_version
    )
    .map_err(|e| e.to_string())?;
    journal.sync_all().map_err(|e| e.to_string())?;
    drop(journal);
    replace_with_rollback(&staged, current, &target, &backup).map_err(|e| e.to_string())?;
    // The running app acknowledges this receipt after a successful normal frame.
    Ok(())
}
