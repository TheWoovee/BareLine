// SPDX-License-Identifier: MPL-2.0
use bareline_app::workspace::extensions::OriginalSource;
use bareline_document::{Budget, TextOffset, paged::WindowPoll};
use bareline_editor_surface::paged_view::PagedReadHandle;
use bareline_extensions_protocol::{RawRange, TextRange};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
pub(super) struct Readers {
    original: Option<OriginalSource>,
    sealed: Option<bareline_file_io::codecs::disk::SealedStoreRead>,
    file: Option<std::fs::File>,
    pub paged: Option<PagedReadHandle>,
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_bytes_are_exact_bounded_and_cancelled_without_text_fallback() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let original = vec![0xff, 0xfe, 0x41, 0, 0, 0xd8];
        let mut reader = Readers::new(
            Some(OriginalSource::Resident(Arc::new(original.clone()))),
            None,
            cancelled.clone(),
            Instant::now() + std::time::Duration::from_secs(1),
        );
        assert_eq!(reader.raw(RawRange { start: 0, end: 6 }).unwrap(), original);
        assert!(reader.raw(RawRange { start: 0, end: 7 }).is_err());
        assert!(reader.text(TextRange { start: 0, end: 1 }).is_err());
        cancelled.store(true, Ordering::Release);
        assert!(reader.raw(RawRange { start: 0, end: 1 }).is_err());
    }
}
impl Readers {
    pub fn new(
        original: Option<OriginalSource>,
        paged: Option<PagedReadHandle>,
        cancel: Arc<AtomicBool>,
        deadline: Instant,
    ) -> Self {
        Self {
            original,
            sealed: None,
            file: None,
            paged,
            cancel,
            deadline,
        }
    }
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err("Extension read cancelled or timed out".into())
        } else {
            Ok(())
        }
    }
    pub fn raw(&mut self, range: RawRange) -> Result<Vec<u8>, String> {
        self.check()?;
        let source = self
            .original
            .as_ref()
            .ok_or("Original byte source unavailable")?;
        if range.start > range.end || range.end > source.len() || range.end - range.start > 65536 {
            return Err("Original range limit".into());
        }
        match source {
            OriginalSource::Resident(bytes) => {
                Ok(bytes[range.start as usize..range.end as usize].to_vec())
            }
            OriginalSource::File { .. } => {
                if self.file.is_none() {
                    self.file = source.verified_file(&self.cancel)?;
                }
                let reader = self.file.as_mut().ok_or("Original file unavailable")?;
                reader
                    .seek(SeekFrom::Start(range.start))
                    .map_err(|e| e.to_string())?;
                let mut bytes = vec![0; (range.end - range.start) as usize];
                reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                self.check()?;
                Ok(bytes)
            }
            OriginalSource::Paged(store) => {
                if self.sealed.is_none() {
                    self.sealed = Some(
                        store
                            .sealed_original_reader(
                                &bareline_file_io::cancellation::Cancellation::from_flag(
                                    self.cancel.clone(),
                                ),
                            )
                            .map_err(|e| format!("Original source: {e:?}"))?,
                    );
                }
                let reader = self.sealed.as_mut().unwrap();
                reader
                    .seek(SeekFrom::Start(range.start))
                    .map_err(|e| e.to_string())?;
                let mut bytes = vec![0; (range.end - range.start) as usize];
                reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                self.check()?;
                Ok(bytes)
            }
        }
    }
    pub fn text(&mut self, range: TextRange) -> Result<Vec<u8>, String> {
        self.check()?;
        let handle = self.paged.as_ref().ok_or("Paged text source unavailable")?;
        let start = usize::try_from(range.start).map_err(|_| "Text offset overflow")?;
        let end = usize::try_from(range.end).map_err(|_| "Text offset overflow")?;
        if start > end || end > handle.snapshot().len() || end - start > 65536 {
            return Err("Text range limit".into());
        }
        // Include neighbouring scalars; viewport alignment removes only bytes
        // outside the requested byte range, which may itself split a scalar.
        let left = start.saturating_sub(3);
        let right = end.saturating_add(3).min(handle.snapshot().len());
        let mut request = handle
            .snapshot()
            .begin_viewport(TextOffset(left), right - left, &Budget::new(65542))
            .map_err(|e| format!("{e:?}"))?;
        loop {
            self.check()?;
            match request.poll() {
                WindowPoll::Ready(window) => {
                    let base = window.range().start.0;
                    if base > start || window.range().end.0 < end {
                        return Err("Text window boundary unavailable".into());
                    }
                    return Ok(window.text().as_bytes()[start - base..end - base].to_vec());
                }
                WindowPoll::Pending(ticket) => {
                    if !handle.resolve_page(ticket)? {
                        std::thread::yield_now();
                    }
                }
                _ => return Err("Paged text source changed or became unavailable".into()),
            }
        }
    }
}
