// SPDX-License-Identifier: MPL-2.0
//! Incremental conservative tail continuity verification. Rehashes the complete old
//! prefix before accepting growth; O(file length) I/O, constant verification RAM.
use crate::{
    cancellation::Cancellation,
    lifecycle::{FileError, Fingerprint},
};
use crate::{
    codecs::disk::{DiskDecoded, DiskOptions, DiskTranscoder},
    lifecycle::{FileInput, PagedOpened},
    source::{FileSource, SourceOptions},
};
use bareline_document::{Budget, TextOffset, source::PageTicket};
use bareline_platform::{FileIdentity, LocalFileSystem};
use sha2::{Digest, Sha256};
use std::io::{Seek, SeekFrom, Write};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
fn follow_identity(platform: &dyn LocalFileSystem, path: &Path) -> Result<FileIdentity, FileError> {
    let (file, _guard) = platform.open_follow_read(path)?;
    Ok(platform.identity(&file)?)
}

/// Follow owns immutable suffix stores. Existing snapshots keep their original source
/// generations; only the decoder's final opaque unit is copied into the next suffix.
/// Continuity verification is conservative full-prefix I/O, performed in bounded steps.
pub struct TailSession {
    _cache_guard: Arc<dyn Send + Sync>,
    last_append: Option<AppendReceipt>,
    platform: Arc<dyn LocalFileSystem>,
    budget: Budget,
    cancellation: Cancellation,
    cache: PathBuf,
    encoding: crate::codecs::Encoding,
    fingerprint: Fingerprint,
    raw_start: u64,
    text_start: u64,
    segments: Vec<(FileSource, DiskDecoded, u64)>,
    phase: Option<TailPhase>,
    pub source_changed: bool,
}
/// Published only after the verified decoded suffix becomes the document's new root.
#[derive(Clone, Copy, Debug)]
pub struct AppendReceipt {
    pub generation: bareline_document::source::Generation,
    pub revision: bareline_document::Revision,
    pub raw_bytes: u64,
    pub text_bytes: usize,
    pub applied_at: std::time::Instant,
}
enum TailPhase {
    Verify(TailVerifier),
    Copy {
        file: File,
        output: File,
        scratch: Scratch,
        remaining: u64,
        verified: Fingerprint,
        _guard: Arc<dyn Send + Sync>,
    },
    Decode {
        job: DiskTranscoder,
        scratch: Scratch,
        verified: Fingerprint,
    },
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl TailSession {
    pub fn new(
        opened: &PagedOpened,
        platform: Arc<dyn LocalFileSystem>,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        let (raw_start, text_start) = opened.transcoded.store.tail_boundary().map_err(FileError::Transcode)?;
        let cache = opened
            .transcoded
            .store
            .text_path()
            .parent()
            .and_then(Path::parent)
            .ok_or(FileError::IncompleteSource)?
            .to_owned();
        let cache_guard = platform.guard_directory(&cache)?;
        Ok(Self {
            _cache_guard: cache_guard,
            last_append: None,
            platform,
            budget,
            cancellation,
            cache,
            encoding: opened.transcoded.store.state.interpreted(),
            fingerprint: opened.fingerprint.clone(),
            raw_start,
            text_start,
            segments: Vec::new(),
            phase: None,
            source_changed: false,
        })
    }
    pub fn pending(&self) -> bool {
        self.phase.is_some()
    }
    pub fn append_receipt(&self) -> Option<AppendReceipt> {
        self.last_append
    }
    /// Unlock captures a sealed, provenance-complete fixed generation before editing.
    /// If the path no longer matches the followed generation, preserve the viewer.
    pub fn freeze(&self, opened: &PagedOpened) -> Result<Box<PagedOpened>, FileError> {
        if self.pending() {
            return Err(FileError::Changed);
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let raw_path = self.cache.join(format!(
            "bareline-tail-fixed-{}-{}.raw",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let scratch = Scratch(raw_path.clone());
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&raw_path)?;
        self.export_original(opened, &mut output)?;
        output.sync_all()?;
        drop(output);
        let outcome = crate::lifecycle::open_paged_encoded(
            crate::lifecycle::PagedOpenRequest {
                path: raw_path,
                bytes: self.budget.clone(),
                history: Budget::new(128 * 1024 * 1024),
                cache: self.cache.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 20 * 1024 * 1024 * 1024,
                    interpret: Some(self.encoding),
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            self.platform.clone(),
            self.cancellation.clone(),
            |_| {},
        );
        match outcome {
            crate::lifecycle::TranscodeOutcome::Complete(mut fixed)
                if fixed.fingerprint.sha256 == self.fingerprint.sha256 =>
            {
                fixed.path = opened.path.clone();
                fixed.fingerprint = self.fingerprint.clone();
                fixed.transcoded.store.fingerprint = self.fingerprint.clone();
                drop(scratch);
                Ok(fixed)
            }
            crate::lifecycle::TranscodeOutcome::Failed(error) => Err(error),
            _ => Err(FileError::Changed),
        }
    }
    /// Reconstruct the complete followed raw generation from owned stores. Overlapping
    /// decoder-boundary suffixes replace prior bytes; they are never duplicated or skipped.
    pub fn export_original(&self, opened: &PagedOpened, output: &mut dyn Write) -> Result<(), FileError> {
        let mut write_part = |store: &DiskDecoded, count: u64| -> Result<(), FileError> {
            if count > store.raw_len {
                return Err(FileError::IncompleteSource);
            }
            let mut input = store
                .sealed_original_reader(&self.cancellation)
                .map_err(FileError::Transcode)?;
            let mut remaining = count;
            let mut bytes = [0; 65536];
            while remaining != 0 {
                self.cancellation.check()?;
                let count = remaining.min(bytes.len() as u64) as usize;
                input.read_exact(&mut bytes[..count])?;
                output.write_all(&bytes[..count])?;
                remaining -= count as u64;
            }
            Ok(())
        };
        let first_end = self
            .segments
            .first()
            .map_or(self.fingerprint.identity.length, |(_, _, base)| *base);
        write_part(&opened.transcoded.store, first_end)?;
        for (index, (_, store, base)) in self.segments.iter().enumerate() {
            let end = self
                .segments
                .get(index + 1)
                .map_or(self.fingerprint.identity.length, |(_, _, next)| *next);
            write_part(store, end.checked_sub(*base).ok_or(FileError::IncompleteSource)?)?;
        }
        Ok(())
    }
    pub fn request(&mut self, path: &Path) -> Result<(), FileError> {
        if self.phase.is_none() && !self.source_changed {
            self.phase = Some(TailPhase::Verify(TailVerifier::begin(
                path,
                self.fingerprint.clone(),
                self.platform.clone(),
                self.cancellation.clone(),
            )?));
        }
        Ok(())
    }
    /// One bounded read/copy/decode step. A true result publishes a logical revision.
    pub fn step(&mut self, opened: &mut PagedOpened) -> Result<bool, FileError> {
        self.cancellation.check()?;
        let Some(phase) = self.phase.take() else {
            return Ok(false);
        };
        match phase {
            TailPhase::Verify(mut verifier) => match verifier.step()? {
                TailProgress::Pending { .. } => self.phase = Some(TailPhase::Verify(verifier)),
                TailProgress::SourceChanged => self.source_changed = true,
                TailProgress::Verified(verified) => {
                    if verified.identity.length == self.fingerprint.identity.length {
                        return Ok(false);
                    }
                    let (mut file, guard) = self.platform.open_follow_read(&opened.path)?;
                    if self.platform.identity(&file)? != verified.identity {
                        self.source_changed = true;
                        return Ok(false);
                    }
                    file.seek(SeekFrom::Start(self.raw_start))?;
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    let path = self.cache.join(format!(
                        "bareline-tail-{}-{}.raw",
                        std::process::id(),
                        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    ));
                    let output = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
                    self.phase = Some(TailPhase::Copy {
                        file,
                        output,
                        scratch: Scratch(path),
                        remaining: verified.identity.length - self.raw_start,
                        verified,
                        _guard: guard,
                    });
                }
            },
            TailPhase::Copy {
                mut file,
                mut output,
                scratch,
                mut remaining,
                verified,
                _guard,
            } => {
                if self.platform.identity(&file)? != verified.identity {
                    self.source_changed = true;
                    return Ok(false);
                }
                let mut bytes = [0; 65536];
                let count = remaining.min(bytes.len() as u64) as usize;
                file.read_exact(&mut bytes[..count])?;
                output.write_all(&bytes[..count])?;
                remaining -= count as u64;
                if remaining != 0 {
                    self.phase = Some(TailPhase::Copy {
                        file,
                        output,
                        scratch,
                        remaining,
                        verified,
                        _guard,
                    });
                } else {
                    output.sync_all()?;
                    drop(output);
                    if self.platform.identity(&file)? != verified.identity
                        || follow_identity(self.platform.as_ref(), &opened.path)? != verified.identity
                    {
                        self.source_changed = true;
                        return Ok(false);
                    }
                    let job = DiskTranscoder::continuation(
                        FileInput {
                            file: File::open(&scratch.0)?,
                            path: scratch.0.clone(),
                        },
                        self.platform.clone(),
                        &self.cache,
                        DiskOptions {
                            temp_quota_bytes: 20 * 1024 * 1024 * 1024,
                            interpret: Some(self.encoding),
                        },
                        self.budget.clone(),
                        self.cancellation.clone(),
                    )
                    .map_err(FileError::Transcode)?;
                    self.phase = Some(TailPhase::Decode { job, scratch, verified });
                }
            }
            TailPhase::Decode {
                mut job,
                scratch,
                verified,
            } => {
                if !job.step().map_err(FileError::Transcode)?.complete {
                    self.phase = Some(TailPhase::Decode { job, scratch, verified });
                } else {
                    let store = job.finish().map_err(FileError::Transcode)?;
                    let (raw_next, text_next) = store.tail_boundary().map_err(FileError::Transcode)?;
                    let source = FileSource::open(
                        &store.text_path(),
                        self.platform.clone(),
                        SourceOptions {
                            resident_max_bytes: 0,
                            ..SourceOptions::default()
                        },
                        self.budget.clone(),
                        self.cancellation.clone(),
                    )?;
                    opened
                        .transcoded
                        .document
                        .replace_tail_source(
                            TextOffset(usize::try_from(self.text_start).map_err(|_| FileError::Budget)?),
                            source.source(),
                        )
                        .map_err(|_| FileError::Budget)?;
                    let raw_base = self.raw_start;
                    self.raw_start += raw_next;
                    self.text_start += text_next;
                    self.fingerprint = verified.clone();
                    opened.fingerprint = verified;
                    let snapshot = opened.transcoded.document.snapshot();
                    self.last_append = Some(AppendReceipt {
                        generation: source.source().generation(),
                        revision: snapshot.revision,
                        raw_bytes: self.fingerprint.identity.length,
                        text_bytes: snapshot.len(),
                        applied_at: std::time::Instant::now(),
                    });
                    self.segments.push((source, store, raw_base));
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// Returns false for the original source, which remains owned by PagedOpened.
    pub fn read_page(&mut self, ticket: PageTicket) -> Result<bool, FileError> {
        if let Some((source, _, _)) = self
            .segments
            .iter_mut()
            .find(|(source, _, _)| source.source().generation() == ticket.generation)
        {
            source.read_page(ticket)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
pub enum TailProgress {
    Pending { verified: u64, total: u64 },
    Verified(Fingerprint),
    SourceChanged,
}
pub struct TailVerifier {
    _path_guard: Arc<dyn Send + Sync>,
    path: PathBuf,
    file: File,
    platform: Arc<dyn LocalFileSystem>,
    expected: Fingerprint,
    current: FileIdentity,
    hash: Sha256,
    offset: u64,
    prefix_verified: bool,
    changed: bool,
    cancellation: Cancellation,
}
impl TailVerifier {
    /// Establish an initial full fingerprint against the displayed source identity.
    /// The viewport remains usable while the caller drives these bounded steps.
    pub fn baseline(
        path: &Path,
        displayed: FileIdentity,
        platform: Arc<dyn LocalFileSystem>,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        let mut empty = displayed;
        empty.length = 0;
        let expected = Fingerprint {
            identity: empty,
            sha256: Sha256::digest([]).into(),
        };
        let mut verifier = Self::begin(path, expected, platform, cancellation)?;
        verifier.changed |= verifier.current != displayed;
        Ok(verifier)
    }
    /// Caller must have authorized the path and captured a complete old fingerprint.
    pub fn begin(
        path: &Path,
        expected: Fingerprint,
        platform: Arc<dyn LocalFileSystem>,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        cancellation.check()?;
        let (file, path_guard) = platform.open_follow_read(path)?;
        let current = platform.identity(&file)?;
        let changed = current.volume != expected.identity.volume
            || current.file != expected.identity.file
            || current.length < expected.identity.length;
        Ok(Self {
            _path_guard: path_guard,
            path: path.into(),
            file,
            platform,
            expected,
            current,
            hash: Sha256::new(),
            offset: 0,
            prefix_verified: false,
            changed,
            cancellation,
        })
    }
    /// One bounded 64 KiB step on an I/O worker. No bytes are accepted before verification.
    pub fn step(&mut self) -> Result<TailProgress, FileError> {
        self.cancellation.check()?;
        if self.changed {
            return Ok(TailProgress::SourceChanged);
        }
        if self.platform.identity(&self.file)? != self.current
            || follow_identity(self.platform.as_ref(), &self.path)? != self.current
        {
            self.changed = true;
            return Ok(TailProgress::SourceChanged);
        }
        if !self.prefix_verified && self.offset == self.expected.identity.length {
            if <[u8; 32]>::from(self.hash.clone().finalize()) != self.expected.sha256 {
                self.changed = true;
                return Ok(TailProgress::SourceChanged);
            }
            self.prefix_verified = true;
        }
        if self.offset == self.current.length {
            return Ok(TailProgress::Verified(Fingerprint {
                identity: self.current,
                sha256: self.hash.clone().finalize().into(),
            }));
        }
        let end = if self.prefix_verified {
            self.current.length
        } else {
            self.expected.identity.length
        };
        let count = (end - self.offset).min(64 * 1024) as usize;
        let mut bytes = [0u8; 64 * 1024];
        self.file.read_exact(&mut bytes[..count])?;
        self.hash.update(&bytes[..count]);
        self.offset += count as u64;
        Ok(TailProgress::Pending {
            verified: self.offset,
            total: self.current.length,
        })
    }
}
/// Follow-scroll is separate from ingestion. Unlock is an explicit fixed-generation transition.
#[derive(Default)]
pub struct FollowState {
    paused_scroll: bool,
    unlocked: bool,
}
impl FollowState {
    pub fn set_scrolled_away(&mut self, away: bool) {
        self.paused_scroll = away;
    }
    pub fn auto_scroll(&self) -> bool {
        !self.paused_scroll && !self.unlocked
    }
    pub fn following(&self) -> bool {
        !self.unlocked
    }
    pub fn unlock(&mut self, confirmed: bool) -> bool {
        if confirmed {
            self.unlocked = true;
        }
        self.unlocked
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Platform;
    impl LocalFileSystem for Platform {
        fn open_follow_read(&self, path: &Path) -> std::io::Result<(File, Arc<dyn Send + Sync>)> {
            Ok((File::open(path)?, Arc::new(())))
        }
        fn available_space(&self, _: &Path) -> std::io::Result<u64> {
            Ok(u64::MAX)
        }
        fn guard_directory(&self, _: &Path) -> std::io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn open_sealed_read(&self, path: &Path) -> std::io::Result<File> {
            File::open(path)
        }
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn identity(&self, f: &File) -> std::io::Result<FileIdentity> {
            let m = f.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: m.len(),
                modified: m.modified()?.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
            })
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> std::io::Result<()> {
            unreachable!()
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "bareline-tail-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&p, b"partial").unwrap();
            Self(p)
        }
        fn fingerprint(&self) -> Fingerprint {
            Fingerprint {
                identity: Platform.identity(&File::open(&self.0).unwrap()).unwrap(),
                sha256: Sha256::digest(b"partial").into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn finish(v: &mut TailVerifier) -> TailProgress {
        for _ in 0..20 {
            let p = v.step().unwrap();
            if !matches!(p, TailProgress::Pending { .. }) {
                return p;
            }
        }
        panic!("did not complete")
    }
    #[test]
    fn segmented_follow_completes_split_scalar_and_unlocks_fixed_generation() {
        use crate::lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded};
        use bareline_document::paged::WindowPoll;
        let fixture = Fixture::new();
        std::fs::write(&fixture.0, b"prefix \xe2").unwrap();
        let cache = fixture.0.with_extension("cache");
        std::fs::create_dir(&cache).unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let TranscodeOutcome::Complete(mut opened) = open_paged_encoded(
            PagedOpenRequest {
                path: fixture.0.clone(),
                bytes: budget.clone(),
                history: Budget::new(1024),
                cache: cache.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 1024 * 1024,
                    interpret: Some(crate::codecs::Encoding::Utf8),
                },
                source_options: SourceOptions {
                    resident_max_bytes: 0,
                    ..SourceOptions::default()
                },
            },
            Arc::new(Platform),
            Cancellation::default(),
            |_| {},
        ) else {
            panic!("open failed")
        };
        let mut tail = TailSession::new(&opened, Arc::new(Platform), budget.clone(), Cancellation::default()).unwrap();
        let before = opened.transcoded.document.snapshot();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&fixture.0)
            .unwrap()
            .write_all(b"\x82\xac\n")
            .unwrap();
        tail.request(&fixture.0).unwrap();
        for _ in 0..100 {
            tail.step(&mut opened).unwrap();
            if !tail.pending() {
                break;
            }
        }
        assert!(!tail.source_changed);
        assert!(!tail.pending());
        let snapshot = opened.transcoded.document.snapshot();
        assert!(snapshot.same_document(&before));
        assert!(snapshot.revision.0 > before.revision.0);
        let mut read = snapshot
            .begin_read(TextOffset(0)..TextOffset(snapshot.len()), 100, &budget)
            .unwrap();
        loop {
            match read.poll() {
                WindowPoll::Pending(ticket) => {
                    if !tail.read_page(ticket).unwrap() {
                        opened.transcoded.source.read_page(ticket).unwrap();
                    }
                }
                WindowPoll::Ready(window) => {
                    assert_eq!(window.text(), "prefix €\n");
                    break;
                }
                _ => panic!("unavailable tail"),
            }
        }
        let fixed = tail.freeze(&opened).unwrap();
        assert_eq!(fixed.fingerprint, opened.fingerprint);
        drop(fixed);
        drop(tail);
        drop(opened);
        drop(before);
        drop(snapshot);
        drop(read);
        std::fs::remove_dir_all(cache).unwrap();
    }
    #[test]
    fn partial_page_append_verified_and_new_generation_created() {
        let f = Fixture::new();
        let mut baseline = TailVerifier::baseline(
            &f.0,
            f.fingerprint().identity,
            Arc::new(Platform),
            Cancellation::default(),
        )
        .unwrap();
        let TailProgress::Verified(old) = finish(&mut baseline) else {
            panic!("baseline not verified")
        };
        use bareline_document::source::SourceRead;
        let options = crate::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 8,
            page_cache_bytes: 16,
        };
        let mut before = crate::source::FileSource::open(
            &f.0,
            Arc::new(Platform),
            options,
            bareline_document::Budget::new(32),
            Cancellation::default(),
        )
        .unwrap();
        let old_source = before.source();
        let SourceRead::Pending(ticket) = old_source.read(0..7) else {
            panic!()
        };
        before.read_page(ticket).unwrap();
        let SourceRead::Ready(old_ready) = old_source.read(0..7) else {
            panic!()
        };
        let mut writer = std::fs::OpenOptions::new().append(true).open(&f.0).unwrap();
        writer.write_all(b" next").unwrap();
        drop(writer);
        let mut v = TailVerifier::begin(&f.0, old, Arc::new(Platform), Cancellation::default()).unwrap();
        let TailProgress::Verified(new) = finish(&mut v) else {
            panic!("append rejected")
        };
        assert_eq!(new.identity.length, 12);
        let mut p = crate::source::FileSource::open_verified_generation(
            &f.0,
            Arc::new(Platform),
            options,
            bareline_document::Budget::new(2 * 1024 * 1024),
            Cancellation::default(),
            &new,
        )
        .unwrap();
        assert_eq!(p.kind(), bareline_document::source::SourceKind::Paged);
        let source = p.source();
        let SourceRead::Pending(ticket) = source.read(0..8) else {
            panic!()
        };
        p.read_page(ticket).unwrap();
        let SourceRead::Ready(new_ready) = source.read(0..8) else {
            panic!()
        };
        assert_eq!(new_ready.bytes(), b"partial ");
        assert_eq!(old_ready.bytes(), b"partial");
        assert_ne!(old_ready.generation, new_ready.generation);
    }
    #[test]
    fn rewrite_and_truncate_never_accepted() {
        for replacement in [b"changed".as_slice(), b"x".as_slice()] {
            let f = Fixture::new();
            let old = f.fingerprint();
            std::fs::write(&f.0, replacement).unwrap();
            let mut v = TailVerifier::begin(&f.0, old, Arc::new(Platform), Cancellation::default()).unwrap();
            assert!(matches!(finish(&mut v), TailProgress::SourceChanged));
        }
    }
    #[test]
    fn cancellation_and_scroll_pause_are_independent() {
        let f = Fixture::new();
        let cancel = Cancellation::default();
        let mut v = TailVerifier::begin(&f.0, f.fingerprint(), Arc::new(Platform), cancel.clone()).unwrap();
        cancel.cancel();
        assert!(matches!(v.step(), Err(FileError::Cancelled)));
        let mut state = FollowState::default();
        state.set_scrolled_away(true);
        assert!(state.following());
        assert!(!state.auto_scroll());
        assert!(!state.unlock(false));
        assert!(state.unlock(true));
        assert!(!state.following());
    }
}
