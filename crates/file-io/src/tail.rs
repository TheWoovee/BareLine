// SPDX-License-Identifier: MPL-2.0
//! Incremental conservative tail continuity. Growth is an append to the pinned
//! generation: the live bytes before it are compared with the sealed copy's last page
//! and the retained running SHA-256 is extended with the appended bytes only. A session
//! without that running state rehashes the complete prefix once, in bounded steps.
use crate::{
    cancellation::Cancellation,
    lifecycle::{FileError, Fingerprint},
};
use crate::{
    codecs::disk::{DiskDecoded, DiskOptions, DiskTranscoder, pinned_or_appended},
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
/// Bytes at the end of the followed generation compared with the live file before
/// any appended byte is accepted.
const CHECK_PAGE: usize = 64 * 1024;
/// Suffix segments are merged so their count, store directories and open handles
/// stay bounded however long a session follows.
const MAX_SEGMENTS: usize = 8;
fn follow_identity(platform: &dyn LocalFileSystem, path: &Path) -> Result<FileIdentity, FileError> {
    let (file, _guard) = platform.open_follow_read(path)?;
    Ok(platform.identity(&file)?)
}
fn push_page(page: &mut Vec<u8>, bytes: &[u8]) {
    page.extend_from_slice(bytes);
    let excess = page.len().saturating_sub(CHECK_PAGE);
    page.drain(..excess);
}

/// Follow owns immutable suffix stores. Only the decoder's final opaque unit is copied
/// from the sealed predecessor into the next suffix; trailing suffixes are periodically
/// re-read, re-verified against the running hash and merged into one.
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
    /// Running file hash at `fingerprint.identity.length`; None until a full verification.
    hash: Option<Sha256>,
    /// Sealed copy of the followed generation's last bytes, at most `CHECK_PAGE`.
    page: Vec<u8>,
    segments: Vec<Segment>,
    phase: Option<TailPhase>,
    /// A request made while a check was in flight. Growth it signals may have come
    /// after that check pinned its target, so another check follows (QA-08).
    requested: bool,
    pub source_changed: bool,
}
/// One decoded suffix. Its bytes start at `raw_base` in the file and `text_base` in the
/// document; `start` is the followed length before its new bytes and `start_hash` the
/// running file hash there, so a later merge can re-read and verify it.
struct Segment {
    source: FileSource,
    store: DiskDecoded,
    raw_base: u64,
    text_base: u64,
    start: u64,
    start_hash: Sha256,
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
    /// Fallback without a running hash: one conservative full-prefix rehash.
    Verify(TailVerifier),
    /// Compare the live last page with the sealed copy, then plan the append.
    Check,
    Copy(CopyJob),
    Decode(DecodeJob),
}
/// The planned suffix: segments from `merge` on are replaced by one new segment.
struct Append {
    merge: usize,
    raw_base: u64,
    text_base: u64,
    start: u64,
    start_hash: Sha256,
    target: FileIdentity,
    page: Vec<u8>,
}
struct CopyJob {
    file: File,
    output: File,
    scratch: Scratch,
    offset: u64,
    hash: Sha256,
    append: Append,
    _guard: Arc<dyn Send + Sync>,
}
struct DecodeJob {
    job: DiskTranscoder,
    _scratch: Scratch,
    verified: Fingerprint,
    hash: Sha256,
    append: Append,
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
/// Left on a merged segment's source. Snapshots captured before the merge still hold
/// that source and resolve its pages from the sealed store as owned pages; the store
/// is released with the last of them, and its text is opened only per page read.
struct RetiredSegment(DiskDecoded);
impl bareline_document::source::OwnedPageLoader for RetiredSegment {
    fn read(&self, offset: u64, output: &mut [u8]) -> std::io::Result<()> {
        let mut text = self.0.platform().open_sealed_read(&self.0.text_path())?;
        text.seek(SeekFrom::Start(offset))?;
        text.read_exact(output)
    }
}
impl TailSession {
    pub fn new(
        opened: &PagedOpened,
        platform: Arc<dyn LocalFileSystem>,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        let store = &opened.transcoded.store;
        let (raw_start, text_start) = store.tail_boundary().map_err(FileError::Transcode)?;
        let cache = store
            .text_path()
            .parent()
            .and_then(Path::parent)
            .ok_or(FileError::IncompleteSource)?
            .to_owned();
        let cache_guard = platform.guard_directory(&cache)?;
        // The running hash is reusable only when the sealed original is exactly the
        // followed generation; otherwise the first request verifies the full prefix.
        let hash = store
            .raw_hash_state()
            .filter(|_| store.fingerprint == opened.fingerprint && store.raw_len == opened.fingerprint.identity.length);
        let mut page = Vec::new();
        if hash.is_some() {
            let count = store.raw_len.min(CHECK_PAGE as u64);
            let mut original = File::open(store.original_path())?;
            original.seek(SeekFrom::Start(store.raw_len - count))?;
            page.resize(count as usize, 0);
            original.read_exact(&mut page)?;
        }
        Ok(Self {
            _cache_guard: cache_guard,
            last_append: None,
            platform,
            budget,
            cancellation,
            cache,
            encoding: store.state.interpreted(),
            fingerprint: opened.fingerprint.clone(),
            raw_start,
            text_start,
            hash,
            page,
            segments: Vec::new(),
            phase: None,
            requested: false,
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
            .map_or(self.fingerprint.identity.length, |segment| segment.raw_base);
        write_part(&opened.transcoded.store, first_end)?;
        for (index, segment) in self.segments.iter().enumerate() {
            let end = self
                .segments
                .get(index + 1)
                .map_or(self.fingerprint.identity.length, |next| next.raw_base);
            write_part(
                &segment.store,
                end.checked_sub(segment.raw_base).ok_or(FileError::IncompleteSource)?,
            )?;
        }
        Ok(())
    }
    /// Start one continuity check. Growth never latches `source_changed`; only a
    /// replaced, shortened or rewritten followed prefix does. A request during a
    /// check is kept: the next check starts when that one ends, so an append made
    /// while earlier bytes are verified, copied or converted is never left behind.
    pub fn request(&mut self, path: &Path) -> Result<(), FileError> {
        if self.phase.is_some() {
            self.requested = true;
        } else if !self.source_changed {
            self.phase = Some(if self.hash.is_some() {
                TailPhase::Check
            } else {
                TailPhase::Verify(TailVerifier::begin(
                    path,
                    self.fingerprint.clone(),
                    self.platform.clone(),
                    self.cancellation.clone(),
                )?)
            });
        }
        Ok(())
    }
    /// One bounded read/copy/decode step. A true result publishes a logical revision.
    pub fn step(&mut self, opened: &mut PagedOpened) -> Result<bool, FileError> {
        self.cancellation.check()?;
        let Some(phase) = self.phase.take() else {
            return Ok(false);
        };
        let published = match phase {
            TailPhase::Verify(mut verifier) => {
                match (verifier.step()?, verifier.prefix_hash.take()) {
                    (TailProgress::SourceChanged, _) => self.source_changed = true,
                    (_, Some(hash)) => {
                        self.hash = Some(hash);
                        // The verified prefix's own last bytes, so later checks compare a full page.
                        self.page = std::mem::take(&mut verifier.page);
                        self.begin_append(opened)?;
                    }
                    (TailProgress::Pending { .. }, None) => self.phase = Some(TailPhase::Verify(verifier)),
                    (TailProgress::Verified(_), None) => return Err(FileError::IncompleteSource),
                }
                false
            }
            TailPhase::Check => {
                self.begin_append(opened)?;
                false
            }
            TailPhase::Copy(copy) => {
                self.copy_step(copy, &opened.path)?;
                false
            }
            TailPhase::Decode(decode) => self.decode_step(decode, opened)?,
        };
        if self.phase.is_none() && std::mem::take(&mut self.requested) {
            self.request(&opened.path)?;
        }
        Ok(published)
    }
    /// Bounded: one identity query and one page read. Plans the copy of the appended
    /// bytes, re-reading trailing segments that are merged into the new suffix.
    fn begin_append(&mut self, opened: &PagedOpened) -> Result<(), FileError> {
        let hash = self.hash.clone().ok_or(FileError::IncompleteSource)?;
        let old = self.fingerprint.identity;
        let (mut file, guard) = self.platform.open_follow_read(&opened.path)?;
        let live = self.platform.identity(&file)?;
        if live == old {
            return Ok(());
        }
        if live.volume != old.volume || live.file != old.file || live.length < old.length {
            self.source_changed = true;
            return Ok(());
        }
        let mut bytes = vec![0; self.page.len()];
        file.seek(SeekFrom::Start(old.length - bytes.len() as u64))?;
        match file.read_exact(&mut bytes) {
            // Shortened after the identity query: the next request observes it.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            result => result?,
        }
        if bytes != self.page {
            self.source_changed = true;
            return Ok(());
        }
        if live.length == old.length {
            return Ok(());
        }
        // Merge trailing segments no larger than the bytes joining them: sizes grow
        // geometrically, so re-reads are amortized and the count stays logarithmic.
        let mut merge = self.segments.len();
        let mut joined = live.length - old.length;
        while merge > 0 {
            let end = self.segments.get(merge).map_or(old.length, |next| next.start);
            let size = end - self.segments[merge - 1].start;
            if merge < MAX_SEGMENTS && size > joined {
                break;
            }
            joined += size;
            merge -= 1;
        }
        let (raw_base, text_base, start, start_hash) = match self.segments.get(merge) {
            Some(segment) => (
                segment.raw_base,
                segment.text_base,
                segment.start,
                segment.start_hash.clone(),
            ),
            None => (self.raw_start, self.text_start, old.length, hash),
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = self.cache.join(format!(
            "bareline-tail-{}-{}.raw",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        let scratch = Scratch(path);
        // The decoder restarts at an opaque unit; its bytes come from the sealed
        // predecessor, which ends exactly where the new bytes start.
        let (carry, carry_base) = match merge.checked_sub(1) {
            Some(index) => (&self.segments[index].store, self.segments[index].raw_base),
            None => (&opened.transcoded.store, 0),
        };
        if raw_base < carry_base || start < raw_base || start - carry_base != carry.raw_len {
            return Err(FileError::IncompleteSource);
        }
        let carried = start - raw_base;
        let mut sealed = File::open(carry.original_path())?;
        sealed.seek(SeekFrom::Start(raw_base - carry_base))?;
        if std::io::copy(&mut sealed.take(carried), &mut output)? != carried {
            return Err(FileError::IncompleteSource);
        }
        file.seek(SeekFrom::Start(start))?;
        self.phase = Some(TailPhase::Copy(CopyJob {
            file,
            output,
            scratch,
            offset: start,
            hash: start_hash.clone(),
            append: Append {
                merge,
                raw_base,
                text_base,
                start,
                start_hash,
                target: live,
                page: self.page.clone(),
            },
            _guard: guard,
        }));
        Ok(())
    }
    fn copy_step(&mut self, mut copy: CopyJob, path: &Path) -> Result<(), FileError> {
        let old = self.fingerprint.identity;
        let target = copy.append.target;
        // Growth while copying is a later append. Anything else drops this attempt;
        // the next request re-plans it or observes the change.
        if !pinned_or_appended(&target, &self.platform.identity(&copy.file)?) {
            return Ok(());
        }
        let end = if copy.offset < old.length {
            old.length
        } else {
            target.length
        };
        let mut bytes = [0; 65536];
        let count = (end - copy.offset).min(bytes.len() as u64) as usize;
        match copy.file.read_exact(&mut bytes[..count]) {
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            result => result?,
        }
        copy.output.write_all(&bytes[..count])?;
        copy.hash.update(&bytes[..count]);
        if copy.offset >= old.length {
            push_page(&mut copy.append.page, &bytes[..count]);
        }
        copy.offset += count as u64;
        // A merge re-reads already accepted bytes; they must still hash to the
        // followed generation before any appended byte is accepted.
        if copy.offset == old.length
            && copy.append.start < old.length
            && <[u8; 32]>::from(copy.hash.clone().finalize()) != self.fingerprint.sha256
        {
            self.source_changed = true;
            return Ok(());
        }
        if copy.offset < target.length {
            self.phase = Some(TailPhase::Copy(copy));
            return Ok(());
        }
        copy.output.sync_all()?;
        drop(copy.output);
        if !pinned_or_appended(&target, &self.platform.identity(&copy.file)?)
            || !pinned_or_appended(&target, &follow_identity(self.platform.as_ref(), path)?)
        {
            return Ok(());
        }
        let verified = Fingerprint {
            identity: target,
            sha256: copy.hash.clone().finalize().into(),
        };
        let job = DiskTranscoder::continuation(
            FileInput {
                file: File::open(&copy.scratch.0)?,
                path: copy.scratch.0.clone(),
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
        self.phase = Some(TailPhase::Decode(DecodeJob {
            job,
            _scratch: copy.scratch,
            verified,
            hash: copy.hash,
            append: copy.append,
        }));
        Ok(())
    }
    fn decode_step(&mut self, mut decode: DecodeJob, opened: &mut PagedOpened) -> Result<bool, FileError> {
        if !decode.job.step().map_err(FileError::Transcode)?.complete {
            self.phase = Some(TailPhase::Decode(decode));
            return Ok(false);
        }
        let DecodeJob {
            job,
            _scratch: scratch,
            verified,
            hash,
            append,
        } = decode;
        let store = job.finish().map_err(FileError::Transcode)?;
        drop(scratch);
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
        // Merged text before the previous decoder restart point is re-decoded from the
        // same bytes; only what follows it is reported as changed.
        let retained = self
            .text_start
            .checked_sub(append.text_base)
            .ok_or(FileError::IncompleteSource)?;
        opened
            .transcoded
            .document
            .replace_tail_source_retaining(
                TextOffset(usize::try_from(append.text_base).map_err(|_| FileError::Budget)?),
                usize::try_from(retained).map_err(|_| FileError::Budget)?,
                source.source(),
            )
            .map_err(|_| FileError::Budget)?;
        self.raw_start = append.raw_base + raw_next;
        self.text_start = append.text_base + text_next;
        self.fingerprint = verified.clone();
        opened.fingerprint = verified;
        self.hash = Some(hash);
        self.page = append.page;
        let snapshot = opened.transcoded.document.snapshot();
        self.last_append = Some(AppendReceipt {
            generation: source.source().generation(),
            revision: snapshot.revision,
            raw_bytes: self.fingerprint.identity.length,
            text_bytes: snapshot.len(),
            applied_at: std::time::Instant::now(),
        });
        // The new root no longer references merged segments, but captured snapshots may.
        // Their sources keep a loader over the sealed store; the segment's handles are
        // released now and its store directory with the last snapshot holding it.
        for merged in self.segments.drain(append.merge..) {
            let retired = merged.source.source();
            if !retired.has_owned_loader() {
                let _ = retired.attach_owned_loader(Arc::new(RetiredSegment(merged.store)));
            }
        }
        self.segments.push(Segment {
            source,
            store,
            raw_base: append.raw_base,
            text_base: append.text_base,
            start: append.start,
            start_hash: append.start_hash,
        });
        Ok(true)
    }
    /// Returns false for the original source, which remains owned by PagedOpened, and
    /// for merged segments, which captured snapshots resolve as owned pages.
    pub fn read_page(&mut self, ticket: PageTicket) -> Result<bool, FileError> {
        if let Some(segment) = self
            .segments
            .iter_mut()
            .find(|segment| segment.source.source().generation() == ticket.generation)
        {
            segment.source.read_page(ticket)?;
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
    /// Running hash at the expected length, captured once the prefix matched.
    prefix_hash: Option<Sha256>,
    /// Last bytes of the expected prefix as hashed, at most `CHECK_PAGE`.
    page: Vec<u8>,
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
    /// The generation is pinned at the length observed here; later growth is a later
    /// append and does not fail this verification.
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
            prefix_hash: None,
            page: Vec::new(),
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
        if !pinned_or_appended(&self.current, &self.platform.identity(&self.file)?)
            || !pinned_or_appended(&self.current, &follow_identity(self.platform.as_ref(), &self.path)?)
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
            self.prefix_hash = Some(self.hash.clone());
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
        if !self.prefix_verified {
            push_page(&mut self.page, &bytes[..count]);
        }
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
    fn open_followed(path: &Path, budget: &Budget) -> (Box<PagedOpened>, TailSession, PathBuf) {
        use crate::lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded};
        let cache = path.with_extension("cache");
        std::fs::create_dir(&cache).unwrap();
        let TranscodeOutcome::Complete(opened) = open_paged_encoded(
            PagedOpenRequest {
                path: path.to_owned(),
                bytes: budget.clone(),
                history: Budget::new(1024),
                cache: cache.clone(),
                options: DiskOptions {
                    temp_quota_bytes: 64 * 1024 * 1024,
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
        let tail = TailSession::new(&opened, Arc::new(Platform), budget.clone(), Cancellation::default()).unwrap();
        (opened, tail, cache)
    }
    fn append(path: &Path, bytes: &[u8]) {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    fn drive(tail: &mut TailSession, opened: &mut PagedOpened) {
        for _ in 0..1000 {
            tail.step(opened).unwrap();
            if !tail.pending() {
                return;
            }
        }
        panic!("tail did not settle")
    }
    fn document_text(tail: &mut TailSession, opened: &mut PagedOpened, budget: &Budget) -> String {
        let snapshot = opened.transcoded.document.snapshot();
        snapshot_text(tail, opened, &snapshot, budget)
    }
    /// Resolves pages in the editor's order: owned pages, then tail segments, then the original.
    fn snapshot_text(
        tail: &mut TailSession,
        opened: &mut PagedOpened,
        snapshot: &bareline_document::paged::PagedSnapshot,
        budget: &Budget,
    ) -> String {
        use bareline_document::paged::WindowPoll;
        let mut read = snapshot
            .begin_read(TextOffset(0)..TextOffset(snapshot.len()), snapshot.len(), budget)
            .unwrap();
        loop {
            match read.poll() {
                WindowPoll::Pending(ticket) => {
                    if !snapshot.resolve_owned(ticket).unwrap() && !tail.read_page(ticket).unwrap() {
                        opened.transcoded.source.read_page(ticket).unwrap();
                    }
                }
                WindowPoll::Ready(window) => return window.text().to_owned(),
                _ => panic!("unavailable tail"),
            }
        }
    }
    #[test]
    fn verification_pins_length_and_survives_growth_between_steps() {
        let f = Fixture::new();
        let prefix = vec![b'a'; 150 * 1024];
        std::fs::write(&f.0, &prefix).unwrap();
        let old = Fingerprint {
            identity: Platform.identity(&File::open(&f.0).unwrap()).unwrap(),
            sha256: Sha256::digest(&prefix).into(),
        };
        append(&f.0, b" next");
        let mut v = TailVerifier::begin(&f.0, old, Arc::new(Platform), Cancellation::default()).unwrap();
        assert!(matches!(v.step().unwrap(), TailProgress::Pending { .. }));
        append(&f.0, b" later");
        let TailProgress::Verified(new) = finish(&mut v) else {
            panic!("growth during verification latched a change")
        };
        let mut expected = prefix;
        expected.extend_from_slice(b" next");
        assert_eq!(new.identity.length, expected.len() as u64);
        assert_eq!(new.sha256, <[u8; 32]>::from(Sha256::digest(&expected)));
    }
    #[test]
    fn follow_survives_growth_while_copying_and_publishes_it_next() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"first\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        append(&f.0, b"second\n");
        tail.request(&f.0).unwrap();
        // Pins the grown length and plans the copy; the file grows again before it runs.
        tail.step(&mut opened).unwrap();
        assert!(tail.pending());
        append(&f.0, b"third\n");
        drive(&mut tail, &mut opened);
        assert!(!tail.source_changed);
        assert_eq!(document_text(&mut tail, &mut opened, &budget), "first\nsecond\n");
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(!tail.source_changed);
        assert_eq!(document_text(&mut tail, &mut opened, &budget), "first\nsecond\nthird\n");
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
    /// QA-08: an append signalled while an earlier append is still being copied and
    /// converted is published by the same drive, without waiting for another event.
    #[test]
    fn request_during_a_conversion_publishes_the_later_append_too() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"first\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        append(&f.0, b"second\n");
        tail.request(&f.0).unwrap();
        tail.step(&mut opened).unwrap();
        assert!(tail.pending());
        // The writer appends again while the pinned copy runs, and its file event
        // arrives now: the request is kept rather than ignored.
        append(&f.0, b"third\n");
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(!tail.source_changed);
        assert_eq!(document_text(&mut tail, &mut opened, &budget), "first\nsecond\nthird\n");
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
    #[test]
    fn many_appends_keep_segments_and_store_directories_bounded() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"start\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        let mut expected = String::from("start\n");
        for line in 0..100 {
            let text = format!("line {line}\n");
            append(&f.0, text.as_bytes());
            expected.push_str(&text);
            tail.request(&f.0).unwrap();
            drive(&mut tail, &mut opened);
            assert!(!tail.source_changed);
            assert!(tail.segments.len() <= MAX_SEGMENTS, "{} segments", tail.segments.len());
        }
        let stores = std::fs::read_dir(&cache)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("bareline-transcode-")
            })
            .count();
        assert!(stores <= MAX_SEGMENTS + 1, "{stores} store directories");
        assert_eq!(document_text(&mut tail, &mut opened, &budget), expected);
        assert_eq!(
            opened.fingerprint.sha256,
            <[u8; 32]>::from(Sha256::digest(expected.as_bytes()))
        );
        let fixed = tail.freeze(&opened).unwrap();
        assert_eq!(fixed.fingerprint, opened.fingerprint);
        drop(fixed);
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
    #[test]
    fn rewritten_last_page_is_a_change_not_an_append() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"hello world\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        std::fs::write(&f.0, b"HELLO world\nmore\n").unwrap();
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(tail.source_changed);
        assert_eq!(document_text(&mut tail, &mut opened, &budget), "hello world\n");
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
    fn store_directories(cache: &Path) -> usize {
        std::fs::read_dir(cache)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("bareline-transcode-")
            })
            .count()
    }
    #[test]
    fn captured_snapshot_reads_merged_segments_and_merge_reports_only_appended_text() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"start\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        append(&f.0, b"line 0\n");
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert_eq!(tail.segments.len(), 1);
        // Captured before the merge and never read, so none of its tail pages are cached.
        let captured = opened.transcoded.document.snapshot();
        let restart = usize::try_from(tail.text_start).unwrap();
        assert!(restart > "start\n".len());
        // Larger than the first suffix: that suffix is merged into the new one.
        append(&f.0, b"merged tail line\n");
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(!tail.source_changed);
        assert_eq!(tail.segments.len(), 1);
        assert_eq!(tail.segments[0].start, "start\n".len() as u64);
        let current = opened.transcoded.document.snapshot();
        let change = current.applied_change().unwrap();
        assert_eq!(change.edits().len(), 1);
        assert_eq!(
            change.edits()[0].before,
            TextOffset(restart)..TextOffset(captured.len())
        );
        assert_eq!(change.edits()[0].inserted_len, current.len() - restart);
        drop(current);
        assert_eq!(
            snapshot_text(&mut tail, &mut opened, &captured, &budget),
            "start\nline 0\n"
        );
        assert!(!tail.source_changed);
        assert_eq!(
            document_text(&mut tail, &mut opened, &budget),
            "start\nline 0\nmerged tail line\n"
        );
        // The merged store outlives the merge only while a captured snapshot holds it.
        assert_eq!(store_directories(&cache), 3);
        drop(captured);
        assert_eq!(store_directories(&cache), 2);
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
    #[test]
    fn full_prefix_verification_fills_the_check_page() {
        let f = Fixture::new();
        std::fs::write(&f.0, b"hello world\n").unwrap();
        let budget = Budget::new(16 * 1024 * 1024);
        let (mut opened, mut tail, cache) = open_followed(&f.0, &budget);
        // As for a session opened from retained metadata: no running hash or sealed page.
        tail.hash = None;
        tail.page.clear();
        append(&f.0, b"a\n");
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(!tail.source_changed);
        assert_eq!(tail.page, b"hello world\na\n");
        // Rewritten in place and longer: the appended bytes alone still match.
        std::fs::write(&f.0, b"HELLO world\na\nmore\n").unwrap();
        tail.request(&f.0).unwrap();
        drive(&mut tail, &mut opened);
        assert!(tail.source_changed);
        assert_eq!(document_text(&mut tail, &mut opened, &budget), "hello world\na\n");
        drop(tail);
        drop(opened);
        std::fs::remove_dir_all(cache).unwrap();
    }
}
