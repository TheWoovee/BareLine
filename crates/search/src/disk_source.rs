// SPDX-License-Identifier: MPL-2.0
//! Private disk-backed decoded sources shared by folder search and workspace replacement.
use super::*;
use bareline_document::{
    Budget,
    paged::{PagedSnapshot, TextWindow, WindowPoll},
};
use bareline_file_io::{
    codecs::disk::DiskOptions,
    lifecycle::{PagedOpenRequest, PagedOpened, TranscodeOutcome, open_paged_encoded},
    source::SourceOptions,
};
use bareline_platform::{LocalFileSystem, PathOperation, PathOrigin, PathTrustProvider};
use std::{io, path::Path};
pub(crate) fn open(
    path: &Path,
    trust: &dyn PathTrustProvider,
    platform: Arc<dyn LocalFileSystem>,
    job: &SearchJob,
) -> io::Result<Box<PagedOpened>> {
    let approved = trust.open_read(path, PathOrigin::User)?;
    if approved.trust.traverses_reparse_point
        || !trust.permits(&approved.trust, PathOperation::Read)
    {
        return Err(io::Error::other("Untrusted source"));
    }
    let expected = platform.identity(&approved.file)?;
    let opened = match open_paged_encoded(
        PagedOpenRequest {
            path: approved.trust.canonical.clone(),
            bytes: Budget::new(64 * 1024 * 1024),
            history: Budget::new(MAX_RESULT_BYTES),
            cache: std::env::temp_dir(),
            options: DiskOptions {
                temp_quota_bytes: 20u64 << 30,
                interpret: None,
            },
            source_options: SourceOptions {
                resident_max_bytes: 0,
                page_size_bytes: 1024 * 1024,
                page_cache_bytes: 8 * 1024 * 1024,
            },
        },
        platform.clone(),
        job.io_cancel.clone(),
        |_| {},
    ) {
        TranscodeOutcome::Complete(opened) => opened,
        TranscodeOutcome::Paused(_) => {
            return Err(io::Error::other("Temporary source quota reached"));
        }
        TranscodeOutcome::Failed(error) => return Err(io::Error::other(format!("{error:?}"))),
    };
    if opened.fingerprint.identity != expected || platform.identity(&approved.file)? != expected {
        return Err(io::Error::other("Source changed during decode"));
    }
    Ok(opened)
}
pub(crate) fn window(
    opened: &mut PagedOpened,
    snapshot: &PagedSnapshot,
    start: usize,
    count: usize,
    job: &SearchJob,
) -> io::Result<TextWindow> {
    if count > 16 * 1024 * 1024 {
        return Err(io::Error::other("Read window limit"));
    }
    let mut request = snapshot
        .begin_viewport(TextOffset(start), count, &Budget::new(count))
        .map_err(|error| io::Error::other(format!("{error:?}")))?;
    loop {
        if job.is_cancelled() {
            return Err(io::Error::other("Cancelled"));
        }
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(window),
            WindowPoll::Pending(ticket) => opened
                .transcoded
                .source
                .read_page(ticket)
                .map_err(|error| io::Error::other(format!("{error:?}")))?,
            _ => return Err(io::Error::other("Source unavailable")),
        }
    }
}
