// SPDX-License-Identifier: MPL-2.0
//! Prepare a private paged copy of a Resident snapshot on the existing I/O pool.
//! No mutation/publication occurs here; the actor validates the captured identity later.
use crate::{cancellation::Cancellation, codecs::{Encoding, disk::{DiskOptions, DiskTranscoder, PagedTranscoded}, resident::ResidentEncoding}, lifecycle::{FileError, FileInput}, source::SourceOptions};
use bareline_document::{Budget, DocumentSnapshot, TextOffset};
use bareline_platform::LocalFileSystem;
use std::{fs::{self, OpenOptions}, io::{self, Write}, path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicU64, Ordering}}};
struct Staging(PathBuf);
impl Drop for Staging { fn drop(&mut self) { let _ = fs::remove_file(self.0.join("input.raw")); let _ = fs::remove_dir(&self.0); } }
struct QuotaWriter<'a> { output: std::fs::File, directory: &'a Path, platform: &'a dyn LocalFileSystem, cancel: &'a Cancellation, quota: u64, written: u64 }
impl Write for QuotaWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.cancel.check().map_err(|_| io::Error::new(io::ErrorKind::Interrupted, "spill cancelled"))?;
        let effective = self.quota.min(self.platform.available_space(self.directory)?.saturating_add(self.written) / 5);
        if bytes.len() as u64 > effective.saturating_sub(self.written) { return Err(io::Error::new(io::ErrorKind::StorageFull, "owned spill quota exceeded; Resident data retained")); }
        let count = self.output.write(bytes)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> { self.output.flush() }
}
/// The original-byte/provenance store is rebuilt through the existing codec pipeline,
/// preserving unchanged legacy aliases and invalid spans. Failure leaves the actor intact.
pub fn prepare_resident(
    snapshot: &DocumentSnapshot, encoding: Option<&ResidentEncoding>, bom: bool,
    cache: &Path, quota: u64, platform: Arc<dyn LocalFileSystem>, options: SourceOptions,
    bytes: Budget, history: Budget, cancellation: Cancellation,
) -> Result<PagedTranscoded, FileError> {
    if !snapshot.is_complete() { return Err(FileError::IncompleteSource); }
    cancellation.check()?;
    fs::create_dir_all(cache)?;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = cache.join(format!("resident-spill-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&directory)?;
    let staging = Staging(directory);
    let path = staging.0.join("input.raw");
    let output = OpenOptions::new().write(true).create_new(true).open(&path)?;
    // Reserve space for both staged raw input and the durable raw/text/map triplet.
    let mut writer = QuotaWriter { output, directory: &staging.0, platform: platform.as_ref(), cancel: &cancellation, quota: quota / 2, written: 0 };
    if let Some(encoding) = encoding {
        encoding.write_snapshot(snapshot, encoding.state.save_target, encoding.state.bom, &mut writer).map_err(FileError::Encoding)?;
    } else {
        if bom { writer.write_all(&[0xef, 0xbb, 0xbf])?; }
        for chunk in snapshot.chunks(TextOffset(0)..TextOffset(snapshot.len())).map_err(|_| FileError::IncompleteSource)? { writer.write_all(chunk.as_bytes())?; }
    }
    writer.output.sync_all()?;
    let used = writer.written;
    drop(writer);
    let input = FileInput { path: path.clone(), file: std::fs::File::open(&path)? };
    let options_disk = DiskOptions { temp_quota_bytes: quota.saturating_sub(used), interpret: Some(encoding.map_or(Encoding::Utf8, |encoding| encoding.state.save_target)) };
    let mut job = if encoding.map_or(bom, |encoding| encoding.state.bom) {
        DiskTranscoder::new(input, platform.clone(), cache, options_disk, bytes.clone(), cancellation.clone())
    } else {
        DiskTranscoder::continuation(input, platform.clone(), cache, options_disk, bytes.clone(), cancellation.clone())
    }.map_err(FileError::Transcode)?;
    loop { if job.step().map_err(FileError::Transcode)?.complete { break; } }
    let store = job.finish().map_err(FileError::Transcode)?;
    let result = store.open_paged(platform, options, bytes, history, cancellation).map_err(FileError::Transcode)?;
    // Snapshots keep the store alive even after the producer/controller is dropped.
    result.source.source().retain_owner(Arc::new(store)).map_err(|_| FileError::Budget)?;
    Ok(result)
}
