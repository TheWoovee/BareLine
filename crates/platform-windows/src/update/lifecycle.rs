// SPDX-License-Identifier: MPL-2.0
//! Update trust-state lifecycle: per-user state and ledgers (SEC-04), per-artifact
//! floors (SEC-03), trust state delivered with core updates (SEC-02), failed-launch
//! recovery (SEC-09) and bounded retention of update evidence (SEC-17).
use super::*;
use bareline_distribution::update::{Manifest, sha256_hex};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub(super) const RELEASE_AUTHORITY: &str = "bareline.release-authority.json";
pub(super) const RELEASE_AUTHORITY_SIGNATURE: &str = "bareline.release-authority.minisig";
pub(super) const ROOT_TRANSITIONS: &str = "bareline.root-transitions.json";
const UPDATE_HELPER: &str = "bareline-update-helper.exe";
const PENDING_AUTHORITY: &str = "bareline.pending-authority.json";
const PENDING_AUTHORITY_SIGNATURE: &str = "bareline.pending-authority.minisig";
pub(super) const PENDING_ROOT_TRANSITIONS: &str = "bareline.pending-root-transitions.json";
const PENDING_UPDATE_HELPER: &str = "bareline.pending-update-helper.exe";
const TRUST_JOURNAL: &str = "bareline.trust-journal";
const UPDATE_JOURNAL: &str = "bareline.update-journal";
/// Delivered trust state (SEC-02): pending name in the stage and installation, the
/// published and installed name, and the size limit.
pub(super) const DELIVERED_TRUST_FILES: [(&str, &str, u64); 4] = [
    (PENDING_AUTHORITY, RELEASE_AUTHORITY, 16384),
    (PENDING_AUTHORITY_SIGNATURE, RELEASE_AUTHORITY_SIGNATURE, 8192),
    (PENDING_ROOT_TRANSITIONS, ROOT_TRANSITIONS, 262144),
    (PENDING_UPDATE_HELPER, UPDATE_HELPER, 64 * 1024 * 1024),
];
/// Files [`retain_update_evidence`] preserves, the apply journal last.
pub(super) const RETAINED_UPDATE_FILES: [&str; 10] = [
    "bareline.rollback.exe",
    "bareline.failed.exe",
    "bareline.pending.exe",
    "bareline.update.json",
    "bareline.update.minisig",
    PENDING_AUTHORITY,
    PENDING_AUTHORITY_SIGNATURE,
    PENDING_ROOT_TRANSITIONS,
    PENDING_UPDATE_HELPER,
    UPDATE_JOURNAL,
];
/// Installed trust files replaced by [`install_delivered_trust`], also retained.
const RETAINED_TRUST_FILES: [&str; 5] = [
    RELEASE_AUTHORITY,
    RELEASE_AUTHORITY_SIGNATURE,
    ROOT_TRANSITIONS,
    UPDATE_HELPER,
    TRUST_JOURNAL,
];
/// Legacy install-root ledger and its per-user replacement (SEC-04).
const ROOT_KEY_LEDGER: (&str, &str) = ("bareline.root-keys", "root-keys");
const ROOT_VERSION_LEDGER: (&str, &str) = ("bareline.root-versions", "root-versions");
/// The core executable's own metadata ledger (SEC-03). The runtime and the catalogs
/// keep theirs in the extension storage.
const CORE_VERSION_LEDGER: (&str, &str) = ("bareline.update-versions", "core.metadata-versions");
const APPLIED_LEDGER: &str = "core.applied";
const LAUNCH_COUNTER: &str = "launch-attempts";
/// Launches of a freshly updated build without a healthy acknowledgement before the
/// previous build is restored automatically (SEC-09).
pub const UPDATE_LAUNCH_ATTEMPTS: u32 = 3;
/// Generations of retained update evidence kept after a healthy acknowledgement (SEC-17).
pub const RETAINED_GENERATIONS: usize = 2;

/// Per-user update state (SEC-04): ledgers, the update lock and the failed-launch
/// counter. A per-machine installation under Program Files is not writable by the user,
/// so resolving trust and running extensions never write the installation. The state
/// follows [`bareline_distribution::data_root`]: the portable data folder, else
/// `%LOCALAPPDATA%\Bareline`, with one directory per installation.
pub fn update_state_root(root: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    validate_install_root(root)?;
    let executable = root.join("bareline.exe");
    let state = if root.join("bareline.portable").is_file() {
        bareline_distribution::data_root(&executable, true, root).map(|data| data.join("update"))
    } else {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|local| local.is_absolute())
            .and_then(|local| bareline_distribution::data_root(&executable, false, &local.join("Bareline")))
            .map(|data| -> std::io::Result<PathBuf> {
                // Side-by-side installations keep separate floors and locks. The key is the
                // final path of the opened root (GetFinalPathNameByHandle), the same for every
                // spelling of one installation: case, 8.3 names or `\\?\` prefixes (SEC-18).
                let mut digest = Sha256::new();
                for unit in std::fs::canonicalize(root)?.as_os_str().encode_wide() {
                    digest.update(unit.to_le_bytes());
                }
                let key = format!("{:x}", digest.finalize());
                Ok(data.join("update").join(&key[..16]))
            })
            .transpose()?
    }
    .ok_or_else(|| std::io::Error::other("per-user update state directory unavailable"))?;
    std::fs::create_dir_all(&state)?;
    validate_install_root(&state)?;
    Ok(state)
}

pub(super) fn read_update_file(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_update_read_file(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("update file limit"));
    }
    Ok(bytes)
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)?;
    out.write_all(bytes)?;
    out.sync_all()
}

/// Ledger lines: the legacy install-root ledger, only ever read, then the per-user
/// ledger that receives new entries. Migration never lowers a floor (SEC-04).
fn ledger_lines(root: &Path, state: &Path, (legacy, name): (&str, &str)) -> std::io::Result<Vec<String>> {
    let mut lines = Vec::new();
    for path in [root.join(legacy), state.join(name)] {
        if path.try_exists()? {
            let bytes = read_update_file(&path, 1024 * 1024)?;
            lines.extend(
                std::str::from_utf8(&bytes)
                    .map_err(std::io::Error::other)?
                    .lines()
                    .map(str::to_owned),
            );
        }
    }
    Ok(lines)
}

/// Durable append to a per-user ledger. Callers hold the update lock.
fn append_ledger(state: &Path, name: &str, line: &str) -> std::io::Result<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .share_mode(0)
        .custom_flags(0x00200000)
        .open(state.join(name))?;
    if out.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(std::io::Error::other("reparse update ledger refused"));
    }
    writeln!(out, "{line}")?;
    out.sync_all()
}

/// How [`resolve_release_authority`] treats expiry of the authority and root chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityFreshness {
    /// Accepting new metadata, packages, runtimes or executables.
    Required,
    /// Using already-verified installed state: extension restore and invocation, the
    /// health acknowledgement and recovery. Signatures, root floors, pins and
    /// revocations still apply; expiry never disables what was accepted (SEC-02).
    Installed,
}

pub(super) struct EvaluatedAuthority {
    pub(super) authority: bareline_distribution::trust::ReleaseAuthority,
    active_root: String,
    accepted_key: Option<String>,
    accepted_version: u64,
}

/// Verify an authority and optional root chain from the compiled root against the
/// accepted root lineage and version ledgers, without recording anything.
#[allow(clippy::too_many_arguments)]
pub(super) fn evaluate_authority(
    root: &Path,
    state: &Path,
    chain: Option<&[u8]>,
    authority: &[u8],
    signature: &[u8],
    policy: OfflineRootPolicy<'_>,
    freshness: AuthorityFreshness,
    now: u64,
) -> std::io::Result<EvaluatedAuthority> {
    use bareline_distribution::trust;
    let root_key = policy.public_key;
    let mut root_floor = policy.minimum_version;
    let mut active_root = root_key.to_owned();
    let mut lineage = vec![active_root.clone()];
    if let Some(chain) = chain {
        let (next, version, keys) = match freshness {
            AuthorityFreshness::Required => trust::verify_root_chain(chain, root_key, now),
            AuthorityFreshness::Installed => trust::verify_installed_root_chain(chain, root_key),
        }
        .map_err(|e| std::io::Error::other(format!("root transition: {e:?}")))?;
        active_root = next;
        lineage = keys;
        root_floor = root_floor.max(version);
    }
    let mut accepted_key = None;
    for old in ledger_lines(root, state, ROOT_KEY_LEDGER)? {
        if !lineage.contains(&old) {
            return Err(std::io::Error::other("root lineage rollback"));
        }
        accepted_key = Some(old);
    }
    let mut accepted_version = 0;
    for line in ledger_lines(root, state, ROOT_VERSION_LEDGER)? {
        accepted_version = accepted_version.max(line.parse::<u64>().map_err(std::io::Error::other)?);
    }
    root_floor = root_floor.max(accepted_version);
    let signature = std::str::from_utf8(signature).map_err(std::io::Error::other)?;
    let authority = match freshness {
        AuthorityFreshness::Required => trust::verify_authority(authority, signature, &active_root, root_floor, now),
        AuthorityFreshness::Installed => {
            trust::verify_installed_authority(authority, signature, &active_root, root_floor)
        }
    }
    .map_err(|e| std::io::Error::other(format!("release authority: {e:?}")))?;
    Ok(EvaluatedAuthority {
        authority,
        active_root,
        accepted_key,
        accepted_version,
    })
}

/// Record an accepted root key and root version in the per-user ledgers.
pub(super) fn record_evaluated_authority(state: &Path, evaluated: &EvaluatedAuthority) -> std::io::Result<()> {
    if evaluated.accepted_key.as_deref() != Some(evaluated.active_root.as_str()) {
        append_ledger(state, ROOT_KEY_LEDGER.1, &evaluated.active_root)?;
    }
    if evaluated.authority.root_version > evaluated.accepted_version {
        append_ledger(
            state,
            ROOT_VERSION_LEDGER.1,
            &evaluated.authority.root_version.to_string(),
        )?;
    }
    Ok(())
}

/// Core metadata floor (SEC-03): the compiled or authority core floor and the core
/// ledger, never the runtime or catalog ledgers. Reads the legacy install-root ledger.
pub fn core_metadata_floor(root: &Path, state: &Path, authority_floor: u64) -> std::io::Result<u64> {
    let mut floor = authority_floor;
    for line in ledger_lines(root, state, CORE_VERSION_LEDGER)? {
        floor = floor.max(line.parse::<u64>().map_err(std::io::Error::other)?);
    }
    Ok(floor)
}

/// Durable monotonic core floor before an update is applied. The caller holds the lock.
pub fn record_core_metadata_version(state: &Path, version: u64) -> std::io::Result<()> {
    append_ledger(state, CORE_VERSION_LEDGER.1, &version.to_string())
}

/// Record which exact build an applied update replaced (SEC-09), so the last update
/// can be undone after the health acknowledgement archived its journal. The caller
/// holds the lock.
pub fn record_applied_update(state: &Path, old_sha256: &str, new_sha256: &str) -> std::io::Result<()> {
    if !sha256_hex(old_sha256) || !sha256_hex(new_sha256) {
        return Err(std::io::Error::other("invalid applied update digest"));
    }
    append_ledger(state, APPLIED_LEDGER, &format!("{old_sha256} {new_sha256}"))
}

/// What `--recover` may restore (SEC-09).
pub enum RecoverySource {
    /// The journal of an applied, not yet acknowledged update pins both builds.
    Journal,
    /// The retained build the last applied update replaced, held and matched by hash.
    Applied(File),
}

/// Recovery requires the apply journal or a ledger entry for the running build. Only
/// the last applied update is undone, to the exact build it replaced, never an older
/// generation, and the metadata floors are never lowered. A bare backup is refused.
pub fn recovery_source(root: &Path, state: &Path) -> std::io::Result<Option<RecoverySource>> {
    validate_install_root(root)?;
    if root.join(UPDATE_JOURNAL).try_exists()? {
        // An interrupted apply or a reinstalled editor leaves nothing to roll back.
        return Ok(journal_pins_fresh_update(root)?.then_some(RecoverySource::Journal));
    }
    let ledger = state.join(APPLIED_LEDGER);
    if !ledger.try_exists()? {
        return Ok(None);
    }
    let bytes = read_update_file(&ledger, 1024 * 1024)?;
    let Some(last) = std::str::from_utf8(&bytes)
        .map_err(std::io::Error::other)?
        .lines()
        .last()
    else {
        return Ok(None);
    };
    let (old, new) = last
        .split_once(' ')
        .filter(|(old, new)| sha256_hex(old) && sha256_hex(new))
        .ok_or_else(|| std::io::Error::other("invalid applied update ledger"))?;
    let mut current = open_update_read_file(&root.join("bareline.exe"))?;
    if update_file_sha256(&mut current)? != new {
        return Ok(None);
    }
    drop(current);
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let name = entry?.file_name();
        let generation = name
            .to_str()
            .and_then(|name| retained_generation(name, "bareline.rollback.exe"));
        if let Some(generation) = generation {
            candidates.push((generation, name));
        }
    }
    candidates.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    for (_, name) in candidates {
        let Ok(mut held) = open_update_file(&root.join(name)) else {
            continue;
        };
        if update_file_sha256(&mut held)? == old {
            return Ok(Some(RecoverySource::Applied(held)));
        }
    }
    Ok(None)
}

/// The generation of `file_name` when it is `name` retained by this module.
fn retained_generation(file_name: &str, name: &str) -> Option<u128> {
    file_name
        .strip_prefix(name)?
        .strip_prefix(".retained-")
        .filter(|generation| !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit()))?
        .parse()
        .ok()
}

/// Keep only the newest `keep` generations of retained update evidence (SEC-17). Only
/// files this module retains are considered; a file that cannot be removed now, such as
/// a still-running retained helper, is left for the next acknowledgement.
pub fn prune_retained_evidence(root: &Path, keep: usize) -> std::io::Result<usize> {
    validate_install_root(root)?;
    let mut retained = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(generation) = RETAINED_UPDATE_FILES
            .iter()
            .chain(RETAINED_TRUST_FILES.iter())
            .find_map(|known| retained_generation(name, known))
        {
            retained.push((generation, name.to_owned()));
        }
    }
    let generations: std::collections::BTreeSet<u128> = retained.iter().map(|(generation, _)| *generation).collect();
    let kept: Vec<u128> = generations.iter().rev().take(keep).copied().collect();
    let mut removed = 0;
    for (generation, name) in retained {
        if kept.contains(&generation) {
            continue;
        }
        let path = root.join(name);
        // Regular, unlinked, non-reparse files only.
        if open_update_file(&path).is_ok() && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether the apply journal pins a freshly updated build: the running editor is the
/// build the journal installed and the backup is the build it replaced, the conditions
/// the helper checks before restoring (SEC-09). An interrupted apply, a reinstalled or
/// replaced editor and a missing or changed backup do not qualify.
fn journal_pins_fresh_update(root: &Path) -> std::io::Result<bool> {
    let journal = read_update_file(&root.join(UPDATE_JOURNAL), 1024)?;
    let journal = std::str::from_utf8(&journal).map_err(std::io::Error::other)?;
    let field = |name: &str| {
        journal
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .filter(|hash| sha256_hex(hash))
    };
    let (Some(old), Some(new)) = (field("old_sha256="), field("new_sha256=")) else {
        return Ok(false);
    };
    let has_hash = |name: &str, expected: &str| -> std::io::Result<bool> {
        let path = root.join(name);
        Ok(path.try_exists()? && update_file_sha256(&mut open_update_read_file(&path)?)? == expected)
    };
    Ok(has_hash("bareline.exe", new)? && has_hash("bareline.rollback.exe", old)?)
}

/// Outcome of [`record_update_launch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchDecision {
    /// No freshly applied update awaits acknowledgement.
    NotPending,
    /// This is the given launch of the freshly updated build.
    Counted(u32),
    /// The limit was reached without a healthy acknowledgement: restore the previous
    /// build. Returned once; the attempt is recorded first.
    Recover,
    /// Automatic recovery was attempted and the update is still in place: start normally
    /// and report it. Terminal until the acknowledgement or a recovery ends the update.
    RecoveryFailed,
}

/// Count a launch of a freshly updated build, i.e. while the apply journal awaits the
/// health acknowledgement, which clears the count (SEC-09). Only the exact build the
/// journal installed counts, with the build it replaced still retained. A trust install
/// interrupted by a crash is rolled back first, before anything reads the authority.
pub fn record_update_launch(root: &Path, state: &Path, limit: u32) -> std::io::Result<LaunchDecision> {
    validate_install_root(root)?;
    let _lock = lock_update_installation(state)?;
    reconcile_trust_install(root)?;
    let counter = state.join(LAUNCH_COUNTER);
    if !root.join(UPDATE_JOURNAL).try_exists()? {
        // A stale count never carries into the next update.
        clear_update_launch_attempts(state)?;
        return Ok(LaunchDecision::NotPending);
    }
    if !journal_pins_fresh_update(root)? {
        return Ok(LaunchDecision::NotPending);
    }
    let previous = if counter.try_exists()? {
        let bytes = read_update_file(&counter, 16)?;
        std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .ok_or_else(|| std::io::Error::other("invalid launch counter"))?
    } else {
        0
    };
    if previous > limit {
        return Ok(LaunchDecision::RecoveryFailed);
    }
    // The count past the limit marks the recovery attempt. It is durable before the
    // helper takes over, so a recovery that fails never makes every launch exit.
    let next = previous + 1;
    let temporary = state.join(format!("{LAUNCH_COUNTER}.tmp"));
    {
        let mut out = File::create(&temporary)?;
        write!(out, "{next}")?;
        out.sync_all()?;
    }
    std::fs::rename(&temporary, &counter)?;
    Ok(if next > limit {
        LaunchDecision::Recover
    } else {
        LaunchDecision::Counted(next)
    })
}

/// Clear the failed-launch count after a healthy acknowledgement or a recovery. The
/// caller holds the update lock.
pub fn clear_update_launch_attempts(state: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(state.join(LAUNCH_COUNTER)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Delivered trust files are served next to the signed core manifest (SEC-02).
pub(super) fn delivered_trust_path(manifest_path: &str, name: &str) -> String {
    let directory = manifest_path.rfind('/').map_or("", |index| &manifest_path[..index]);
    format!("{directory}/{name}")
}

/// Verified trust state delivered with a core update, held until installed.
pub struct DeliveredTrust {
    files: Vec<(File, &'static str)>,
}

/// Verify the trust state a signed core manifest delivers (SEC-02). The current
/// authority's release key signed the manifest that binds the exact authority (and
/// root chain) by digest; the offline root still verifies the authority from the
/// compiled root, it may never lower the accepted root version, and the delivered
/// helper and the staged editor must match the delivered authority's pins. `directory`
/// holds the pending files: the private stage before transfer, the installation when
/// the helper applies. `None` when the manifest delivers nothing or nothing changed.
#[allow(clippy::too_many_arguments)]
pub fn verify_delivered_trust(
    directory: &Path,
    root: &Path,
    state: &Path,
    manifest: &Manifest,
    policy: OfflineRootPolicy<'_>,
    core: &File,
    now: u64,
) -> std::io::Result<Option<DeliveredTrust>> {
    verify_delivered_trust_with(
        directory,
        root,
        state,
        manifest,
        policy,
        core,
        now,
        &|file: &File, pin: &PublisherPin| verify_authenticode(file, pin, Revocation::Online),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify_delivered_trust_with(
    directory: &Path,
    root: &Path,
    state: &Path,
    manifest: &Manifest,
    policy: OfflineRootPolicy<'_>,
    core: &File,
    now: u64,
    verify_publisher: &impl Fn(&File, &PublisherPin) -> Result<(), UpdateError>,
) -> std::io::Result<Option<DeliveredTrust>> {
    let Some(expected) = manifest.authority_sha256.as_deref() else {
        return Ok(None);
    };
    let held = |name: &str, limit: u64| -> std::io::Result<(File, Vec<u8>)> {
        let file = open_update_file(&directory.join(name))?;
        let mut bytes = Vec::new();
        (&file).take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::other("delivered trust file limit"));
        }
        Ok((file, bytes))
    };
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let (authority_file, authority) = held(PENDING_AUTHORITY, 16384)?;
    if digest(&authority) != expected {
        return Err(std::io::Error::other(
            "delivered authority differs from the signed update",
        ));
    }
    let (signature_file, signature) = held(PENDING_AUTHORITY_SIGNATURE, 8192)?;
    let mut unchanged = installed_equals(&root.join(RELEASE_AUTHORITY), &authority)?
        && installed_equals(&root.join(RELEASE_AUTHORITY_SIGNATURE), &signature)?;
    let mut files = vec![
        (authority_file, RELEASE_AUTHORITY),
        (signature_file, RELEASE_AUTHORITY_SIGNATURE),
    ];
    let installed_chain = root.join(ROOT_TRANSITIONS);
    let chain_installed = installed_chain.try_exists()?;
    let chain = match manifest.root_transitions_sha256.as_deref() {
        Some(expected) => {
            let (file, bytes) = held(PENDING_ROOT_TRANSITIONS, 262144)?;
            if digest(&bytes) != expected {
                return Err(std::io::Error::other(
                    "delivered root transitions differ from the signed update",
                ));
            }
            unchanged &= installed_equals(&installed_chain, &bytes)?;
            files.push((file, ROOT_TRANSITIONS));
            Some(bytes)
        }
        None if chain_installed => Some(read_update_file(&installed_chain, 262144)?),
        None => None,
    };
    let next = evaluate_authority(
        root,
        state,
        chain.as_deref(),
        &authority,
        &signature,
        policy,
        AuthorityFreshness::Required,
        now,
    )?
    .authority;
    let pin = next.publisher_pin();
    let mut helper = open_update_file(&directory.join(PENDING_UPDATE_HELPER))?;
    if update_file_sha256(&mut helper)? != next.update_helper_sha256 {
        return Err(std::io::Error::other(
            "delivered update helper differs from the delivered authority",
        ));
    }
    verify_publisher(&helper, &pin).map_err(|e| std::io::Error::other(format!("delivered helper publisher: {e:?}")))?;
    // The updated editor must satisfy the authority it will run with.
    verify_publisher(core, &pin)
        .map_err(|e| std::io::Error::other(format!("update publisher under the delivered authority: {e:?}")))?;
    let installed_helper = root.join(UPDATE_HELPER);
    unchanged &= installed_helper.try_exists()?
        && update_file_sha256(&mut open_update_read_file(&installed_helper)?)? == next.update_helper_sha256;
    if unchanged {
        return Ok(None);
    }
    files.push((helper, UPDATE_HELPER));
    Ok(Some(DeliveredTrust { files }))
}

fn installed_equals(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if !path.try_exists()? {
        return Ok(false);
    }
    let mut installed = Vec::new();
    open_update_read_file(path)?
        .take(bytes.len() as u64 + 1)
        .read_to_end(&mut installed)?;
    Ok(installed == bytes)
}

/// Install verified delivered trust state with rollback (SEC-02). A journal is written
/// first; each installed file is retained under a generation suffix and the verified
/// pending handle takes its name, including the running helper's own image, which can
/// be renamed. On failure, or after a crash via [`reconcile_trust_install`], the
/// previous authority, root chain and helper are restored. The caller holds the lock.
pub fn install_delivered_trust(root: &Path, trust: DeliveredTrust) -> std::io::Result<()> {
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    install_trust_generation(root, trust, generation)
}

pub(super) fn install_trust_generation(root: &Path, trust: DeliveredTrust, generation: u128) -> std::io::Result<()> {
    validate_install_root(root)?;
    let mut journal = format!("generation={generation}\n");
    for (_, name) in &trust.files {
        let kind = if root.join(name).try_exists()? {
            "replaced"
        } else {
            "added"
        };
        journal.push_str(&format!("{kind} {name}\n"));
    }
    write_new_synced(&root.join(TRUST_JOURNAL), journal.as_bytes())?;
    let result = swap_trust(root, &trust.files, generation);
    // Release the pending handles before any rollback reopens those files.
    drop(trust);
    if let Err(error) = result {
        // A failed rollback keeps the journal for the next reconciliation.
        rollback_trust_install(root, &journal)?;
        retain_trust_journal(root, generation)?;
        return Err(error);
    }
    retain_trust_journal(root, generation)
}

fn swap_trust(root: &Path, files: &[(File, &'static str)], generation: u128) -> std::io::Result<()> {
    for (_, name) in files {
        let path = root.join(name);
        if path.try_exists()? {
            let installed = open_update_file(&path)?;
            rename_update_handle(&installed, &root.join(format!("{name}.retained-{generation}")))?;
        }
    }
    for (pending, name) in files {
        rename_update_handle(pending, &root.join(name))?;
    }
    Ok(())
}

fn journal_generation(journal: &str) -> std::io::Result<u128> {
    journal
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("generation="))
        .filter(|generation| !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|generation| generation.parse().ok())
        .ok_or_else(|| std::io::Error::other("invalid trust journal"))
}

fn rollback_trust_install(root: &Path, journal: &str) -> std::io::Result<()> {
    let generation = journal_generation(journal)?;
    let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
        let held = open_update_file(from)?;
        rename_update_handle(&held, to)
    };
    for line in journal.lines().skip(1) {
        let invalid = || std::io::Error::other("invalid trust journal");
        let (kind, name) = line.split_once(' ').ok_or_else(invalid)?;
        let (pending, _, _) = DELIVERED_TRUST_FILES
            .iter()
            .find(|(_, installed, _)| *installed == name)
            .ok_or_else(invalid)?;
        let installed = root.join(name);
        let pending = root.join(pending);
        let retained = root.join(format!("{name}.retained-{generation}"));
        match kind {
            "replaced" => {
                if retained.try_exists()? {
                    // The delivered file took the name: return it to its pending name.
                    if !pending.try_exists()? && installed.try_exists()? {
                        rename(&installed, &pending)?;
                    }
                    if !installed.try_exists()? {
                        rename(&retained, &installed)?;
                    }
                }
            }
            "added" => {
                if installed.try_exists()? && !pending.try_exists()? {
                    rename(&installed, &pending)?;
                }
            }
            _ => return Err(invalid()),
        }
    }
    Ok(())
}

fn retain_trust_journal(root: &Path, generation: u128) -> std::io::Result<()> {
    let journal = open_update_file(&root.join(TRUST_JOURNAL))?;
    rename_update_handle(&journal, &root.join(format!("{TRUST_JOURNAL}.retained-{generation}")))
}

/// Roll back a trust install interrupted by a crash (SEC-02); the pending files return
/// to their pending names so the staged update can be retried or discarded. The caller
/// holds the update lock.
pub fn reconcile_trust_install(root: &Path) -> std::io::Result<()> {
    let path = root.join(TRUST_JOURNAL);
    if !path.try_exists()? {
        return Ok(());
    }
    let journal = String::from_utf8(read_update_file(&path, 4096)?).map_err(std::io::Error::other)?;
    rollback_trust_install(root, &journal)?;
    retain_trust_journal(root, journal_generation(&journal)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> PathBuf {
        create_private_stage(&std::env::temp_dir()).unwrap()
    }
    fn clean(directory: PathBuf) {
        for entry in std::fs::read_dir(&directory).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }
    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn receipt(old: &[u8], new: &[u8]) -> String {
        format!(
            "schema_version=1\nold_sha256={}\nnew_sha256={}\nmetadata_version=4\n",
            sha(old),
            sha(new)
        )
    }

    #[test]
    fn launch_counter_recovers_once_after_repeated_unacknowledged_launches() {
        // SEC-09: only launches of the exact build an unacknowledged update installed count.
        let (root, state) = (fresh(), fresh());
        std::fs::write(root.join("bareline.exe"), b"new build").unwrap();
        std::fs::write(root.join("bareline.rollback.exe"), b"old build").unwrap();
        assert_eq!(
            record_update_launch(&root, &state, 3).unwrap(),
            LaunchDecision::NotPending
        );
        std::fs::write(root.join(UPDATE_JOURNAL), receipt(b"old build", b"new build")).unwrap();
        for launch in 1..=3 {
            assert_eq!(
                record_update_launch(&root, &state, 3).unwrap(),
                LaunchDecision::Counted(launch)
            );
        }
        // The limit hands off to recovery once; the attempt is recorded first.
        assert_eq!(record_update_launch(&root, &state, 3).unwrap(), LaunchDecision::Recover);
        // The update still in place means the recovery failed: later launches start
        // normally and report it, and never hand off (and exit) again.
        for _ in 0..3 {
            assert_eq!(
                record_update_launch(&root, &state, 3).unwrap(),
                LaunchDecision::RecoveryFailed
            );
        }
        // The health acknowledgement clears it.
        clear_update_launch_attempts(&state).unwrap();
        clear_update_launch_attempts(&state).unwrap();
        assert_eq!(
            record_update_launch(&root, &state, 3).unwrap(),
            LaunchDecision::Counted(1)
        );
        // Nothing else counts: an interrupted apply (the old build runs), a reinstalled
        // editor, a missing or changed backup, or an invalid journal.
        for (editor, backup, journal) in [
            ("old build", Some("old build"), receipt(b"old build", b"new build")),
            (
                "reinstalled build",
                Some("old build"),
                receipt(b"old build", b"new build"),
            ),
            ("new build", Some("changed build"), receipt(b"old build", b"new build")),
            ("new build", None, receipt(b"old build", b"new build")),
            ("new build", Some("old build"), "receipt".to_owned()),
        ] {
            std::fs::write(root.join("bareline.exe"), editor).unwrap();
            let _ = std::fs::remove_file(root.join("bareline.rollback.exe"));
            if let Some(backup) = backup {
                std::fs::write(root.join("bareline.rollback.exe"), backup).unwrap();
            }
            std::fs::write(root.join(UPDATE_JOURNAL), journal).unwrap();
            assert_eq!(
                record_update_launch(&root, &state, 3).unwrap(),
                LaunchDecision::NotPending
            );
        }
        std::fs::write(root.join(UPDATE_JOURNAL), receipt(b"old build", b"new build")).unwrap();
        assert_eq!(
            record_update_launch(&root, &state, 3).unwrap(),
            LaunchDecision::Counted(2)
        );
        // Recovery archives the journal: the stale count is dropped, never carried over.
        std::fs::remove_file(root.join(UPDATE_JOURNAL)).unwrap();
        assert_eq!(
            record_update_launch(&root, &state, 3).unwrap(),
            LaunchDecision::NotPending
        );
        assert!(!state.join(LAUNCH_COUNTER).exists());
        std::fs::write(root.join(UPDATE_JOURNAL), receipt(b"old build", b"new build")).unwrap();
        assert_eq!(
            record_update_launch(&root, &state, 3).unwrap(),
            LaunchDecision::Counted(1)
        );
        // A corrupt counter is an error, never a silent reset.
        std::fs::write(state.join(LAUNCH_COUNTER), b"many").unwrap();
        assert!(record_update_launch(&root, &state, 3).is_err());
        // Counting uses per-user state only; the installation is unchanged.
        assert_eq!(
            names(&root),
            vec!["bareline.exe", "bareline.rollback.exe", UPDATE_JOURNAL]
        );
        clean(root);
        clean(state);
    }

    #[test]
    fn recover_requires_the_journal_or_an_applied_ledger_entry() {
        // SEC-09: a bare backup never authorizes recovery.
        let (root, state) = (fresh(), fresh());
        std::fs::write(root.join("bareline.exe"), b"new build").unwrap();
        std::fs::write(root.join("bareline.rollback.exe"), b"old build").unwrap();
        std::fs::write(root.join("bareline.rollback.exe.retained-5"), b"old build").unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        // The journal of an unacknowledged update authorizes recovery while its build runs.
        std::fs::write(root.join(UPDATE_JOURNAL), receipt(b"old build", b"new build")).unwrap();
        assert!(matches!(
            recovery_source(&root, &state).unwrap(),
            Some(RecoverySource::Journal)
        ));
        // The journal of an interrupted apply, or of a replaced editor, restores nothing.
        std::fs::write(root.join(UPDATE_JOURNAL), receipt(b"new build", b"newer build")).unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        std::fs::write(root.join(UPDATE_JOURNAL), b"receipt").unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        std::fs::remove_file(root.join(UPDATE_JOURNAL)).unwrap();
        // After acknowledgement, the ledger entry names the running build and the exact
        // retained build it replaced.
        assert!(record_applied_update(&state, "not a digest", &sha(b"new build")).is_err());
        record_applied_update(&state, &sha(b"old build"), &sha(b"new build")).unwrap();
        match recovery_source(&root, &state).unwrap() {
            Some(RecoverySource::Applied(mut file)) => {
                let mut bytes = Vec::new();
                std::io::Seek::rewind(&mut file).unwrap();
                file.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes, b"old build");
            }
            _ => panic!("the ledger entry authorizes the retained previous build"),
        }
        // A retained file with other bytes is never restored.
        std::fs::write(root.join("bareline.rollback.exe.retained-5"), b"tampered").unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        std::fs::write(root.join("bareline.rollback.exe.retained-5"), b"old build").unwrap();
        // Only the last update, and only while its build runs: after a rollback (or any
        // other build) the entry no longer applies, so no older generation is reachable.
        std::fs::write(root.join("bareline.exe"), b"old build").unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        record_applied_update(&state, &sha(b"older build"), &sha(b"old build")).unwrap();
        assert!(recovery_source(&root, &state).unwrap().is_none());
        clean(root);
        clean(state);
    }

    #[test]
    fn retention_keeps_the_newest_generations_of_known_evidence_only() {
        // SEC-17: prune retained executables and receipts to the newest generations.
        let root = fresh();
        for (name, generation) in [
            ("bareline.rollback.exe", 1),
            ("bareline.update-journal", 1),
            ("bareline.rollback.exe", 2),
            ("bareline.failed.exe", 2),
            ("bareline-update-helper.exe", 3),
            ("bareline.release-authority.json", 3),
            ("bareline.rollback.exe", 10),
            ("bareline.update-journal", 10),
        ] {
            std::fs::write(root.join(format!("{name}.retained-{generation}")), b"evidence").unwrap();
        }
        for unrelated in [
            "bareline.exe",
            "notes.txt.retained-1",
            "bareline.rollback.exe.retained-x",
            "bareline.rollback.exe.retained-",
        ] {
            std::fs::write(root.join(unrelated), b"keep").unwrap();
        }
        assert_eq!(prune_retained_evidence(&root, RETAINED_GENERATIONS).unwrap(), 4);
        assert_eq!(
            names(&root),
            vec![
                "bareline-update-helper.exe.retained-3",
                "bareline.exe",
                "bareline.release-authority.json.retained-3",
                "bareline.rollback.exe.retained-",
                "bareline.rollback.exe.retained-10",
                "bareline.rollback.exe.retained-x",
                "bareline.update-journal.retained-10",
                "notes.txt.retained-1",
            ]
        );
        assert_eq!(prune_retained_evidence(&root, RETAINED_GENERATIONS).unwrap(), 0);
        clean(root);
    }

    #[test]
    fn ledgers_move_to_per_user_state_and_keep_legacy_floors() {
        // SEC-03/SEC-04: the core ledger is its own, read from the legacy install-root
        // ledger and written only to per-user state.
        let (root, state) = (fresh(), fresh());
        assert_eq!(core_metadata_floor(&root, &state, 3).unwrap(), 3);
        std::fs::write(root.join("bareline.update-versions"), b"5\n7\n").unwrap();
        assert_eq!(core_metadata_floor(&root, &state, 3).unwrap(), 7);
        record_core_metadata_version(&state, 8).unwrap();
        assert_eq!(core_metadata_floor(&root, &state, 3).unwrap(), 8);
        assert_eq!(core_metadata_floor(&root, &state, 9).unwrap(), 9);
        assert_eq!(std::fs::read(root.join("bareline.update-versions")).unwrap(), b"5\n7\n");
        std::fs::write(state.join(CORE_VERSION_LEDGER.1), b"x\n").unwrap();
        assert!(core_metadata_floor(&root, &state, 3).is_err());
        clean(root);
        clean(state);
    }

    #[test]
    fn delivered_trust_is_served_next_to_the_manifest() {
        assert_eq!(
            delivered_trust_path("/updates/manifest.json", RELEASE_AUTHORITY),
            "/updates/bareline.release-authority.json"
        );
        assert_eq!(
            delivered_trust_path("/manifest.json", UPDATE_HELPER),
            "/bareline-update-helper.exe"
        );
    }

    #[test]
    fn running_helper_image_can_be_renamed_for_trust_install() {
        // The helper installs a new helper while its own image runs (SEC-02).
        let Some(ready) = std::env::var_os("BARELINE_TEST_RENAME_READY") else {
            let root = fresh();
            let image = root.join(UPDATE_HELPER);
            let ready = root.join("ready");
            std::fs::copy(std::env::current_exe().unwrap(), &image).unwrap();
            let mut child = std::process::Command::new(&image)
                .args([
                    "--exact",
                    "update::lifecycle::tests::running_helper_image_can_be_renamed_for_trust_install",
                ])
                .env("BARELINE_TEST_RENAME_READY", &ready)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            // A bounded wait for a slow process start on loaded CI machines.
            for _ in 0..3000 {
                if ready.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(ready.exists());
            let retained = root.join(format!("{UPDATE_HELPER}.retained-1"));
            let renamed = open_update_file(&image).and_then(|held| rename_update_handle(&held, &retained));
            std::io::Write::write_all(&mut child.stdin.take().unwrap(), b"x").unwrap();
            assert!(child.wait().unwrap().success());
            renamed.expect("a running image can be renamed");
            assert!(!image.exists() && retained.exists());
            // An exited image can stay locked briefly (section release, scanners): retry
            // for a bounded time, then leave it in the temporary directory.
            std::fs::remove_file(root.join("ready")).unwrap();
            for _ in 0..100 {
                if std::fs::remove_file(&retained).is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = std::fs::remove_dir(&root);
            return;
        };
        std::fs::write(ready, b"ready").unwrap();
        let mut byte = [0];
        std::io::Read::read_exact(&mut std::io::stdin(), &mut byte).unwrap();
    }
}
