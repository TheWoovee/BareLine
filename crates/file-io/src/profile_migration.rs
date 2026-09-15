// SPDX-License-Identifier: MPL-2.0
//! Resumable, per-item roaming-to-local profile migration.
use bareline_platform::LocalFileSystem;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const VERSION: u32 = 1;
const JOURNAL: &str = ".bareline-profile-migration.json";
const ITEMS: [&str; 5] = ["settings.toml", "session.json", "recovery", "macros", "extensions"];
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ItemState {
    Pending,
    CopiedVerified,
    Published,
    SourceRetired,
    FailedRetryable,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadAuthority {
    Local,
    Legacy,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Fingerprint {
    directory: bool,
    entries: u64,
    bytes: u64,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ItemRecord {
    name: String,
    state: ItemState,
    source: Option<Fingerprint>,
    detail: Option<String>,
    #[serde(default)]
    retirement: RetirementDisposition,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
enum RetirementDisposition {
    #[default]
    Pending,
    Retry,
    RetainedByPolicy,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Journal {
    version: u32,
    items: Vec<ItemRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemReceipt {
    pub name: &'static str,
    pub state: ItemState,
    pub detail: Option<String>,
    pub retained_source: Option<PathBuf>,
    pub authority: ReadAuthority,
    pub migrated: bool,
    pub destination_present: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MigrationReport {
    pub items: Vec<ItemReceipt>,
    pub retryable: bool,
    pub conflicts: bool,
}
impl MigrationReport {
    pub fn state(&self, name: &str) -> Option<ItemState> {
        self.items.iter().find(|item| item.name == name).map(|item| item.state)
    }
    pub fn authority(&self, name: &str) -> Option<ReadAuthority> {
        self.items
            .iter()
            .find(|item| item.name == name)
            .map(|item| item.authority)
    }
}

pub fn inspect_authorities(roaming: &Path, local: &Path, platform: &dyn LocalFileSystem) -> MigrationReport {
    let journal = load_journal(local, platform)
        .ok()
        .filter(|journal| validate_journal(journal).is_ok());
    let items: Vec<_> = ITEMS
        .into_iter()
        .map(|name| {
            let destination = local.join(name);
            let source = roaming.join(name);
            let record = journal
                .as_ref()
                .and_then(|journal| journal.items.iter().find(|item| item.name == name));
            ItemReceipt {
                name,
                state: record.map_or(ItemState::Pending, |record| record.state),
                detail: record.and_then(|record| record.detail.clone()),
                retained_source: entry_present(&source, platform)
                    .ok()
                    .and_then(|present| present.then_some(source.clone())),
                authority: probe_record_authority(record, &destination, &source, platform),
                migrated: record.is_some_and(|record| record.source.is_some()),
                destination_present: entry_present(&destination, platform).unwrap_or(false),
            }
        })
        .collect();
    let retryable = items
        .iter()
        .any(|item| item.authority == ReadAuthority::Unavailable || item.state == ItemState::FailedRetryable);
    MigrationReport {
        items,
        retryable,
        conflicts: false,
    }
}

fn probe_authority(local: &Path, legacy: &Path, platform: &dyn LocalFileSystem) -> ReadAuthority {
    match platform.migration_entry_guard(local) {
        Ok(_) => ReadAuthority::Local,
        Err(error) if error.kind() == io::ErrorKind::NotFound => match platform.migration_entry_guard(legacy) {
            Ok(_) => ReadAuthority::Legacy,
            Err(error) if error.kind() == io::ErrorKind::NotFound => ReadAuthority::Local,
            Err(_) => ReadAuthority::Unavailable,
        },
        Err(_) => ReadAuthority::Unavailable,
    }
}

pub struct MigrationRequest<'a> {
    pub roaming: &'a Path,
    pub local: &'a Path,
    pub retire_sources: bool,
    pub max_entries: usize,
    pub max_io_bytes: u64,
    pub max_time: Duration,
}

pub fn retirement_ready(local: &Path, platform: &dyn LocalFileSystem) -> bool {
    load_journal(local, platform).is_ok_and(|journal| {
        journal.items.len() == ITEMS.len()
            && journal
                .items
                .iter()
                .all(|item| matches!(item.state, ItemState::Published | ItemState::SourceRetired))
            && journal.items.iter().any(|item| {
                item.state == ItemState::Published
                    && matches!(
                        item.retirement,
                        RetirementDisposition::Pending | RetirementDisposition::Retry
                    )
            })
    })
}

pub fn migrate(
    request: MigrationRequest<'_>,
    platform: &dyn LocalFileSystem,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<MigrationReport> {
    if request.roaming == request.local {
        let mut report = inspect_authorities(request.roaming, request.local, platform);
        for item in &mut report.items {
            item.authority = ReadAuthority::Local;
        }
        return Ok(report);
    }
    fs::create_dir_all(request.local)?;
    let _local_guard = platform.guard_directory(request.local)?;
    let _roaming_guard = match platform.guard_directory(request.roaming) {
        Ok(guard) => guard,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(inspect_authorities(request.roaming, request.local, platform));
        }
        Err(error) => return Err(error),
    };
    let mut journal = match load_journal(request.local, platform) {
        Ok(journal) => journal,
        Err(error) if error.kind() == io::ErrorKind::NotFound => new_journal(),
        Err(error) => return Err(error),
    };
    validate_journal(&journal)?;
    persist_journal(request.local, &journal, platform)?;
    let mut budget = Budget::new(request.max_entries, request.max_io_bytes, request.max_time, cancelled);
    let retained = request.local.join(".bareline-migration-retained");
    let mut report = MigrationReport::default();
    for (index, name) in ITEMS.into_iter().enumerate() {
        let source = request.roaming.join(name);
        let destination = request.local.join(name);
        if !budget.exhausted() {
            let result = migrate_item(
                name,
                &source,
                &destination,
                &retained,
                request.retire_sources,
                &mut journal,
                index,
                request.local,
                platform,
                &mut budget,
            );
            if let Err(error) = result {
                let state = if error.kind() == io::ErrorKind::AlreadyExists {
                    // A verified divergence remains a conflict even when the
                    // local publication is already authoritative.
                    ItemState::Conflict
                } else if journal.items[index].state == ItemState::Published {
                    // Publication remains authoritative when only source retirement
                    // failed. Preserve that durable fact so readers never fall back
                    // to an older roaming copy and the next run can retry retirement.
                    ItemState::Published
                } else {
                    ItemState::FailedRetryable
                };
                if state == ItemState::Published {
                    journal.items[index].retirement = RetirementDisposition::Retry;
                }
                transition(
                    request.local,
                    &mut journal,
                    index,
                    state,
                    Some(error.to_string()),
                    platform,
                )?;
            }
        }
        let record = &journal.items[index];
        report.retryable |=
            record.state == ItemState::FailedRetryable || record.retirement == RetirementDisposition::Retry;
        report.conflicts |= record.state == ItemState::Conflict;
        let authority = probe_record_authority(Some(record), &destination, &source, platform);
        let destination_present = entry_present(&destination, platform).unwrap_or(false);
        let retained_source = entry_present(&source, platform)
            .ok()
            .and_then(|present| present.then_some(source));
        report.items.push(ItemReceipt {
            name,
            state: record.state,
            detail: record.detail.clone(),
            retained_source,
            authority,
            migrated: record.source.is_some(),
            destination_present,
        });
        report.retryable |= report
            .items
            .last()
            .is_some_and(|item| item.authority == ReadAuthority::Unavailable);
        if budget.exhausted() {
            report.retryable = true;
        }
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn migrate_item(
    name: &'static str,
    source: &Path,
    destination: &Path,
    retained_root: &Path,
    retire_sources: bool,
    journal: &mut Journal,
    index: usize,
    local: &Path,
    platform: &dyn LocalFileSystem,
    budget: &mut Budget<'_>,
) -> io::Result<()> {
    budget.check()?;
    if journal.items[index].state == ItemState::Published
        && journal.items[index].retirement == RetirementDisposition::RetainedByPolicy
        && entry_present(destination, platform)?
    {
        return Ok(());
    }
    let source_present = entry_present(source, platform)?;
    let prior_unverified_publication = journal.items[index].source.is_some()
        && matches!(
            journal.items[index].state,
            ItemState::CopiedVerified | ItemState::FailedRetryable
        );
    if !source_present && prior_unverified_publication {
        let recorded = journal.items[index]
            .source
            .as_ref()
            .expect("unverified publication has a recorded source fingerprint");
        if !entry_present(destination, platform)? || fingerprint(destination, platform, budget)? != *recorded {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unverified {name} publication cannot replace its absent retained source"),
            ));
        }
    }
    if !source_present {
        journal.items[index].retirement = RetirementDisposition::Complete;
        return transition(local, journal, index, ItemState::SourceRetired, None, platform);
    }
    let source_fingerprint = fingerprint(source, platform, budget)?;
    if entry_present(destination, platform)? {
        let destination_fingerprint = fingerprint(destination, platform, budget)?;
        if destination_fingerprint != source_fingerprint {
            return Err(io::Error::new(
                if prior_unverified_publication {
                    io::ErrorKind::InvalidData
                } else {
                    io::ErrorKind::AlreadyExists
                },
                if prior_unverified_publication {
                    format!("unverified {name} publication differs from its retained source")
                } else {
                    format!("{name} differs at source and destination; both copies were retained")
                },
            ));
        }
    } else {
        let stage = local.join(format!(".bareline-migration-stage-{name}"));
        if entry_present(&stage, platform)? && fingerprint(&stage, platform, budget)? != source_fingerprint {
            quarantine_stage(&stage, local, name, platform)?;
        }
        if !entry_present(&stage, platform)? {
            copy_entry(source, &stage, platform, budget, 0)
                .map_err(|error| migration_stage_error("stage copy", error))?;
        }
        let lease = platform
            .migration_entry_guard(&stage)
            .map_err(|error| migration_stage_error("stage root lease", error))?;
        let (staged_fingerprint, staged_children) = fingerprint_held(&stage, &lease, platform, budget)
            .map_err(|error| migration_stage_error("held stage fingerprint", error))?;
        let source_after_copy = fingerprint(source, platform, budget)?;
        if staged_fingerprint != source_after_copy || source_after_copy != source_fingerprint {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{name} changed while its staging copy was prepared"),
            ));
        }
        journal.items[index].source = Some(source_fingerprint.clone());
        transition(local, journal, index, ItemState::CopiedVerified, None, platform)?;
        let published_identity = lease.identity;
        // Windows cannot rename a nonempty directory while any descendant
        // handle remains open, even when those handles share delete. The root
        // lease still pins the staged identity across this required release.
        drop(staged_children);
        platform
            .publish_migration_entry(destination, lease)
            .map_err(|error| migration_stage_error("stage publication", error))?;
        let verification = fingerprint(destination, platform, budget).and_then(|fingerprint| {
            if fingerprint == source_fingerprint {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("published {name} failed verification"),
                ))
            }
        });
        if let Err(error) = verification {
            let quarantine = quarantine_published(destination, local, name, published_identity, platform);
            let detail = match quarantine {
                Ok(()) => error.to_string(),
                Err(quarantine_error) => {
                    format!("{error}; exact-identity quarantine failed: {quarantine_error}")
                }
            };
            return Err(migration_stage_error(
                "published destination verification",
                io::Error::new(error.kind(), detail),
            ));
        }
    }
    journal.items[index].source = Some(source_fingerprint.clone());
    let retained_detail = (journal.items[index].retirement == RetirementDisposition::RetainedByPolicy)
        .then(|| "directory source retained by policy; local publication remains authoritative".into());
    transition(local, journal, index, ItemState::Published, retained_detail, platform)?;
    if !retire_sources {
        return Ok(());
    }
    if journal.items[index].retirement == RetirementDisposition::RetainedByPolicy {
        return Ok(());
    }
    if source_fingerprint.directory {
        journal.items[index].retirement = RetirementDisposition::RetainedByPolicy;
        return transition(
            local,
            journal,
            index,
            ItemState::Published,
            Some("directory source retained by policy; exclusive generation authority unavailable".into()),
            platform,
        );
    }
    let lease = platform.migration_entry_guard(source)?;
    let (current, _source_children) = fingerprint_held(source, &lease, platform, budget)?;
    if current != source_fingerprint {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{name} changed after publication; source retained as a conflict"),
        ));
    }
    fs::create_dir_all(retained_root)?;
    let _retained_guard = platform.guard_directory(retained_root)?;
    let retained = retained_root.join(name);
    if entry_present(&retained, platform)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("retained source slot for {name} already exists"),
        ));
    }
    platform.publish_migration_entry(&retained, lease)?;
    journal.items[index].retirement = RetirementDisposition::Complete;
    transition(local, journal, index, ItemState::SourceRetired, None, platform)
}

fn probe_record_authority(
    record: Option<&ItemRecord>,
    local: &Path,
    legacy: &Path,
    platform: &dyn LocalFileSystem,
) -> ReadAuthority {
    let unverified_publication = record.is_some_and(|record| {
        record.source.is_some() && matches!(record.state, ItemState::CopiedVerified | ItemState::FailedRetryable)
    });
    if unverified_publication {
        return match platform.migration_entry_guard(legacy) {
            Ok(_) => ReadAuthority::Legacy,
            Err(_) => ReadAuthority::Unavailable,
        };
    }
    probe_authority(local, legacy, platform)
}

fn migration_stage_error(stage: &'static str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

fn entry_present(path: &Path, platform: &dyn LocalFileSystem) -> io::Result<bool> {
    match platform.migration_entry_guard(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn copy_entry(
    source: &Path,
    target: &Path,
    platform: &dyn LocalFileSystem,
    budget: &mut Budget<'_>,
    depth: usize,
) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "migration tree depth limit"));
    }
    budget.visit()?;
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "migration source is linked",
        ));
    }
    if metadata.is_dir() {
        let _source_guard = platform.guard_directory(source)?;
        fs::create_dir(target)?;
        let _target_guard = platform.guard_directory(target)?;
        for entry in sorted_entries(source, budget)? {
            copy_entry(
                &entry.path(),
                &target.join(entry.file_name()),
                platform,
                budget,
                depth + 1,
            )?;
        }
        Ok(())
    } else if metadata.is_file() {
        let source_lease = platform.migration_entry_guard(source)?;
        let mut input = platform.open_migration_read(&source_lease)?;
        let mut output = fs::OpenOptions::new().write(true).create_new(true).open(target)?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            budget.check()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            budget.bytes(count as u64)?;
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "migration source has unsupported type",
        ))
    }
}

fn fingerprint(path: &Path, platform: &dyn LocalFileSystem, budget: &mut Budget<'_>) -> io::Result<Fingerprint> {
    let mut hash = Sha256::new();
    let (directory, entries, bytes) = fingerprint_into(path, platform, budget, &mut hash, None, None, 0)?;
    Ok(Fingerprint {
        directory,
        entries,
        bytes,
        sha256: hash.finalize().into(),
    })
}

fn fingerprint_held(
    path: &Path,
    lease: &bareline_platform::CacheDirectoryLease,
    platform: &dyn LocalFileSystem,
    budget: &mut Budget<'_>,
) -> io::Result<(Fingerprint, Vec<bareline_platform::CacheDirectoryLease>)> {
    let mut hash = Sha256::new();
    let mut held_entries = Vec::new();
    let (directory, entries, bytes) = fingerprint_into(
        path,
        platform,
        budget,
        &mut hash,
        Some(lease),
        Some(&mut held_entries),
        0,
    )?;
    Ok((
        Fingerprint {
            directory,
            entries,
            bytes,
            sha256: hash.finalize().into(),
        },
        held_entries,
    ))
}

fn fingerprint_into(
    path: &Path,
    platform: &dyn LocalFileSystem,
    budget: &mut Budget<'_>,
    hash: &mut Sha256,
    current_lease: Option<&bareline_platform::CacheDirectoryLease>,
    mut held_entries: Option<&mut Vec<bareline_platform::CacheDirectoryLease>>,
    depth: usize,
) -> io::Result<(bool, u64, u64)> {
    if depth > MAX_DEPTH {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "migration tree depth limit"));
    }
    budget.visit()?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "migration entry is linked",
        ));
    }
    if metadata.is_dir() {
        let _guard = if current_lease.is_some() || held_entries.is_some() {
            None
        } else {
            Some(platform.guard_directory(path)?)
        };
        if current_lease.is_none()
            && let Some(held) = held_entries.as_deref_mut()
        {
            held.push(platform.migration_entry_guard(path)?);
        }
        hash.update([b'D']);
        let mut entries = 1u64;
        let mut bytes = 0u64;
        for entry in sorted_entries(path, budget)? {
            let name = encoded_name(&entry.file_name())?;
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(&name);
            let (_, child_entries, child_bytes) = fingerprint_into(
                &entry.path(),
                platform,
                budget,
                hash,
                None,
                held_entries.as_deref_mut(),
                depth + 1,
            )?;
            entries = entries.saturating_add(child_entries);
            bytes = bytes.saturating_add(child_bytes);
        }
        hash.update([b'E']);
        Ok((true, entries, bytes))
    } else if metadata.is_file() {
        let _lease = if current_lease.is_some() || held_entries.is_some() {
            None
        } else {
            Some(platform.migration_entry_guard(path)?)
        };
        let mut child_lease = None;
        if current_lease.is_none() && held_entries.is_some() {
            child_lease = Some(platform.migration_entry_guard(path)?);
        }
        let mut file = match current_lease.or(child_lease.as_ref()).or(_lease.as_ref()) {
            Some(lease) => platform.open_migration_read(lease)?,
            None => platform.open_sealed_read(path)?,
        };
        if let Some(lease) = child_lease
            && let Some(held) = held_entries.as_deref_mut()
        {
            held.push(lease);
        }
        let mut content_hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            budget.check()?;
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            budget.bytes(count as u64)?;
            bytes = bytes.saturating_add(count as u64);
            content_hash.update(&buffer[..count]);
        }
        hash.update([b'F']);
        hash.update(bytes.to_le_bytes());
        hash.update(content_hash.finalize());
        Ok((false, 1, bytes))
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "migration entry has unsupported type",
        ))
    }
}

fn sorted_entries(path: &Path, budget: &mut Budget<'_>) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        budget.visit()?;
        entries.push(entry?);
    }
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn encoded_name(name: &OsStr) -> io::Result<Vec<u8>> {
    bareline_platform::SerializedPath::from_native(Path::new(name))
        .identity_bytes()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn new_journal() -> Journal {
    Journal {
        version: VERSION,
        items: ITEMS
            .into_iter()
            .map(|name| ItemRecord {
                name: name.into(),
                state: ItemState::Pending,
                source: None,
                detail: None,
                retirement: RetirementDisposition::Pending,
            })
            .collect(),
    }
}

fn validate_journal(journal: &Journal) -> io::Result<()> {
    if journal.version != VERSION
        || journal.items.len() != ITEMS.len()
        || !journal
            .items
            .iter()
            .zip(ITEMS)
            .all(|(item, expected)| item.name == expected)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported migration journal",
        ));
    }
    Ok(())
}

fn load_journal(local: &Path, platform: &dyn LocalFileSystem) -> io::Result<Journal> {
    let path = local.join(JOURNAL);
    let lease = platform.migration_entry_guard(&path)?;
    let file = platform.open_migration_read(&lease)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid migration journal"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "migration journal limit"));
    }
    let journal = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid migration journal"))?;
    validate_journal(&journal)?;
    Ok(journal)
}

fn transition(
    local: &Path,
    journal: &mut Journal,
    index: usize,
    state: ItemState,
    detail: Option<String>,
    platform: &dyn LocalFileSystem,
) -> io::Result<()> {
    journal.items[index].state = state;
    journal.items[index].detail = detail;
    persist_journal(local, journal, platform)
}

fn persist_journal(local: &Path, journal: &Journal, platform: &dyn LocalFileSystem) -> io::Result<()> {
    let bytes = serde_json::to_vec(journal).map_err(io::Error::other)?;
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "migration journal limit"));
    }
    let path = local.join(JOURNAL);
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let stage = local.join(format!(
        "{JOURNAL}.new-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&stage)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    let existed = path.exists();
    match platform.commit(&stage, &path, existed) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(stage);
            Err(error)
        }
    }
}

fn quarantine_stage(stage: &Path, local: &Path, name: &str, platform: &dyn LocalFileSystem) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let retained = local.join(format!(
        ".bareline-migration-incomplete-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let lease = platform.migration_entry_guard(stage)?;
    platform.publish_migration_entry(&retained, lease)
}

fn quarantine_published(
    destination: &Path,
    local: &Path,
    name: &str,
    expected: bareline_platform::CacheDirectoryIdentity,
    platform: &dyn LocalFileSystem,
) -> io::Result<()> {
    let lease = platform.migration_entry_guard(destination)?;
    if lease.identity != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "published migration identity changed before quarantine",
        ));
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let retained = local.join(format!(
        ".bareline-migration-incomplete-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    platform.publish_migration_entry(&retained, lease)
}

struct Budget<'a> {
    entries: usize,
    bytes: u64,
    max_entries: usize,
    max_bytes: u64,
    started: Instant,
    max_time: Duration,
    cancelled: &'a dyn Fn() -> bool,
}
impl<'a> Budget<'a> {
    fn new(max_entries: usize, max_bytes: u64, max_time: Duration, cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            entries: 0,
            bytes: 0,
            max_entries,
            max_bytes,
            started: Instant::now(),
            max_time,
            cancelled,
        }
    }
    fn check(&self) -> io::Result<()> {
        if (self.cancelled)() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "profile migration cancelled",
            ))
        } else if self.started.elapsed() >= self.max_time {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "profile migration time budget exhausted",
            ))
        } else {
            Ok(())
        }
    }
    fn visit(&mut self) -> io::Result<()> {
        self.check()?;
        self.entries = self.entries.checked_add(1).ok_or_else(limit)?;
        if self.entries > self.max_entries {
            return Err(limit());
        }
        Ok(())
    }
    fn bytes(&mut self, count: u64) -> io::Result<()> {
        self.bytes = self.bytes.checked_add(count).ok_or_else(limit)?;
        if self.bytes > self.max_bytes {
            return Err(limit());
        }
        Ok(())
    }
    fn exhausted(&self) -> bool {
        self.entries >= self.max_entries
            || self.bytes >= self.max_bytes
            || self.started.elapsed() >= self.max_time
            || (self.cancelled)()
    }
}

fn limit() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "profile migration budget exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::{CacheDirectoryIdentity, CacheDirectoryLease, FileIdentity};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct FakeFileSystem {
        fail_reads: AtomicUsize,
        fail_publications: AtomicUsize,
        corrupt_publications: AtomicUsize,
        fail_quarantines: AtomicUsize,
        denied_root: std::sync::Mutex<Option<PathBuf>>,
    }
    impl FakeFileSystem {
        fn identity_for(path: &Path) -> io::Result<CacheDirectoryIdentity> {
            let metadata = fs::symlink_metadata(path)?;
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            Ok(CacheDirectoryIdentity {
                volume: metadata.len(),
                file: modified,
            })
        }
        fn consume(counter: &AtomicUsize) -> bool {
            counter
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| value.checked_sub(1))
                .is_ok()
        }
    }
    impl LocalFileSystem for FakeFileSystem {
        fn guard_directory(&self, path: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            if self
                .denied_root
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected unavailable root",
                ));
            }
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a plain directory"));
            }
            Ok(Arc::new(()))
        }
        fn migration_entry_guard(&self, path: &Path) -> io::Result<CacheDirectoryLease> {
            if self
                .denied_root
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|root| path.starts_with(root))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected unavailable root",
                ));
            }
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "linked entry"));
            }
            Ok(CacheDirectoryLease {
                path: path.to_path_buf(),
                identity: Self::identity_for(path)?,
                guard: Arc::new(()),
                migration_publisher: None,
            })
        }
        fn publish_migration_entry(&self, target: &Path, lease: CacheDirectoryLease) -> io::Result<()> {
            if target
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".bareline-migration-incomplete-"))
                && Self::consume(&self.fail_quarantines)
            {
                return Err(io::Error::other("injected quarantine failure"));
            }
            if Self::consume(&self.fail_publications) {
                return Err(io::Error::other("injected publication failure"));
            }
            if Self::identity_for(&lease.path)? != lease.identity {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "source identity changed"));
            }
            if target.exists() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "target exists"));
            }
            fs::rename(lease.path, target)?;
            if Self::consume(&self.corrupt_publications) {
                fs::write(target.join("injected-child"), b"changed after publication")?;
            }
            Ok(())
        }
        fn open_migration_read(&self, lease: &CacheDirectoryLease) -> io::Result<fs::File> {
            self.open_sealed_read(&lease.path)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<fs::File> {
            if Self::consume(&self.fail_reads) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected sharing violation",
                ));
            }
            fs::File::open(path)
        }
        fn identity(&self, _: &fs::File) -> io::Result<FileIdentity> {
            Ok(FileIdentity {
                volume: 0,
                file: 0,
                length: 0,
                modified: 0,
            })
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, existed: bool) -> io::Result<()> {
            if existed {
                fs::remove_file(target)?;
            }
            fs::rename(staged, target)
        }
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-profile-migration-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn roots(&self) -> (PathBuf, PathBuf) {
            let roaming = self.0.join("roaming");
            let local = self.0.join("local");
            fs::create_dir(&roaming).unwrap();
            (roaming, local)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(roaming: &Path, local: &Path, retire_sources: bool, platform: &FakeFileSystem) -> MigrationReport {
        migrate(
            MigrationRequest {
                roaming,
                local,
                retire_sources,
                max_entries: 1_024,
                max_io_bytes: 16 * 1024 * 1024,
                max_time: Duration::from_secs(5),
            },
            platform,
            &|| false,
        )
        .unwrap()
    }

    #[test]
    fn locked_item_retries_after_local_root_and_journal_exist() {
        let temp = Temp::new("locked-retry");
        let (roaming, local) = temp.roots();
        fs::write(roaming.join("settings.toml"), b"theme = 'dark'").unwrap();
        let platform = FakeFileSystem::default();
        platform.fail_reads.store(1, Ordering::SeqCst);

        let first = run(&roaming, &local, false, &platform);
        assert_eq!(first.state("settings.toml"), Some(ItemState::FailedRetryable));
        assert_eq!(first.authority("settings.toml"), Some(ReadAuthority::Legacy));
        assert_eq!(first.authority("session.json"), Some(ReadAuthority::Local));
        assert!(roaming.join("settings.toml").is_file());
        assert!(!local.join("settings.toml").exists());
        assert!(local.join(JOURNAL).is_file());

        let second = run(&roaming, &local, false, &platform);
        assert_eq!(second.state("settings.toml"), Some(ItemState::Published));
        assert_eq!(fs::read(local.join("settings.toml")).unwrap(), b"theme = 'dark'");
        assert!(roaming.join("settings.toml").is_file());
    }

    #[test]
    fn authority_handles_local_only_and_damaged_journal_without_empty_report_fallback() {
        let local_only = Temp::new("local-only-authority");
        let roaming = local_only.0.join("absent-roaming");
        let local = local_only.0.join("local");
        fs::create_dir(&local).unwrap();
        fs::write(local.join("session.json"), b"local session").unwrap();
        let platform = FakeFileSystem::default();
        let report = run(&roaming, &local, false, &platform);
        assert_eq!(report.authority("session.json"), Some(ReadAuthority::Local));
        let session = report.items.iter().find(|item| item.name == "session.json").unwrap();
        assert!(session.destination_present);
        assert!(!session.migrated);
        assert_eq!(report.items.len(), ITEMS.len());

        let empty = Temp::new("empty-profile-authority");
        let (roaming, local) = empty.roots();
        let report = run(&roaming, &local, false, &platform);
        let settings = report.items.iter().find(|item| item.name == "settings.toml").unwrap();
        assert_eq!(settings.authority, ReadAuthority::Local);
        assert!(!settings.destination_present);
        assert!(!settings.migrated);

        let damaged = Temp::new("damaged-journal-authority");
        let (roaming, local) = damaged.roots();
        fs::write(roaming.join("settings.toml"), b"legacy settings").unwrap();
        fs::create_dir(&local).unwrap();
        fs::write(local.join(JOURNAL), b"damaged").unwrap();
        let result = migrate(
            MigrationRequest {
                roaming: &roaming,
                local: &local,
                retire_sources: false,
                max_entries: 64,
                max_io_bytes: 1024,
                max_time: Duration::from_secs(1),
            },
            &platform,
            &|| false,
        );
        assert!(result.is_err());
        let authority = inspect_authorities(&roaming, &local, &platform);
        assert_eq!(authority.authority("settings.toml"), Some(ReadAuthority::Legacy));

        let unavailable = Temp::new("unavailable-authority");
        let (roaming, local) = unavailable.roots();
        let platform = FakeFileSystem::default();
        *platform.denied_root.lock().unwrap() = Some(roaming.clone());
        let authority = inspect_authorities(&roaming, &local, &platform);
        assert_eq!(authority.authority("settings.toml"), Some(ReadAuthority::Unavailable));
        assert!(authority.retryable);
    }

    #[test]
    fn publication_failure_resumes_from_verified_stage_without_removing_source() {
        let temp = Temp::new("publish-retry");
        let (roaming, local) = temp.roots();
        fs::create_dir(roaming.join("recovery")).unwrap();
        fs::write(roaming.join("recovery/generation.bin"), b"original recovery bytes").unwrap();
        let platform = FakeFileSystem::default();
        platform.fail_publications.store(1, Ordering::SeqCst);

        let first = run(&roaming, &local, false, &platform);
        assert_eq!(first.state("recovery"), Some(ItemState::FailedRetryable));
        assert!(roaming.join("recovery/generation.bin").is_file());
        assert!(!local.join("recovery").exists());

        let second = run(&roaming, &local, false, &platform);
        assert_eq!(second.state("recovery"), Some(ItemState::Published));
        assert_eq!(
            fs::read(local.join("recovery/generation.bin")).unwrap(),
            b"original recovery bytes"
        );
        assert!(roaming.join("recovery/generation.bin").is_file());
    }

    #[test]
    fn unverified_published_tree_keeps_legacy_authority_until_a_verified_retry() {
        let temp = Temp::new("post-publish-verification-retry");
        let (roaming, local) = temp.roots();
        fs::create_dir(roaming.join("recovery")).unwrap();
        fs::write(roaming.join("recovery/generation.bin"), b"original recovery bytes").unwrap();
        let platform = FakeFileSystem::default();
        platform.corrupt_publications.store(1, Ordering::SeqCst);
        platform.fail_quarantines.store(1, Ordering::SeqCst);

        let first = run(&roaming, &local, false, &platform);
        let recovery = first.items.iter().find(|item| item.name == "recovery").unwrap();
        assert_eq!(recovery.state, ItemState::FailedRetryable);
        assert_eq!(recovery.authority, ReadAuthority::Legacy);
        assert!(recovery.detail.as_deref().is_some_and(|detail| {
            detail.contains("published destination verification") && detail.contains("exact-identity quarantine failed")
        }));
        assert!(local.join("recovery/injected-child").is_file());
        assert_eq!(
            inspect_authorities(&roaming, &local, &platform).authority("recovery"),
            Some(ReadAuthority::Legacy)
        );

        let absent_source = temp.0.join("temporarily-absent-recovery");
        fs::rename(roaming.join("recovery"), &absent_source).unwrap();
        let absent = run(&roaming, &local, false, &platform);
        assert_eq!(absent.state("recovery"), Some(ItemState::FailedRetryable));
        assert_eq!(absent.authority("recovery"), Some(ReadAuthority::Unavailable));
        assert!(local.join("recovery/injected-child").is_file());
        assert_eq!(
            inspect_authorities(&roaming, &local, &platform).authority("recovery"),
            Some(ReadAuthority::Unavailable)
        );

        fs::rename(absent_source, roaming.join("recovery")).unwrap();
        fs::remove_file(local.join("recovery/injected-child")).unwrap();
        let second = run(&roaming, &local, false, &platform);
        assert_eq!(second.state("recovery"), Some(ItemState::Published));
        assert_eq!(second.authority("recovery"), Some(ReadAuthority::Local));
        assert_eq!(
            fs::read(local.join("recovery/generation.bin")).unwrap(),
            b"original recovery bytes"
        );
        assert!(roaming.join("recovery/generation.bin").is_file());
    }

    #[test]
    fn divergent_destination_is_a_lossless_conflict() {
        let temp = Temp::new("conflict");
        let (roaming, local) = temp.roots();
        fs::create_dir(&local).unwrap();
        fs::write(roaming.join("session.json"), b"old session").unwrap();
        fs::write(local.join("session.json"), b"new session").unwrap();

        let report = run(&roaming, &local, false, &FakeFileSystem::default());
        assert_eq!(report.state("session.json"), Some(ItemState::Conflict));
        assert_eq!(fs::read(roaming.join("session.json")).unwrap(), b"old session");
        assert_eq!(fs::read(local.join("session.json")).unwrap(), b"new session");
    }

    #[test]
    fn changed_source_is_rechecked_and_retained_before_retirement() {
        let temp = Temp::new("retirement-drift");
        let (roaming, local) = temp.roots();
        fs::write(roaming.join("settings.toml"), b"first generation").unwrap();
        let platform = FakeFileSystem::default();
        let first = run(&roaming, &local, false, &platform);
        assert_eq!(first.state("settings.toml"), Some(ItemState::Published));

        fs::write(roaming.join("settings.toml"), b"new roaming generation").unwrap();
        let second = run(&roaming, &local, true, &platform);
        assert_eq!(second.state("settings.toml"), Some(ItemState::Conflict));
        assert_eq!(fs::read(local.join("settings.toml")).unwrap(), b"first generation");
        assert_eq!(
            fs::read(roaming.join("settings.toml")).unwrap(),
            b"new roaming generation"
        );
    }

    #[test]
    fn verified_second_pass_retires_source_to_local_provenance() {
        let temp = Temp::new("retained-success");
        let (roaming, local) = temp.roots();
        fs::write(roaming.join("session.json"), b"recoverable session").unwrap();
        let platform = FakeFileSystem::default();

        let first = run(&roaming, &local, false, &platform);
        assert_eq!(first.state("session.json"), Some(ItemState::Published));
        assert!(roaming.join("session.json").is_file());

        let second = run(&roaming, &local, true, &platform);
        assert_eq!(second.state("session.json"), Some(ItemState::SourceRetired));
        assert!(!roaming.join("session.json").exists());
        assert_eq!(fs::read(local.join("session.json")).unwrap(), b"recoverable session");
        assert_eq!(
            fs::read(local.join(".bareline-migration-retained/session.json")).unwrap(),
            b"recoverable session"
        );
    }

    #[test]
    fn published_directory_is_retained_by_policy_without_a_retry_loop() {
        let temp = Temp::new("directory-retention-policy");
        let (roaming, local) = temp.roots();
        fs::create_dir(roaming.join("recovery")).unwrap();
        fs::write(roaming.join("recovery/original.bin"), b"original").unwrap();
        let platform = FakeFileSystem::default();
        assert_eq!(
            run(&roaming, &local, false, &platform).state("recovery"),
            Some(ItemState::Published)
        );
        let report = run(&roaming, &local, true, &platform);
        assert_eq!(report.state("recovery"), Some(ItemState::Published));
        assert!(!report.retryable);
        assert!(
            report
                .items
                .iter()
                .find(|item| item.name == "recovery")
                .and_then(|item| item.detail.as_deref())
                .is_some_and(|detail| detail.contains("retained by policy"))
        );
        assert!(roaming.join("recovery/original.bin").is_file());
        assert!(local.join("recovery/original.bin").is_file());
        assert!(!retirement_ready(&local, &platform));

        let repeated = run(&roaming, &local, true, &platform);
        assert_eq!(repeated.state("recovery"), Some(ItemState::Published));
        assert!(!repeated.retryable);
    }

    #[test]
    fn canonical_tree_hash_distinguishes_equal_size_equal_count_structures() {
        let temp = Temp::new("tree-framing");
        let left = temp.0.join("left");
        let right = temp.0.join("right");
        let nested = temp.0.join("nested");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(left.join("a"), b"bc").unwrap();
        fs::write(left.join("b"), b"d").unwrap();
        fs::write(right.join("ab"), b"c").unwrap();
        fs::write(right.join("d"), b"bc").unwrap();
        fs::create_dir(nested.join("a")).unwrap();
        fs::write(nested.join("a/b"), b"bcd").unwrap();
        let platform = FakeFileSystem::default();
        let mut left_budget = Budget::new(64, 1024, Duration::from_secs(1), &|| false);
        let mut right_budget = Budget::new(64, 1024, Duration::from_secs(1), &|| false);
        let mut nested_budget = Budget::new(64, 1024, Duration::from_secs(1), &|| false);
        let left = fingerprint(&left, &platform, &mut left_budget).unwrap();
        let right = fingerprint(&right, &platform, &mut right_budget).unwrap();
        let nested = fingerprint(&nested, &platform, &mut nested_budget).unwrap();
        assert_eq!(left.entries, right.entries);
        assert_eq!(left.bytes, right.bytes);
        assert_ne!(left.sha256, right.sha256);
        assert_eq!(left.entries, nested.entries);
        assert_eq!(left.bytes, nested.bytes);
        assert_ne!(left.sha256, nested.sha256);
    }

    #[test]
    fn flat_and_deep_trees_stop_with_source_retained() {
        let temp = Temp::new("tree-bounds");
        let (roaming, local) = temp.roots();
        let recovery = roaming.join("recovery");
        fs::create_dir(&recovery).unwrap();
        for index in 0..32 {
            fs::write(recovery.join(format!("{index:02}.bin")), b"x").unwrap();
        }
        let platform = FakeFileSystem::default();
        let report = migrate(
            MigrationRequest {
                roaming: &roaming,
                local: &local,
                retire_sources: false,
                max_entries: 8,
                max_io_bytes: 1024,
                max_time: Duration::from_secs(1),
            },
            &platform,
            &|| false,
        )
        .unwrap();
        assert_eq!(report.state("recovery"), Some(ItemState::FailedRetryable));
        assert!(recovery.is_dir());

        let deep = roaming.join("macros");
        fs::create_dir(&deep).unwrap();
        let mut cursor = deep.clone();
        for _ in 0..=MAX_DEPTH {
            cursor = cursor.join("d");
            fs::create_dir(&cursor).unwrap();
        }
        let report = run(&roaming, &local, false, &platform);
        assert_eq!(report.state("macros"), Some(ItemState::FailedRetryable));
        assert!(deep.is_dir());
    }
}
