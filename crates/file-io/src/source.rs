// SPDX-License-Identifier: MPL-2.0
//! Raw source paging. Keep the producer on an I/O worker; give consumers its MemorySource.
use crate::{cancellation::Cancellation, lifecycle::FileError};
use bareline_document::{
    Budget,
    source::{Generation, MemorySource, PageTicket, SourceKind, SourcePublisher},
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy)]
pub struct SourceOptions {
    pub resident_max_bytes: u64,
    pub page_size_bytes: usize,
    pub page_cache_bytes: usize,
}

impl Default for SourceOptions {
    fn default() -> Self {
        Self {
            resident_max_bytes: 256 * 1024 * 1024,
            page_size_bytes: 1024 * 1024,
            page_cache_bytes: 64 * 1024 * 1024,
        }
    }
}

pub struct FileSource {
    file: File,
    path: PathBuf,
    platform: Arc<dyn LocalFileSystem>,
    identity: FileIdentity,
    generation: Generation,
    kind: SourceKind,
    source: MemorySource,
    publisher: SourcePublisher,
    cancellation: Cancellation,
}
impl FileSource {
    /// Opens metadata only. No full-file scan or allocation precedes the requested viewport.
    pub fn open(
        path: &Path,
        platform: Arc<dyn LocalFileSystem>,
        options: SourceOptions,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        cancellation.check()?;
        platform.validate_source(path)?;
        let file = File::open(path)?;
        let identity = platform.identity(&file)?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let generation = Generation(NEXT.fetch_add(1, Ordering::Relaxed));
        let kind = if identity.length <= options.resident_max_bytes {
            SourceKind::Resident
        } else {
            SourceKind::Paged
        };
        let (source, publisher) = MemorySource::new(
            identity.length,
            generation,
            kind,
            options.page_size_bytes,
            options.page_cache_bytes,
            budget,
        )
        .map_err(|_| FileError::Budget)?;
        Ok(Self {
            file,
            path: path.to_owned(),
            platform,
            identity,
            generation,
            kind,
            source,
            publisher,
            cancellation,
        })
    }
    /// Starts a fresh paged generation only after tail continuity verification.
    /// Old sources and their published Ready ranges are never modified.
    pub fn open_verified_generation(
        path: &Path,
        platform: Arc<dyn LocalFileSystem>,
        mut options: SourceOptions,
        budget: Budget,
        cancellation: Cancellation,
        verified: &crate::lifecycle::Fingerprint,
    ) -> Result<Self, FileError> {
        options.resident_max_bytes = 0;
        let producer = Self::open(path, platform, options, budget, cancellation)?;
        if producer.identity != verified.identity {
            return Err(FileError::Changed);
        }
        producer.check()?;
        Ok(producer)
    }
    pub fn source(&self) -> MemorySource {
        self.source.clone()
    }
    pub fn kind(&self) -> SourceKind {
        self.kind
    }
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }
    fn check(&self) -> Result<(), FileError> {
        if let Err(error) = self.cancellation.check() {
            self.publisher.cancel();
            return Err(error);
        }
        let checked = (|| {
            if self.platform.identity(&self.file)? != self.identity
                || self.platform.identity(&File::open(&self.path)?)? != self.identity
            {
                return Err(FileError::Changed);
            }
            Ok(())
        })();
        if checked.is_err() {
            self.publisher.mark_changed();
        }
        checked
    }
    /// Services one Pending ticket. Cached old-generation pages stay immutable on failure.
    pub fn read_page(&mut self, ticket: PageTicket) -> Result<(), FileError> {
        if ticket.generation != self.generation {
            return Err(FileError::Changed);
        }
        let start = ticket
            .page
            .checked_mul(self.source.page_size() as u64)
            .filter(|start| *start < self.identity.length)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "page outside source")
            })?;
        self.check()?;
        let end = start + (self.identity.length - start).min(self.source.page_size() as u64);
        if matches!(
            self.source.read(start..end),
            bareline_document::source::SourceRead::Ready(_)
        ) {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(start))?;
        let mut buffer = self
            .publisher
            .prepare_page(ticket)
            .map_err(|error| match error {
                bareline_document::Error::BudgetExceeded => FileError::Budget,
                _ => FileError::Changed,
            })?;
        for chunk in buffer.bytes_mut().chunks_mut(64 * 1024) {
            if let Err(error) = self.cancellation.check() {
                self.publisher.cancel();
                return Err(error);
            }
            if let Err(error) = self.file.read_exact(chunk) {
                self.publisher.mark_changed();
                return Err(error.into());
            }
        }
        self.check()?;
        self.publisher
            .publish_buffer(buffer, self.generation)
            .map_err(|error| match error {
                bareline_document::Error::BudgetExceeded => FileError::Budget,
                _ => FileError::Changed,
            })
    }
    /// Fill at most one sequential page per worker turn, allowing viewport jobs to take priority.
    /// Returns true after the final verified Resident page. Paged sources are never fully filled.
    pub fn fill_resident_page(&mut self, page: u64) -> Result<bool, FileError> {
        if self.kind != SourceKind::Resident {
            return Err(FileError::StreamingRequired);
        }
        if self.identity.length == 0 {
            self.check()?;
            return Ok(true);
        }
        self.read_page(PageTicket {
            generation: self.generation,
            page,
        })?;
        self.check()?;
        match self.publisher.seal(self.generation) {
            Ok(()) => Ok(true),
            Err(bareline_document::Error::OutOfBounds) => Ok(false),
            Err(_) => Err(FileError::Changed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::source::{SourceRead, Unavailable};
    struct Platform;
    impl LocalFileSystem for Platform {
        fn identity(&self, file: &File) -> std::io::Result<FileIdentity> {
            let meta = file.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: meta.len(),
                modified: meta
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
            unreachable!()
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "bareline-paging-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn pending(source: &MemorySource, range: std::ops::Range<u64>) -> PageTicket {
        match source.read(range) {
            SourceRead::Pending(ticket) => ticket,
            _ => panic!("expected pending"),
        }
    }
    #[test]
    fn gigabyte_source_reads_requested_page_only_and_fails_closed_after_truncation() {
        let fixture = Fixture::new();
        File::options()
            .write(true)
            .open(&fixture.0)
            .unwrap()
            .set_len(1024 * 1024 * 1024)
            .unwrap();
        let budget = Budget::new(8192);
        let mut producer = FileSource::open(
            &fixture.0,
            Arc::new(Platform),
            SourceOptions {
                resident_max_bytes: 1,
                page_size_bytes: 4096,
                page_cache_bytes: 4096,
            },
            budget.clone(),
            Cancellation::default(),
        )
        .unwrap();
        assert_eq!(producer.kind(), SourceKind::Paged);
        let source = producer.source();
        assert_eq!(budget.used(), 0);
        producer.read_page(pending(&source, 4096..8192)).unwrap();
        assert_eq!(budget.used(), 4096);
        let SourceRead::Ready(retained) = source.read(4096..8192) else {
            panic!()
        };
        producer.read_page(pending(&source, 0..4096)).unwrap();
        assert_eq!(budget.used(), 8192);
        File::options()
            .write(true)
            .open(&fixture.0)
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(matches!(
            producer.read_page(pending(&source, 8192..12288)),
            Err(FileError::Changed)
        ));
        assert!(matches!(
            source.read(8192..12288),
            SourceRead::Unavailable(Unavailable::SourceChanged)
        ));
        assert_eq!(retained.bytes().len(), 4096);
        drop(retained);
        assert_eq!(budget.used(), 4096);
    }
    #[test]
    fn resident_seals_and_cancelled_missing_pages_are_unavailable() {
        let fixture = Fixture::new();
        std::fs::write(&fixture.0, b"abcdefgh").unwrap();
        let options = SourceOptions {
            resident_max_bytes: 8,
            page_size_bytes: 4,
            page_cache_bytes: 4,
        };
        let mut producer = FileSource::open(
            &fixture.0,
            Arc::new(Platform),
            options,
            Budget::new(8),
            Cancellation::default(),
        )
        .unwrap();
        assert!(!producer.fill_resident_page(1).unwrap());
        assert!(producer.fill_resident_page(0).unwrap());
        let source = producer.source();
        assert!(source.sealed());
        std::fs::write(&fixture.0, b"changed").unwrap();
        assert!(matches!(source.read(0..4), SourceRead::Ready(_)));
        let cancellation = Cancellation::default();
        let mut producer = FileSource::open(
            &fixture.0,
            Arc::new(Platform),
            options,
            Budget::new(8),
            cancellation.clone(),
        )
        .unwrap();
        let source = producer.source();
        let ticket = pending(&source, 0..4);
        cancellation.cancel();
        assert!(matches!(
            producer.read_page(ticket),
            Err(FileError::Cancelled)
        ));
        assert!(matches!(
            source.read(0..4),
            SourceRead::Unavailable(Unavailable::Cancelled)
        ));
    }
    #[test]
    fn streaming_resident_publishes_prefix_before_eof_and_preserves_utf8_bom() {
        let fixture = Fixture::new();
        let mut content = vec![b'a'; 2 * 1024 * 1024];
        content.splice(..3, [0xef, 0xbb, 0xbf]);
        content.splice(65535..65535, "😀".bytes());
        std::fs::write(&fixture.0, &content).unwrap();
        let mut prefix = None;
        let opened = crate::lifecycle::open_utf8_streaming(
            &fixture.0,
            &Platform,
            Budget::new(content.len()),
            Budget::new(0),
            &Cancellation::default(),
            3 * 1024 * 1024,
            |snapshot| {
                assert!(snapshot.len() < content.len());
                prefix = Some(snapshot);
            },
        )
        .unwrap();
        assert!(opened.bom);
        let full = opened.document.snapshot();
        assert_eq!(
            full.read(
                bareline_document::TextOffset(0)..bareline_document::TextOffset(full.len()),
                content.len()
            )
            .unwrap()
            .as_bytes(),
            &content[3..]
        );
        let prefix = prefix.unwrap();
        assert!(prefix.len() < full.len());
        assert!(!prefix.is_complete());
        assert!(full.is_complete());
        assert_ne!(prefix.content_state, full.content_state);
        assert_ne!(prefix.revision, full.revision);
        assert!(matches!(
            crate::lifecycle::save_utf8(prefix.clone(), &fixture.0, None, true, &Platform),
            Err(FileError::IncompleteSource)
        ));
        let mut document = opened.document;
        assert_eq!(
            document.mark_saved(&prefix),
            Err(bareline_document::Error::IncompleteSource)
        );
        assert!(!document.dirty());
        let cancellation = Cancellation::default();
        assert!(matches!(
            crate::lifecycle::open_utf8_streaming(
                &fixture.0,
                &Platform,
                Budget::new(content.len()),
                Budget::new(0),
                &cancellation,
                3 * 1024 * 1024,
                |_| cancellation.cancel()
            ),
            Err(FileError::Cancelled)
        ));
    }
}
