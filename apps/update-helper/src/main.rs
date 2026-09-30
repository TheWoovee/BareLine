// SPDX-License-Identifier: MPL-2.0
//! Explicit local helper. Trust pins are compiled by owner-controlled release builds.
mod build_capabilities {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/capability_assertion.rs"
    ));
}
fn main() {
    // First: on-demand DLL loads may only come from System32, never the install root (SEC-16).
    #[cfg(windows)]
    if let Err(error) = bareline_platform_windows::shell_integration::restrict_dll_search_to_system32() {
        eprintln!("update refused: {error}");
        std::process::exit(1);
    }
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
    use bareline_distribution::update::{PublisherPin, core_update_policy, verify_manifest};
    use bareline_platform_windows::update::{
        self as native, AuthorityFreshness, Revocation, open_update_file, open_update_read_file, rename_update_handle,
        replace_with_rollback, update_file_sha256, verify_authenticode,
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
                && action[0] != "--acknowledge"
                && action[1] == "--wait-pid"
                && action[3] == "--ready-event"))
    {
        return Err("usage: bareline-update-helper --apply | --recover (editor must be closed)".into());
    }
    // Preview builds have no trust; fixture builds compile public private-seed keys (SEC-18).
    if env!("BARELINE_BUILD_MODE") != "configured" {
        return Err(
            "updates are disabled in preview and nonshipping fixture builds; use a schema-validated configured release build"
                .into(),
        );
    }
    let key = env!("BARELINE_RELEASE_PUBLIC_KEY");
    // `trust.publisher` is the manifest identity; the Authenticode pin is separate (SEC-01).
    let publisher = env!("BARELINE_PUBLISHER");
    let embedded_signer = PublisherPin::parse(
        env!("BARELINE_AUTHENTICODE_SUBJECT"),
        env!("BARELINE_AUTHENTICODE_ISSUERS"),
    )
    .map_err(|_| "invalid compiled Authenticode publisher pin")?;
    let channel = env!("BARELINE_RELEASE_CHANNEL");
    let embedded_floor = env!("BARELINE_METADATA_FLOOR")
        .parse::<u64>()
        .map_err(|_| "invalid embedded metadata floor")?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("helper has no installation directory")?;
    native::validate_install_root(root).map_err(|e| e.to_string())?;
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
    // Ledgers and the lock are per-user state; the installation is read (SEC-04).
    let state = native::update_state_root(root).map_err(|e| e.to_string())?;
    {
        // A trust install interrupted by a crash is rolled back before the authority is read.
        let _lock = native::lock_update_installation(&state).map_err(|e| e.to_string())?;
        native::reconcile_trust_install(root).map_err(|e| format!("trust reconciliation: {e}"))?;
    }
    let authority_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let offline_policy = native::OfflineRootPolicy {
        public_key: env!("BARELINE_OFFLINE_ROOT_PUBLIC_KEY"),
        minimum_version: env!("BARELINE_ROOT_VERSION_FLOOR")
            .parse::<u64>()
            .map_err(|_| "invalid offline root floor")?,
    };
    // Applying accepts new metadata; acknowledgement and recovery use installed state,
    // which an expired authority never disables (SEC-02).
    let freshness = if action[0] == "--apply" {
        AuthorityFreshness::Required
    } else {
        AuthorityFreshness::Installed
    };
    let authority = native::resolve_release_authority(
        root,
        &state,
        key,
        &embedded_signer,
        embedded_floor,
        Some(offline_policy),
        freshness,
        authority_now,
    )
    .map_err(|e| e.to_string())?;
    let key = authority.release_public_key.as_str();
    let signer = &authority.signer;
    if action.len() == 5 && action[1] == "--wait-pid" {
        let pid = action[2]
            .to_str()
            .ok_or("invalid parent PID")?
            .parse::<u32>()
            .map_err(|_| "invalid parent PID")?;
        native::wait_for_update_parent(pid, &target, action[4].to_str().ok_or("invalid ready event")?)
            .map_err(|e| e.to_string())?;
    }
    let backup = root.join("bareline.rollback.exe");
    let _installation_lock = native::lock_update_installation(&state).map_err(|e| e.to_string())?;
    let journal_path = root.join("bareline.update-journal");
    if action[0] == "--acknowledge" {
        let pid = action[2]
            .to_str()
            .ok_or("invalid healthy PID")?
            .parse::<u32>()
            .map_err(|_| "invalid healthy PID")?;
        let _healthy_process = native::hold_healthy_update_process(pid, &target).map_err(|e| e.to_string())?;
        native::signal_update_parent_ready(pid, action[4].to_str().ok_or("invalid ready event")?)
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
        // Launch-time acknowledgement: the receipt hash pins the file, no online revocation (SEC-07).
        verify_authenticode(&current, signer, Revocation::Offline).map_err(|e| format!("healthy publisher: {e:?}"))?;
        // Journal is archived last, making interrupted acknowledgement retryable.
        native::retain_update_evidence(root).map_err(|e| e.to_string())?;
        // Healthy: the failed-launch count ends (SEC-09) and only the newest generations
        // of retained evidence stay (SEC-17); pruning never fails the acknowledgement.
        native::clear_update_launch_attempts(&state).map_err(|e| e.to_string())?;
        let _ = native::prune_retained_evidence(root, native::RETAINED_GENERATIONS);
        return Ok(());
    }
    // Reconcile a durable intent after helper crash without changing executable layout.
    // Recovery must work offline: the receipt hashes pin both files (SEC-07).
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
                verify_authenticode(&current, signer, Revocation::Offline)
                    .map_err(|e| format!("recovery trust: {e:?}"))?;
                if action[0] == "--recover" {
                    let mut old = open_update_file(&backup).map_err(|e| e.to_string())?;
                    if update_file_sha256(&mut old).map_err(|e| e.to_string())? != old_hash {
                        return Err("backup differs from receipt".into());
                    }
                    verify_authenticode(&old, signer, Revocation::Offline)
                        .map_err(|e| format!("backup trust: {e:?}"))?;
                    replace_with_rollback(&old, current, &target, &root.join("bareline.failed.exe"))
                        .map_err(|e| e.to_string())?;
                    drop(old);
                    native::retain_update_evidence(root).map_err(|e| e.to_string())?;
                    native::clear_update_launch_attempts(&state).map_err(|e| e.to_string())?;
                }
            } else {
                // Original target survived an interrupted apply. Authenticate it,
                // preserve the failed attempt and allow a fresh explicit check.
                verify_authenticode(&current, signer, Revocation::Offline)
                    .map_err(|e| format!("original trust: {e:?}"))?;
                drop(current);
                native::retain_update_evidence(root).map_err(|e| e.to_string())?;
            }
        } else {
            let mut old = open_update_file(&backup).map_err(|e| e.to_string())?;
            if update_file_sha256(&mut old).map_err(|e| e.to_string())? != old_hash {
                return Err("backup differs from receipt".into());
            }
            verify_authenticode(&old, signer, Revocation::Offline).map_err(|e| format!("recovery trust: {e:?}"))?;
            rename_update_handle(&old, &target).map_err(|e| e.to_string())?;
            drop(old);
            native::retain_update_evidence(root).map_err(|e| e.to_string())?;
        }
        // Retain the journal until the new app explicitly acknowledges healthy startup.
        return Ok(());
    }
    if action[0] == "--recover" {
        // Without a journal only the last applied update can be undone, to the exact
        // retained build its ledger entry names, and only while its build runs. A bare
        // backup is never restored: Authenticode alone never authorizes an executable
        // (SEC-08), and older generations stay unreachable (SEC-09).
        if root
            .join("bareline.failed.exe")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            native::retain_update_evidence(root).map_err(|e| e.to_string())?;
        }
        let Some(native::RecoverySource::Applied(previous)) =
            native::recovery_source(root, &state).map_err(|e| e.to_string())?
        else {
            return Err("recovery requires the update journal or an applied-update ledger entry for the running build; nothing is restored".into());
        };
        verify_authenticode(&previous, signer, Revocation::Offline)
            .map_err(|e| format!("previous build trust: {e:?}"))?;
        let current = open_update_file(&target).map_err(|e| e.to_string())?;
        replace_with_rollback(&previous, current, &target, &root.join("bareline.failed.exe"))
            .map_err(|e| e.to_string())?;
        drop(previous);
        native::retain_update_evidence(root).map_err(|e| e.to_string())?;
        native::clear_update_launch_attempts(&state).map_err(|e| e.to_string())?;
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
    // The core executable's own floor and ledger (SEC-03), never the runtime's or catalog's.
    let highest =
        native::core_metadata_floor(root, &state, authority.minimum_metadata_version).map_err(|e| e.to_string())?;
    let manifest = bounded(&root.join("bareline.update.json"), 64 * 1024)?;
    let signature = bounded(&root.join("bareline.update.minisig"), 8192)?;
    let signature = std::str::from_utf8(&signature).map_err(|_| "invalid signature encoding")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "clock unavailable")?
        .as_secs();
    let policy = core_update_policy(key, publisher, channel, highest);
    let verified = verify_manifest(&manifest, signature, &policy, now).map_err(|e| format!("manifest: {e:?}"))?;
    let mut staged = open_update_file(&root.join("bareline.pending.exe")).map_err(|e| e.to_string())?;
    verified
        .verify_package(&mut staged)
        .map_err(|e| format!("package: {e:?}"))?;
    // Applying is an explicit user action: require current revocation evidence (SEC-07).
    verify_authenticode(&staged, signer, Revocation::Online).map_err(|e| format!("publisher: {e:?}"))?;
    // Trust state the signed manifest delivers is reverified and installed first, with
    // rollback on failure; it stays valid whether or not the executable swap succeeds (SEC-02).
    if let Some(delivered) =
        native::verify_delivered_trust(root, root, &state, verified.metadata(), offline_policy, &staged, now)
            .map_err(|e| format!("delivered trust: {e}"))?
    {
        native::install_delivered_trust(root, delivered).map_err(|e| format!("trust install: {e}"))?;
    }
    let mut current = open_update_file(&target).map_err(|e| e.to_string())?;
    let old_hash = update_file_sha256(&mut current).map_err(|e| e.to_string())?;
    // Durable monotonic floor before application. A failed apply may retry equal metadata.
    native::record_core_metadata_version(&state, verified.metadata().metadata_version).map_err(|e| e.to_string())?;
    // Durable intent before backup copy and atomic replacement. Existing intent is reviewed/recovered,
    // never overwritten; a crash after apply retains backup and intent evidence.
    use std::os::windows::fs::OpenOptionsExt;
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
    // After the acknowledgement archives the journal, this entry still lets the user
    // roll the update back to exactly the build it replaced (SEC-09).
    native::record_applied_update(&state, &old_hash, &verified.metadata().sha256).map_err(|e| e.to_string())?;
    replace_with_rollback(&staged, current, &target, &backup).map_err(|e| e.to_string())?;
    // The running app acknowledges this receipt after a successful normal frame.
    Ok(())
}
