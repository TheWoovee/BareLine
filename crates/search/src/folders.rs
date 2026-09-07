// SPDX-License-Identifier: MPL-2.0
//! Bounded, read-only UTF-8 folder scanning. Call on a worker with real platform trust.
use super::*;
use bareline_document::Budget;
use bareline_file_io::lifecycle::{DecodeOptions, FileError, Fingerprint, open_encoded_streaming};
use bareline_platform::{
    LocalFileSystem, PathOperation, PathOrigin, PathTrustProvider, TrustedRead,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_DEPTH: usize = 64;
const MAX_PATH: usize = 32768;
const EXCERPT_BYTES: usize = 160;
#[derive(Clone)]
pub struct FolderScope {
    pub root: PathBuf,
    pub origin: PathOrigin,
    pub include_binary: bool,
    /// Exact, case-insensitive extensions without a dot. Empty includes all extensions.
    pub extensions: Vec<String>,
    /// Exact directory names. No implicit gitignore/glob interpretation.
    pub excluded_directory_names: Vec<String>,
}
impl FolderScope {
    pub fn user(root: PathBuf) -> Self {
        Self {
            root,
            origin: PathOrigin::User,
            include_binary: false,
            extensions: Vec::new(),
            excluded_directory_names: vec![".git".into(), ".svn".into(), ".hg".into()],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderSkip {
    Untrusted,
    Symlink,
    Io,
    Changed,
    UnsupportedEncoding,
    SubjectLimit,
    Binary,
    DepthLimit,
    ResourceLimit,
}
pub struct FolderMatch {
    pub range: Range<TextOffset>,
    pub excerpt_start: TextOffset,
    pub excerpt: String,
}
/// Borrowed events never retain a whole file. File fingerprints bind navigation to
/// the searched disk version; callers must revalidate before using emitted ranges.
pub enum FolderEvent<'a> {
    Batch {
        path: &'a Path,
        fingerprint: &'a Fingerprint,
        matches: &'a [FolderMatch],
    },
    Skipped {
        path: &'a Path,
        reason: FolderSkip,
    },
}
pub struct FolderSummary {
    pub searched_files: usize,
    pub skipped_files: usize,
    pub count: usize,
    pub completeness: Completeness,
}
pub struct FolderGroup {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub matches: Vec<FolderMatch>,
}
pub struct FolderResults {
    pub groups: Vec<FolderGroup>,
    pub skips: Vec<(PathBuf, FolderSkip)>,
    pub summary: FolderSummary,
}
pub fn collect_folder(
    scope: &FolderScope,
    query: &SearchQuery,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
) -> FolderResults {
    let mut groups: Vec<FolderGroup> = Vec::new();
    let mut skips = Vec::new();
    let summary = scan_folder(scope, query, job, trust, platform, |event| match event {
        FolderEvent::Batch {
            path,
            fingerprint,
            matches,
        } => {
            if groups.last().is_none_or(|group| group.path != path) {
                groups.push(FolderGroup {
                    path: path.into(),
                    fingerprint: fingerprint.clone(),
                    matches: Vec::new(),
                });
            }
            let group = groups.last_mut().unwrap();
            group
                .matches
                .extend(matches.iter().map(|matched| FolderMatch {
                    range: matched.range.clone(),
                    excerpt_start: matched.excerpt_start,
                    excerpt: matched.excerpt.clone(),
                }));
        }
        FolderEvent::Skipped { path, reason } => {
            // Diagnostics have an independent finite cap; terminal summary retains total skips.
            if skips.len() < 256 {
                skips.push((path.into(), reason));
            }
        }
    });
    FolderResults {
        groups,
        skips,
        summary,
    }
}
fn skip(
    summary: &mut FolderSummary,
    path: &Path,
    reason: FolderSkip,
    emit: &mut impl FnMut(FolderEvent<'_>),
) {
    summary.skipped_files += 1;
    if reason != FolderSkip::Binary && summary.completeness == Completeness::Complete {
        summary.completeness = Completeness::Unsupported;
    }
    emit(FolderEvent::Skipped { path, reason });
}
fn trusted(
    path: &Path,
    root: Option<&Path>,
    scope: &FolderScope,
    trust: &dyn PathTrustProvider,
) -> Result<TrustedRead, FolderSkip> {
    if path.as_os_str().len() > MAX_PATH {
        return Err(FolderSkip::ResourceLimit);
    }
    let approved = trust
        .open_read(path, scope.origin)
        .map_err(|_| FolderSkip::Untrusted)?;
    let classified = &approved.trust;
    if !trust.permits(&classified, PathOperation::Read) {
        return Err(FolderSkip::Untrusted);
    }
    if classified.traverses_reparse_point {
        return Err(FolderSkip::Symlink);
    }
    if root.is_some_and(|root| !classified.canonical.starts_with(root)) {
        return Err(FolderSkip::Untrusted);
    }
    if fs::symlink_metadata(path)
        .map_err(|_| FolderSkip::Io)?
        .file_type()
        .is_symlink()
    {
        return Err(FolderSkip::Symlink);
    }
    Ok(approved)
}
/// One worker and at most 64 live directory iterators. Reads each eligible file through
/// the identity-checking UTF-8 lifecycle, capped at 16 MiB; larger/other codecs are visible
/// skips. Results stream with a cumulative output budget; no folder replace API exists.
pub fn scan_folder(
    scope: &FolderScope,
    query: &SearchQuery,
    job: &SearchJob,
    trust: &dyn PathTrustProvider,
    platform: &dyn LocalFileSystem,
    mut emit: impl FnMut(FolderEvent<'_>),
) -> FolderSummary {
    let mut summary = FolderSummary {
        searched_files: 0,
        skipped_files: 0,
        count: 0,
        completeness: Completeness::Complete,
    };
    if job.is_cancelled() {
        summary.completeness = Completeness::Cancelled;
        return summary;
    }
    if query.selection.is_some()
        || scope.extensions.len() > 256
        || scope.excluded_directory_names.len() > 256
        || scope
            .extensions
            .iter()
            .chain(&scope.excluded_directory_names)
            .any(|s| s.len() > 256)
    {
        summary.completeness = Completeness::InvalidQuery;
        return summary;
    }
    let root_guard = match trusted(&scope.root, None, scope, trust) {
        Ok(root) => root,
        Err(reason) => {
            skip(&mut summary, &scope.root, reason, &mut emit);
            return summary;
        }
    };
    let root = root_guard.trust.canonical.clone();
    let initial = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(_) => {
            skip(&mut summary, &root, FolderSkip::Io, &mut emit);
            return summary;
        }
    };
    let mut stack = vec![(root.clone(), initial, root_guard)];
    let mut remaining = query.results_ram_bytes.min(MAX_RESULT_BYTES);
    while let Some((directory, entries, _guard)) = stack.last_mut() {
        if job.is_cancelled() {
            summary.completeness = Completeness::Cancelled;
            break;
        }
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                skip(&mut summary, directory, FolderSkip::Io, &mut emit);
                continue;
            }
        };
        let path = entry.path();
        let guard = match trusted(&path, Some(&root), scope, trust) {
            Ok(path) => path,
            Err(reason) => {
                skip(&mut summary, &path, reason, &mut emit);
                continue;
            }
        };
        let path = guard.trust.canonical.clone();
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(_) => {
                skip(&mut summary, &path, FolderSkip::Io, &mut emit);
                continue;
            }
        };
        if kind.is_dir() {
            if scope
                .excluded_directory_names
                .iter()
                .any(|name| entry.file_name() == name.as_str())
            {
                continue;
            }
            if stack.len() == MAX_DEPTH {
                skip(&mut summary, &path, FolderSkip::DepthLimit, &mut emit);
                continue;
            }
            match fs::read_dir(&path) {
                Ok(entries) => stack.push((path, entries, guard)),
                Err(_) => skip(&mut summary, &path, FolderSkip::Io, &mut emit),
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        if !scope.extensions.is_empty()
            && !path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|extension| {
                    scope
                        .extensions
                        .iter()
                        .any(|filter| extension.eq_ignore_ascii_case(filter))
                })
        {
            continue;
        }
        // Keep approved ancestry alive through the complete read and result scan.
        let _ancestors = guard.ancestors;
        let expected_identity = match platform.identity(&guard.file) {
            Ok(identity) => identity,
            Err(_) => {
                skip(&mut summary, &path, FolderSkip::Io, &mut emit);
                continue;
            }
        };
        let opened = match open_encoded_streaming(
            &path,
            platform,
            Budget::new(regex::SUBJECT_LIMIT * 8),
            Budget::new(0),
            &job.io_cancel,
            DecodeOptions {
                resident_max_bytes: regex::SUBJECT_LIMIT as u64,
                interpret: None,
            },
            |_| {},
        ) {
            Ok(opened) => opened,
            Err(FileError::Cancelled) => {
                summary.completeness = Completeness::Cancelled;
                break;
            }
            Err(error) => {
                let reason = match error {
                    FileError::Changed => FolderSkip::Changed,
                    FileError::UnsupportedEncoding => FolderSkip::UnsupportedEncoding,
                    FileError::StreamingRequired => FolderSkip::SubjectLimit,
                    FileError::Budget => FolderSkip::ResourceLimit,
                    _ => FolderSkip::Io,
                };
                skip(&mut summary, &path, reason, &mut emit);
                continue;
            }
        };
        if opened.fingerprint.identity != expected_identity
            || platform.identity(&guard.file).ok().as_ref() != Some(&expected_identity)
        {
            skip(&mut summary, &path, FolderSkip::Changed, &mut emit);
            continue;
        }
        let snapshot = opened.document.snapshot();
        if !scope.include_binary
            && snapshot
                .chunks(TextOffset(0)..TextOffset(snapshot.len()))
                .unwrap()
                .any(|chunk| chunk.as_bytes().contains(&0))
        {
            skip(&mut summary, &path, FolderSkip::Binary, &mut emit);
            continue;
        }
        let mut scoped = query.clone();
        scoped.results_ram_bytes = remaining;
        let results = scan(&snapshot, &scoped, job, |_| {});
        summary.searched_files += 1;
        let mut file_overhead = path.as_os_str().len() + std::mem::size_of::<Fingerprint>();
        for found in results.matches() {
            if job.is_cancelled() {
                summary.completeness = Completeness::Cancelled;
                break;
            }
            let mut start = found.range.start.0.saturating_sub(EXCERPT_BYTES / 2);
            while !snapshot.is_boundary(TextOffset(start)) {
                start += 1;
            }
            let mut end = (start + EXCERPT_BYTES).min(snapshot.len());
            while !snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let excerpt = snapshot
                .read(TextOffset(start)..TextOffset(end), EXCERPT_BYTES)
                .unwrap();
            let used = file_overhead + std::mem::size_of::<FolderMatch>() + excerpt.capacity();
            if used > remaining {
                summary.completeness = Completeness::ResultLimit;
                break;
            }
            remaining -= used;
            file_overhead = 0;
            let batch = [FolderMatch {
                range: found.range.clone(),
                excerpt_start: TextOffset(start),
                excerpt,
            }];
            emit(FolderEvent::Batch {
                path: &path,
                fingerprint: &opened.fingerprint,
                matches: &batch,
            });
            summary.count += 1;
        }
        if matches!(
            summary.completeness,
            Completeness::ResultLimit | Completeness::Cancelled
        ) {
            break;
        }
        if results.completeness() != Completeness::Complete {
            summary.completeness = results.completeness();
            break;
        }
    }
    if job.is_cancelled() {
        summary.completeness = Completeness::Cancelled;
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::{FileIdentity, PathTrust, StorageKind};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "bareline-search-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, bytes: &[u8]) {
            fs::write(self.0.join(name), bytes).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct TestPlatform {
        allow: bool,
        fail_identity: bool,
    }
    impl PathTrustProvider for TestPlatform {
        fn open_read(&self, path: &Path, origin: PathOrigin) -> std::io::Result<TrustedRead> {
            let trust = self.canonicalize(path, origin)?;
            // This fixture mocks directory capability ownership only. Production uses
            // the Windows provider's no-delete directory and ancestor handles.
            let file = std::fs::File::open(if path.is_dir() {
                std::env::current_exe()?
            } else {
                path.to_owned()
            })?;
            Ok(TrustedRead {
                trust,
                file,
                ancestors: Vec::new(),
            })
        }
        fn canonicalize(&self, path: &Path, origin: PathOrigin) -> std::io::Result<PathTrust> {
            Ok(PathTrust {
                canonical: path.canonicalize()?,
                storage: StorageKind::Local,
                origin,
                traverses_reparse_point: false,
            })
        }
        fn permits(&self, _: &PathTrust, _: PathOperation) -> bool {
            self.allow
        }
    }
    impl LocalFileSystem for TestPlatform {
        fn identity(&self, file: &std::fs::File) -> std::io::Result<FileIdentity> {
            if self.fail_identity {
                return Err(std::io::Error::other("fixture identity failure"));
            }
            let metadata = file.metadata()?;
            Ok(FileIdentity {
                volume: 0,
                file: 1,
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> std::io::Result<()> {
            panic!("search must not write")
        }
    }
    #[test]
    fn folder_streams_identity_ranges_and_visible_skips_without_retaining_files() {
        let fixture = Fixture::new();
        fixture.write("a.txt", b"x x");
        fixture.write("binary", b"x\0");
        fixture.write("legacy", &[0xff]);
        fs::create_dir(fixture.0.join(".git")).unwrap();
        fixture.write(".git/ignored", b"x");
        let platform = TestPlatform {
            allow: true,
            fail_identity: false,
        };
        let mut ranges = Vec::new();
        let mut skips = Vec::new();
        let summary = scan_folder(
            &FolderScope::user(fixture.0.clone()),
            &SearchQuery::literal("x"),
            &SearchJob::default(),
            &platform,
            &platform,
            |event| match event {
                FolderEvent::Batch {
                    path,
                    fingerprint,
                    matches,
                } => {
                    assert_eq!(path.file_name().unwrap(), "a.txt");
                    assert_eq!(fingerprint.identity.length, 3);
                    ranges.extend(matches.iter().map(|m| m.range.clone()));
                    assert!(matches.iter().all(|m| m.excerpt == "x x"));
                }
                FolderEvent::Skipped { reason, .. } => skips.push(reason),
            },
        );
        assert_eq!(summary.count, 2);
        // The PR-007 encoded lifecycle now searches the decoded legacy/opaque
        // text view too; it has no "x" hit and retains the original byte provenance.
        assert_eq!(summary.searched_files, 2);
        assert_eq!(
            ranges,
            [TextOffset(0)..TextOffset(1), TextOffset(2)..TextOffset(3)]
        );
        assert!(skips.contains(&FolderSkip::Binary));
        assert!(!skips.contains(&FolderSkip::UnsupportedEncoding));
        assert_eq!(summary.completeness, Completeness::Complete);
    }
    #[test]
    fn folder_trust_io_limit_and_cancellation_fail_closed() {
        let fixture = Fixture::new();
        fixture.write("a", b"x x");
        let scope = FolderScope::user(fixture.0.clone());
        let query = SearchQuery::literal("x");
        let denied = TestPlatform {
            allow: false,
            fail_identity: false,
        };
        let summary = scan_folder(
            &scope,
            &query,
            &SearchJob::default(),
            &denied,
            &denied,
            |event| {
                assert!(matches!(
                    event,
                    FolderEvent::Skipped {
                        reason: FolderSkip::Untrusted,
                        ..
                    }
                ))
            },
        );
        assert_eq!(summary.count, 0);
        let failed = TestPlatform {
            allow: true,
            fail_identity: true,
        };
        let summary = scan_folder(
            &scope,
            &query,
            &SearchJob::default(),
            &failed,
            &failed,
            |event| {
                assert!(matches!(
                    event,
                    FolderEvent::Skipped {
                        reason: FolderSkip::Io,
                        ..
                    }
                ))
            },
        );
        assert_eq!(summary.completeness, Completeness::Unsupported);
        let platform = TestPlatform {
            allow: true,
            fail_identity: false,
        };
        let job = SearchJob::default();
        let summary = scan_folder(&scope, &query, &job, &platform, &platform, |event| {
            if matches!(event, FolderEvent::Batch { .. }) {
                job.cancel();
            }
        });
        assert_eq!(summary.count, 1);
        assert_eq!(summary.completeness, Completeness::Cancelled);
        let mut bounded = query;
        bounded.results_ram_bytes = 1;
        let summary = scan_folder(
            &scope,
            &bounded,
            &SearchJob::default(),
            &platform,
            &platform,
            |_| {},
        );
        assert_eq!(summary.count, 0);
        assert_eq!(summary.completeness, Completeness::ResultLimit);
    }
}
