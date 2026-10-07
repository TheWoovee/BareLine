// SPDX-License-Identifier: MPL-2.0
//! Prepare a private paged copy of a Resident snapshot on the existing I/O pool.
//! No mutation/publication occurs here; the actor validates the captured identity later.
use crate::{
    cancellation::Cancellation,
    codecs::{
        Encoding,
        disk::{DiskOptions, DiskTranscoder, PagedTranscoded},
        resident::ResidentEncoding,
    },
    lifecycle::{FileError, FileInput, InterpretPagedRequest},
    source::SourceOptions,
};
use bareline_document::{Budget, DocumentSnapshot, TextOffset};
use bareline_platform::LocalFileSystem;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("input.raw"));
        crate::owned_cache::release_empty(&self.0);
    }
}
/// Free space is sampled again after this many bytes are written (FIO-08).
const SPACE_RECHECK_BYTES: u64 = 16 * 1024 * 1024;
/// Private spill writers buffer this much before a write reaches the file (FIO-08).
pub(crate) const WRITE_BUFFER: usize = 1024 * 1024;
/// Free-space admission for private spill and transcode writers (FIO-08). The
/// rule is unchanged, at most min(quota, (free + written) / 5) bytes in total,
/// but free space is queried at the start and then once per
/// `SPACE_RECHECK_BYTES` written, not before every write. Between samples this
/// writer's own growth is what consumes the sampled space. A write the last
/// sample would refuse samples again, so a refusal always reflects the volume.
#[derive(Default)]
pub(crate) struct SpaceGate {
    /// Free space plus bytes written, and bytes written, at the last sample.
    sampled: Option<(u64, u64)>,
    #[cfg_attr(not(test), allow(dead_code))]
    samples: u64,
}
impl SpaceGate {
    /// The effective total quota for a writer at `written` that adds `additional`.
    pub(crate) fn limit(
        &mut self,
        platform: &dyn LocalFileSystem,
        directory: &Path,
        quota: u64,
        written: u64,
        additional: u64,
    ) -> io::Result<u64> {
        let limit = |total: u64| quota.min(total / 5);
        if let Some((total, at)) = self.sampled
            && written.saturating_sub(at) < SPACE_RECHECK_BYTES
            && written.saturating_add(additional) <= limit(total)
        {
            return Ok(limit(total));
        }
        let total = platform.available_space(directory)?.saturating_add(written);
        self.sampled = Some((total, written));
        self.samples += 1;
        Ok(limit(total))
    }
    /// Refuse up front when `expected` bytes cannot fit (FIO-08).
    pub(crate) fn admit(
        &mut self,
        platform: &dyn LocalFileSystem,
        directory: &Path,
        quota: u64,
        expected: u64,
    ) -> io::Result<()> {
        if expected > self.limit(platform, directory, quota, 0, expected)? {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "owned spill quota exceeded; Resident data retained",
            ));
        }
        Ok(())
    }
    /// Free space now for a writer at `written` that needs `needed` of it: the last
    /// sample less what the writer wrote since. Sampled like `limit`, so a writer
    /// can keep a per-write rule (each write within a share of what is free) without
    /// querying the volume before every write.
    pub(crate) fn free(
        &mut self,
        platform: &dyn LocalFileSystem,
        directory: &Path,
        written: u64,
        needed: u64,
    ) -> io::Result<u64> {
        if let Some((total, at)) = self.sampled
            && written.saturating_sub(at) < SPACE_RECHECK_BYTES
            && total.saturating_sub(written) >= needed
        {
            return Ok(total.saturating_sub(written));
        }
        let free = platform.available_space(directory)?;
        self.sampled = Some((free.saturating_add(written), written));
        self.samples += 1;
        Ok(free)
    }
    /// Free-space queries so far.
    #[cfg(test)]
    pub(crate) fn samples(&self) -> u64 {
        self.samples
    }
}
struct QuotaWriter<'a> {
    output: io::BufWriter<std::fs::File>,
    directory: &'a Path,
    platform: &'a dyn LocalFileSystem,
    cancel: &'a Cancellation,
    quota: u64,
    written: u64,
    space: SpaceGate,
}
impl QuotaWriter<'_> {
    /// Flush the buffered bytes and make them durable.
    fn sync(&mut self) -> io::Result<()> {
        self.output.flush()?;
        self.output.get_ref().sync_all()
    }
}
impl Write for QuotaWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancel
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "spill cancelled"))?;
        let effective = self.space.limit(
            self.platform,
            self.directory,
            self.quota,
            self.written,
            bytes.len() as u64,
        )?;
        if bytes.len() as u64 > effective.saturating_sub(self.written) {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "owned spill quota exceeded; Resident data retained",
            ));
        }
        let count = self.output.write(bytes)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}
/// The original-byte/provenance store is rebuilt through the existing codec pipeline,
/// preserving unchanged legacy aliases and invalid spans. Failure leaves the actor intact.
pub fn prepare_resident(
    snapshot: &DocumentSnapshot,
    encoding: Option<&ResidentEncoding>,
    bom: bool,
    cache: &Path,
    quota: u64,
    platform: Arc<dyn LocalFileSystem>,
    options: SourceOptions,
    bytes: Budget,
    history: Budget,
    cancellation: Cancellation,
) -> Result<PagedTranscoded, FileError> {
    if !snapshot.is_complete() {
        return Err(FileError::IncompleteSource);
    }
    cancellation.check()?;
    bareline_platform::private::create_dir_all(cache)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = cache.join(format!(
        "resident-spill-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    bareline_platform::private::create_dir(&directory)?;
    if let Err(error) = crate::owned_cache::publish_ownership(
        &directory,
        crate::owned_cache::CacheKind::ResidentSpill,
        platform.as_ref(),
    ) {
        crate::owned_cache::release_empty(&directory);
        return Err(error.into());
    }
    let staging = Staging(directory);
    let path = staging.0.join("input.raw");
    let output = bareline_platform::private::file_options()
        .write(true)
        .create_new(true)
        .open(&path)?;
    // Reserve space for both staged raw input and the durable raw/text/map triplet.
    let mut writer = QuotaWriter {
        output: io::BufWriter::with_capacity(WRITE_BUFFER, output),
        directory: &staging.0,
        platform: platform.as_ref(),
        cancel: &cancellation,
        quota: quota / 2,
        written: 0,
        space: SpaceGate::default(),
    };
    if let Some(encoding) = encoding {
        encoding
            .write_snapshot(snapshot, encoding.state.save_target, encoding.state.bom, &mut writer)
            .map_err(FileError::Encoding)?;
    } else {
        let expected = snapshot.len() as u64 + if bom { 3 } else { 0 };
        writer.space.admit(platform.as_ref(), &staging.0, quota / 2, expected)?;
        if bom {
            writer.write_all(&[0xef, 0xbb, 0xbf])?;
        }
        for chunk in snapshot
            .chunks(TextOffset(0)..TextOffset(snapshot.len()))
            .map_err(|_| FileError::IncompleteSource)?
        {
            writer.write_all(chunk.as_bytes())?;
        }
    }
    writer.sync()?;
    let used = writer.written;
    drop(writer);
    let input = FileInput {
        path: path.clone(),
        file: std::fs::File::open(&path)?,
    };
    let options_disk = DiskOptions {
        temp_quota_bytes: quota.saturating_sub(used),
        interpret: Some(encoding.map_or(Encoding::Utf8, |encoding| encoding.state.save_target)),
    };
    let mut job = if encoding.map_or(bom, |encoding| encoding.state.bom) {
        DiskTranscoder::new(
            input,
            platform.clone(),
            cache,
            options_disk,
            bytes.clone(),
            cancellation.clone(),
        )
    } else {
        DiskTranscoder::continuation(
            input,
            platform.clone(),
            cache,
            options_disk,
            bytes.clone(),
            cancellation.clone(),
        )
    }
    .map_err(FileError::Transcode)?;
    loop {
        if job.step().map_err(FileError::Transcode)?.complete {
            break;
        }
    }
    let store = job.finish().map_err(FileError::Transcode)?;
    let result = store
        .open_paged(platform, options, bytes, history, cancellation)
        .map_err(FileError::Transcode)?;
    // Snapshots keep the store alive even after the producer/controller is dropped.
    result
        .source
        .source()
        .retain_owner(Arc::new(store))
        .map_err(|_| FileError::Budget)?;
    Ok(result)
}

/// Sealed immutable backing for captured current and history segments. The caller
/// supplies Resident original provenance only after preparing the original codec store.
pub fn prepare_segments(
    plan: bareline_document::spill::SpillPlan,
    original: Option<(&ResidentEncoding, &bareline_document::source::MemorySource)>,
    cache: &Path,
    quota: u64,
    platform: Arc<dyn LocalFileSystem>,
    options: SourceOptions,
    budget: Budget,
    cancellation: &Cancellation,
) -> Result<bareline_document::spill::PreparedSpill, FileError> {
    use bareline_document::{
        source::{Generation, MemorySource, SourceKind},
        spill::StoredSegment,
    };
    cancellation.check()?;
    bareline_platform::private::create_dir_all(cache)?;
    let parent_guard = platform.guard_directory(cache)?;
    static NEXT_OWNED: AtomicU64 = AtomicU64::new(1);
    let serial = NEXT_OWNED.fetch_add(1, Ordering::Relaxed);
    let directory = cache.join(format!("owned-segments-{}-{serial}", std::process::id()));
    bareline_platform::private::create_dir(&directory)?;
    if let Err(error) = crate::owned_cache::publish_ownership(
        &directory,
        crate::owned_cache::CacheKind::OwnedSegments,
        platform.as_ref(),
    ) {
        crate::owned_cache::release_empty(&directory);
        return Err(error.into());
    }
    let cleanup = OwnedDirectory(directory);
    let directory_guard = platform.guard_directory(&cleanup.0)?;
    let path = cleanup.0.join("segments.utf8");
    let output = bareline_platform::private::file_options()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut writer = QuotaWriter {
        output: io::BufWriter::with_capacity(WRITE_BUFFER, output),
        directory: &cleanup.0,
        platform: platform.as_ref(),
        cancel: cancellation,
        quota,
        written: 0,
        space: SpaceGate::default(),
    };
    let count = plan.segments().count();
    let expected = plan.segments().map(|segment| segment.text.len() as u64).sum();
    writer.space.admit(platform.as_ref(), &cleanup.0, quota, expected)?;
    let _mapping_charge = budget
        .claim(
            count
                .checked_mul(std::mem::size_of::<StoredSegment>())
                .ok_or(FileError::Budget)?,
        )
        .map_err(|_| FileError::Budget)?;
    let mut stored = Vec::new();
    stored.try_reserve_exact(count).map_err(|_| FileError::Budget)?;
    // One address index for every segment, not a baseline scan per segment (FIO-15).
    let originals = original.and_then(|(encoding, source)| Some((encoding.original_chunks()?, source)));
    for segment in plan.segments() {
        let start = writer.written;
        for chunk in segment.text.as_bytes().chunks(64 * 1024) {
            writer.write_all(chunk)?;
        }
        let provenance = originals
            .as_ref()
            .and_then(|(chunks, source)| chunks.range_of(segment.text).map(|range| ((*source).clone(), range)));
        stored.push(StoredSegment {
            id: segment.id,
            range: start..writer.written,
            original: provenance,
        });
    }
    writer.sync()?;
    let length = writer.written;
    drop(writer);
    let sealed = platform.open_sealed_read(&path)?;
    // FileSource generations occupy the low namespace; immutable owned stores use
    // the upper half and retain their capability independently of request cancellation.
    let (source, _) = MemorySource::new(
        length,
        Generation((1u64 << 63) | serial),
        SourceKind::Paged,
        options.page_size_bytes,
        options.page_cache_bytes,
        budget,
    )
    .map_err(|_| FileError::Budget)?;
    source
        .attach_owned_loader(Arc::new(SealedSegments {
            file: std::sync::Mutex::new(sealed),
            _directory_guard: directory_guard,
            _parent_guard: parent_guard,
            _cleanup: cleanup,
        }))
        .map_err(|_| FileError::Budget)?;
    cancellation.check()?;
    plan.prepare(source, stored).map_err(|_| FileError::IncompleteSource)
}
struct OwnedDirectory(PathBuf);
impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("segments.utf8"));
        crate::owned_cache::release_empty(&self.0);
    }
}
struct SealedSegments {
    file: std::sync::Mutex<std::fs::File>,
    _directory_guard: Arc<dyn Send + Sync>,
    _parent_guard: Arc<dyn Send + Sync>,
    _cleanup: OwnedDirectory,
}
impl bareline_document::source::OwnedPageLoader for SealedSegments {
    fn read(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("owned store lock poisoned"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(output)
    }
}

/// Rebuild only the immutable opening baseline, never the current edited text.
/// This supplies raw provenance when a Resident actor migrates with dirty history.
pub fn prepare_original_baseline(
    encoding: Option<&ResidentEncoding>,
    cache: &Path,
    quota: u64,
    platform: Arc<dyn LocalFileSystem>,
    options: SourceOptions,
    bytes: Budget,
    history: Budget,
    cancellation: Cancellation,
) -> Result<PagedTranscoded, FileError> {
    cancellation.check()?;
    bareline_platform::private::create_dir_all(cache)?;
    let _parent = platform.guard_directory(cache)?;
    static NEXT_BASELINE: AtomicU64 = AtomicU64::new(1);
    let directory = cache.join(format!(
        "spill-baseline-{}-{}",
        std::process::id(),
        NEXT_BASELINE.fetch_add(1, Ordering::Relaxed)
    ));
    bareline_platform::private::create_dir(&directory)?;
    if let Err(error) = crate::owned_cache::publish_ownership(
        &directory,
        crate::owned_cache::CacheKind::SpillBaseline,
        platform.as_ref(),
    ) {
        crate::owned_cache::release_empty(&directory);
        return Err(error.into());
    }
    let staging = Staging(directory);
    let directory_guard = platform.guard_directory(&staging.0)?;
    let path = staging.0.join("input.raw");
    let output = bareline_platform::private::file_options()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut writer = QuotaWriter {
        output: io::BufWriter::with_capacity(WRITE_BUFFER, output),
        directory: &staging.0,
        platform: platform.as_ref(),
        cancel: &cancellation,
        quota: quota / 2,
        written: 0,
        space: SpaceGate::default(),
    };
    if let Some(encoding) = encoding {
        writer
            .space
            .admit(platform.as_ref(), &staging.0, quota / 2, encoding.original_len() as u64)?;
        // Streams the retained original; no whole-file copy under memory pressure.
        encoding.visit_original(0..encoding.original_len(), |chunk| {
            writer.write_all(chunk).map_err(FileError::Io)
        })?;
    }
    writer.sync()?;
    let used = writer.written;
    drop(writer);
    let file = platform.open_sealed_read(&path)?;
    let mut job = DiskTranscoder::new(
        FileInput { path, file },
        platform.clone(),
        cache,
        DiskOptions {
            temp_quota_bytes: quota.saturating_sub(used),
            interpret: Some(encoding.map_or(Encoding::Utf8, ResidentEncoding::original_encoding)),
        },
        bytes.clone(),
        cancellation.clone(),
    )
    .map_err(FileError::Transcode)?;
    loop {
        if job.step().map_err(FileError::Transcode)?.complete {
            break;
        }
    }
    let mut store = job.finish().map_err(FileError::Transcode)?;
    if let Some(encoding) = encoding {
        store.state = encoding.state.clone();
    }
    let result = store
        .open_paged(platform, options, bytes, history, cancellation)
        .map_err(FileError::Transcode)?;
    result
        .source
        .source()
        .retain_owner(Arc::new(store))
        .map_err(|_| FileError::Budget)?;
    drop(directory_guard);
    Ok(result)
}

/// A paged reinterpretation between slices (FIO-14). The I/O worker first
/// proves the retained original against its sealed hash in bounded steps (a
/// pass over the file, skipped once this process proved it under its current
/// identity), then starts the transcode and steps it the same way; the
/// original stays sealed until `finish`.
pub enum Reinterpreting {
    Validating(Box<crate::codecs::disk::SealedValidation>),
    Transcoding(Box<ReinterpretTranscode>),
}
/// The transcode of a validated reinterpretation.
pub struct ReinterpretTranscode {
    pub job: DiskTranscoder,
    sealed: crate::codecs::disk::SealedStoreRead,
}
/// Seal the retained original; nothing is read until the validation steps.
pub fn start_reinterpret(request: &InterpretPagedRequest) -> Result<Reinterpreting, FileError> {
    request
        .source
        .begin_sealed_original_read()
        .map(|validation| Reinterpreting::Validating(Box::new(validation)))
        .map_err(FileError::Transcode)
}
/// Open the transcode once `validation` has stepped to completion.
pub fn start_reinterpret_transcode(
    validation: crate::codecs::disk::SealedValidation,
    request: &InterpretPagedRequest,
    platform: &Arc<dyn LocalFileSystem>,
    cancellation: &Cancellation,
) -> Result<ReinterpretTranscode, FileError> {
    let sealed = validation.into_reader().map_err(FileError::Transcode)?;
    let path = request.source.original_path();
    let file = platform.open_sealed_read(&path)?;
    let job = DiskTranscoder::new(
        FileInput { path, file },
        platform.clone(),
        &request.cache,
        DiskOptions {
            temp_quota_bytes: request.quota,
            interpret: Some(request.target),
        },
        request.bytes.clone(),
        cancellation.clone(),
    )
    .map_err(FileError::Transcode)?;
    Ok(ReinterpretTranscode { job, sealed })
}
impl ReinterpretTranscode {
    /// Publish the transcode once `job` has stepped to completion.
    pub fn finish(
        self,
        request: &InterpretPagedRequest,
        platform: Arc<dyn LocalFileSystem>,
        cancellation: Cancellation,
    ) -> Result<PagedTranscoded, FileError> {
        let Self { job, sealed } = self;
        let store = job.finish().map_err(FileError::Transcode)?;
        let result = store
            .open_paged(
                platform,
                request.options,
                request.bytes.clone(),
                request.history.clone(),
                cancellation,
            )
            .map_err(FileError::Transcode)?;
        result
            .source
            .source()
            .retain_owner(Arc::new(store))
            .map_err(|_| FileError::Budget)?;
        drop(sealed);
        Ok(result)
    }
}

/// Worker-owned append-only UTF-8 staging. No source is published until its bytes
/// are flushed and a retained immutable read capability has been acquired.
pub struct StreamingStoreBuilder {
    output: Option<io::BufWriter<std::fs::File>>,
    space: SpaceGate,
    cleanup: OwnedDirectory,
    directory_guard: Arc<dyn Send + Sync>,
    parent_guard: Arc<dyn Send + Sync>,
    platform: Arc<dyn LocalFileSystem>,
    options: SourceOptions,
    budget: Budget,
    cancel: Cancellation,
    quota: u64,
    written: u64,
    serial: u64,
    failed: bool,
    utf8_carry: Vec<u8>,
    hash: sha2::Sha256,
}
impl StreamingStoreBuilder {
    pub fn new(
        cache: &Path,
        quota: u64,
        platform: Arc<dyn LocalFileSystem>,
        options: SourceOptions,
        budget: Budget,
        cancel: Cancellation,
    ) -> io::Result<Self> {
        cancel
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "owned staging cancelled"))?;
        bareline_platform::private::create_dir_all(cache)?;
        let parent_guard = platform.guard_directory(cache)?;
        static NEXT_STREAM: AtomicU64 = AtomicU64::new(1);
        let serial = NEXT_STREAM.fetch_add(1, Ordering::Relaxed);
        let directory = cache.join(format!("owned-stream-{}-{serial}", std::process::id()));
        bareline_platform::private::create_dir(&directory)?;
        if let Err(error) = crate::owned_cache::publish_ownership(
            &directory,
            crate::owned_cache::CacheKind::OwnedStream,
            platform.as_ref(),
        ) {
            crate::owned_cache::release_empty(&directory);
            return Err(error);
        }
        let cleanup = OwnedDirectory(directory);
        let directory_guard = platform.guard_directory(&cleanup.0)?;
        let output = bareline_platform::private::file_options()
            .write(true)
            .create_new(true)
            .open(cleanup.0.join("segments.utf8"))?;
        Ok(Self {
            output: Some(io::BufWriter::with_capacity(WRITE_BUFFER, output)),
            space: SpaceGate::default(),
            cleanup,
            directory_guard,
            parent_guard,
            platform,
            options,
            budget,
            cancel,
            quota,
            written: 0,
            serial,
            failed: false,
            utf8_carry: Vec::with_capacity(4),
            hash: Default::default(),
        })
    }
    pub fn len(&self) -> u64 {
        self.written
    }
    pub fn is_empty(&self) -> bool {
        self.written == 0
    }
    pub fn append_utf8(&mut self, text: &str) -> io::Result<std::ops::Range<u64>> {
        if !self.utf8_carry.is_empty() {
            self.failed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cannot append text inside an unfinished UTF-8 scalar",
            ));
        }
        let start = self.written;
        if self.failed {
            return Err(io::Error::other("owned staging previously failed"));
        }
        let result = (|| {
            for chunk in text.as_bytes().chunks(64 * 1024) {
                self.cancel
                    .check()
                    .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "owned staging cancelled"))?;
                let effective = self.space.limit(
                    self.platform.as_ref(),
                    &self.cleanup.0,
                    self.quota,
                    self.written,
                    chunk.len() as u64,
                )?;
                if chunk.len() as u64 > effective.saturating_sub(self.written) {
                    return Err(io::Error::new(
                        io::ErrorKind::StorageFull,
                        "owned staging quota exceeded",
                    ));
                }
                self.output
                    .as_mut()
                    .ok_or_else(|| io::Error::other("owned staging sealed"))?
                    .write_all(chunk)?;
                sha2::Digest::update(&mut self.hash, chunk);
                self.written += chunk.len() as u64;
            }
            Ok(start..self.written)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn append_source(
        &mut self,
        text: &bareline_document::paged::OwnedTextRange,
    ) -> io::Result<std::ops::Range<u64>> {
        let start = self.written;
        let cancel = self.cancel.clone();
        let result = crate::owned_read::visit_utf8::<io::Error>(&text.source, text.range.clone(), &cancel, |chunk| {
            self.append_utf8(chunk).map(|_| ())
        });
        if result.is_err() {
            self.failed = true;
        }
        result.map(|_| start..self.written)
    }
    pub fn finish(mut self) -> io::Result<bareline_document::source::MemorySource> {
        use bareline_document::source::{Generation, MemorySource, SourceKind};
        if self.failed || !self.utf8_carry.is_empty() {
            return Err(io::Error::other("owned staging failed or incomplete UTF-8"));
        }
        self.cancel
            .check()
            .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "owned staging cancelled"))?;
        let mut file = self
            .output
            .take()
            .ok_or_else(|| io::Error::other("owned staging sealed"))?;
        file.flush()?;
        file.get_ref().sync_all()?;
        drop(file);
        let mut sealed = self.platform.open_sealed_read(&self.cleanup.0.join("segments.utf8"))?;
        use std::io::Read;
        let mut actual = sha2::Sha256::default();
        let mut buffer = [0u8; 65536];
        loop {
            self.cancel
                .check()
                .map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "owned staging cancelled"))?;
            let count = sealed.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            sha2::Digest::update(&mut actual, &buffer[..count]);
        }
        if sha2::Digest::finalize(actual) != sha2::Digest::finalize(self.hash) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "owned staging changed before seal",
            ));
        }
        // Distinct namespace from the captured-segment producer.
        let (source, _) = MemorySource::new(
            self.written,
            Generation((3u64 << 62) | self.serial),
            SourceKind::Paged,
            self.options.page_size_bytes,
            self.options.page_cache_bytes,
            self.budget,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        source
            .attach_owned_loader(Arc::new(SealedSegments {
                file: std::sync::Mutex::new(sealed),
                _directory_guard: self.directory_guard,
                _parent_guard: self.parent_guard,
                _cleanup: self.cleanup,
            }))
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(source)
    }
}
impl Write for StreamingStoreBuilder {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other("owned staging previously failed"));
        }
        let result = (|| {
            let mut cursor = 0;
            let mut buffer = [0u8; 8196];
            while cursor < bytes.len() {
                let prefix = self.utf8_carry.len();
                buffer[..prefix].copy_from_slice(&self.utf8_carry);
                let count = (bytes.len() - cursor).min(8192);
                buffer[prefix..prefix + count].copy_from_slice(&bytes[cursor..cursor + count]);
                cursor += count;
                self.utf8_carry.clear();
                let window = &buffer[..prefix + count];
                match std::str::from_utf8(window) {
                    Ok(text) => {
                        self.append_utf8(text)?;
                    }
                    Err(error) => {
                        if error.error_len().is_some() {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "owned staging requires valid UTF-8",
                            ));
                        }
                        let valid = error.valid_up_to();
                        let text = std::str::from_utf8(&window[..valid]).map_err(io::Error::other)?;
                        self.append_utf8(text)?;
                        self.utf8_carry.extend_from_slice(&window[valid..]);
                    }
                }
            }
            Ok(bytes.len())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("owned staging previously failed"));
        }
        self.output
            .as_mut()
            .ok_or_else(|| io::Error::other("owned staging sealed"))?
            .flush()
    }
}

#[cfg(test)]
mod registered_producer_tests {
    use super::*;
    use bareline_document::Document;
    use bareline_platform::FileIdentity;
    use std::fs::File;

    struct Platform;
    impl LocalFileSystem for Platform {
        fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
            Ok(Arc::new(()))
        }
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            Ok(u64::MAX)
        }
        fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
            File::open(path)
        }
        fn identity(&self, file: &File) -> io::Result<FileIdentity> {
            let metadata = file.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: metadata.len(),
                length: metadata.len(),
                modified: metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            })
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::other("not used"))
        }
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-owned-producer-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn count_prefix(root: &Path, prefix: &str) -> usize {
        fs::read_dir(root)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(prefix))
            })
            .count()
    }

    #[test]
    fn resident_and_original_baseline_route_nested_transcodes_to_exact_registered_root() {
        let temp = Temp::new();
        let owned = temp.0.join("Bareline-owned-spill");
        let transcode = temp.0.join("Bareline-transcode");
        fs::create_dir(&owned).unwrap();
        let platform: Arc<dyn LocalFileSystem> = Arc::new(Platform);
        let bytes = Budget::new(32 * 1024 * 1024);
        let history = Budget::new(1024 * 1024);
        let document = Document::from_utf8("resident", bytes.clone(), history.clone()).unwrap();

        let resident = prepare_resident(
            &document.snapshot(),
            None,
            false,
            &owned,
            8 * 1024 * 1024,
            platform.clone(),
            SourceOptions::default(),
            bytes.clone(),
            history.clone(),
            Cancellation::default(),
        )
        .unwrap();
        // REC-11: the staging directory, ownership record included, is gone once the
        // producer returns; only the published transcode remains.
        assert_eq!(count_prefix(&owned, "resident-spill-"), 0);
        assert_eq!(count_prefix(&owned, "bareline-transcode-"), 0);
        assert_eq!(count_prefix(&transcode, "bareline-transcode-"), 1);
        drop(resident);

        let prior_transcodes = count_prefix(&transcode, "bareline-transcode-");
        let baseline = prepare_original_baseline(
            None,
            &owned,
            8 * 1024 * 1024,
            platform,
            SourceOptions::default(),
            bytes,
            history,
            Cancellation::default(),
        )
        .unwrap();
        assert_eq!(count_prefix(&owned, "spill-baseline-"), 0);
        assert_eq!(count_prefix(&owned, "bareline-transcode-"), 0);
        assert_eq!(count_prefix(&transcode, "bareline-transcode-"), prior_transcodes + 1);
        drop(baseline);
    }
}

#[cfg(test)]
mod space_gate_tests {
    use super::*;

    /// A volume with a fixed amount of free space that counts its queries.
    struct Volume(AtomicU64);
    impl LocalFileSystem for Volume {
        fn available_space(&self, _: &Path) -> io::Result<u64> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(100)
        }
        fn identity(&self, _: &std::fs::File) -> io::Result<bareline_platform::FileIdentity> {
            Err(io::Error::other("not used"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> io::Result<()> {
            Err(io::Error::other("not used"))
        }
    }

    /// FIO-08: `free` keeps a per-write rule on sampled free space. The writer's own
    /// growth since the sample counts as used, and a write the estimate cannot hold
    /// samples the volume again rather than being refused on a stale estimate.
    #[test]
    fn free_space_is_estimated_between_samples_and_resampled_before_refusing() {
        let volume = Volume(AtomicU64::new(0));
        let directory = Path::new("unused");
        let mut gate = SpaceGate::default();
        assert_eq!(gate.free(&volume, directory, 0, 50).unwrap(), 100);
        assert_eq!(gate.free(&volume, directory, 30, 50).unwrap(), 70);
        assert_eq!(volume.0.load(Ordering::SeqCst), 1);
        // 40 bytes left by the estimate cannot hold 50: the volume is asked again.
        assert_eq!(gate.free(&volume, directory, 60, 50).unwrap(), 100);
        assert_eq!(volume.0.load(Ordering::SeqCst), 2);
        assert_eq!(gate.samples(), 2);
        // Writing far past the last sample samples again even when the estimate fits.
        assert_eq!(gate.free(&volume, directory, 60 + SPACE_RECHECK_BYTES, 0).unwrap(), 100);
        assert_eq!(gate.samples(), 3);
    }
}
