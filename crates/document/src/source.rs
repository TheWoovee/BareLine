// SPDX-License-Identifier: MPL-2.0
//! Generation-aware, nonblocking byte availability. Background readers publish owned pages.
use crate::{Budget, Error, Reservation};
use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
    sync::{Arc, Mutex, TryLockError},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Resident,
    Paged,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    SourceChanged,
    InvalidRange,
    Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageTicket {
    pub generation: Generation,
    pub page: u64,
}
pub enum SourceRead {
    Ready(PageSlice),
    Pending(PageTicket),
    Unavailable(Unavailable),
}
pub trait ByteSource: Send + Sync {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read(&self, range: Range<u64>) -> SourceRead;
    fn sealed(&self) -> bool;
}
pub struct PageSlice {
    page: Arc<Page>,
    range: Range<usize>,
    pub generation: Generation,
}
impl PageSlice {
    pub fn bytes(&self) -> &[u8] {
        &self.page.bytes[self.range.clone()]
    }
}
struct Page {
    bytes: Box<[u8]>,
    _reservation: Reservation,
}
struct Pages {
    pages: BTreeMap<u64, Arc<Page>>,
    lru: VecDeque<u64>,
    resident_bytes: usize,
}
struct Inner {
    owner: Mutex<Option<Arc<dyn Send + Sync>>>,
    generation: Generation,
    length: u64,
    page_size: usize,
    cache_limit: usize,
    kind: SourceKind,
    budget: Budget,
    pages: Mutex<Pages>,
    changed: AtomicBool,
    sealed: AtomicBool,
    cancelled: AtomicBool,
}
#[derive(Clone)]
pub struct MemorySource(Arc<Inner>);
/// Keep on the bounded I/O worker. The UI only holds ByteSource.
pub struct SourcePublisher(Arc<Inner>);
/// A page allocation already charged to the source budget, including while I/O is pending.
pub struct SourcePageBuffer {
    source: Arc<Inner>,
    ticket: PageTicket,
    bytes: Box<[u8]>,
    reservation: Reservation,
}
impl SourcePageBuffer {
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}
impl ByteSource for MemorySource {
    fn len(&self) -> u64 {
        MemorySource::len(self)
    }
    fn read(&self, range: Range<u64>) -> SourceRead {
        MemorySource::read(self, range)
    }
    fn sealed(&self) -> bool {
        MemorySource::sealed(self)
    }
}
impl MemorySource {
    pub fn new(
        length: u64,
        generation: Generation,
        kind: SourceKind,
        page_size: usize,
        cache_limit: usize,
        budget: Budget,
    ) -> Result<(Self, SourcePublisher), Error> {
        if page_size == 0 || cache_limit < page_size {
            return Err(Error::BudgetExceeded);
        }
        let inner = Arc::new(Inner {
            owner: Mutex::new(None),
            generation,
            length,
            page_size,
            cache_limit,
            kind,
            budget,
            pages: Mutex::new(Pages {
                pages: BTreeMap::new(),
                lru: VecDeque::new(),
                resident_bytes: 0,
            }),
            changed: AtomicBool::new(false),
            sealed: AtomicBool::new(length == 0),
            cancelled: AtomicBool::new(false),
        });
        Ok((Self(inner.clone()), SourcePublisher(inner)))
    }
    /// Retains private backing-store ownership for every snapshot containing this source.
    /// Attach before publishing the source; a second owner is refused.
    pub fn retain_owner(&self, owner: Arc<dyn Send + Sync>) -> Result<(), Error> {
        let mut slot = self.0.owner.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_some() { return Err(Error::WrongDocument); }
        *slot = Some(owner);
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.0.length
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn page_size(&self) -> usize {
        self.0.page_size
    }
    pub fn generation(&self) -> Generation { self.0.generation }
    pub fn sealed(&self) -> bool {
        self.0.sealed.load(Ordering::Acquire)
    }
    /// Ranges are confined to one source page; consumers iterate page-bounded requests.
    /// No disk I/O, mutex wait, allocation, or invented zero bytes occurs on this path.
    pub fn read(&self, range: Range<u64>) -> SourceRead {
        if range.start >= range.end
            || range.end > self.len()
            || range.start / self.0.page_size as u64 != (range.end - 1) / self.0.page_size as u64
        {
            return SourceRead::Unavailable(Unavailable::InvalidRange);
        }
        let index = range.start / self.0.page_size as u64;
        let ticket = PageTicket {
            generation: self.0.generation,
            page: index,
        };
        let mut pages = match self.0.pages.try_lock() {
            Ok(pages) => pages,
            Err(TryLockError::WouldBlock) => return SourceRead::Pending(ticket),
            Err(TryLockError::Poisoned(_)) => {
                return SourceRead::Unavailable(Unavailable::Cancelled);
            }
        };
        if let Some(page) = pages.pages.get(&index).cloned() {
            if self.0.kind == SourceKind::Paged {
                if let Some(i) = pages.lru.iter().position(|p| *p == index) {
                    pages.lru.remove(i);
                }
                pages.lru.push_back(index);
            }
            let start = (range.start % self.0.page_size as u64) as usize;
            return SourceRead::Ready(PageSlice {
                page,
                range: start..start + (range.end - range.start) as usize,
                generation: self.0.generation,
            });
        }
        if self.0.cancelled.load(Ordering::Acquire) {
            return SourceRead::Unavailable(Unavailable::Cancelled);
        }
        if self.0.changed.load(Ordering::Acquire) {
            return SourceRead::Unavailable(Unavailable::SourceChanged);
        }
        SourceRead::Pending(ticket)
    }
}
impl SourcePublisher {
    pub fn prepare_page(&self, ticket: PageTicket) -> Result<SourcePageBuffer, Error> {
        if ticket.generation != self.0.generation
            || self.0.changed.load(Ordering::Acquire)
            || self.0.cancelled.load(Ordering::Acquire)
        {
            return Err(Error::StaleRevision);
        }
        let start = ticket
            .page
            .checked_mul(self.0.page_size as u64)
            .filter(|start| *start < self.0.length)
            .ok_or(Error::OutOfBounds)?;
        let length = (self.0.length - start).min(self.0.page_size as u64) as usize;
        let mut pages = self.0.pages.lock().map_err(|_| Error::StaleRevision)?;
        if self.0.kind == SourceKind::Paged {
            while pages.resident_bytes + length > self.0.cache_limit {
                let Some(old) = pages.lru.pop_front() else {
                    break;
                };
                if let Some(page) = pages.pages.remove(&old) {
                    pages.resident_bytes -= page.bytes.len();
                }
            }
        }
        let reservation = self.0.budget.reserve(length)?;
        drop(pages);
        Ok(SourcePageBuffer {
            source: self.0.clone(),
            ticket,
            bytes: vec![0; length].into_boxed_slice(),
            reservation,
        })
    }
    /// Transfers the prepared allocation into the cache without a second page copy.
    pub fn publish_buffer(
        &self,
        buffer: SourcePageBuffer,
        verified: Generation,
    ) -> Result<(), Error> {
        if !Arc::ptr_eq(&buffer.source, &self.0) || verified != self.0.generation {
            return Err(Error::StaleRevision);
        }
        let mut pages = self.0.pages.lock().map_err(|_| Error::StaleRevision)?;
        if self.0.changed.load(Ordering::Acquire) || self.0.cancelled.load(Ordering::Acquire) {
            return Err(Error::StaleRevision);
        }
        if pages.pages.contains_key(&buffer.ticket.page) {
            return Ok(());
        }
        if self.0.kind == SourceKind::Paged {
            while pages.resident_bytes + buffer.bytes.len() > self.0.cache_limit {
                let Some(old) = pages.lru.pop_front() else {
                    break;
                };
                if let Some(page) = pages.pages.remove(&old) {
                    pages.resident_bytes -= page.bytes.len();
                }
            }
        }
        pages.resident_bytes += buffer.bytes.len();
        pages.pages.insert(
            buffer.ticket.page,
            Arc::new(Page {
                bytes: buffer.bytes,
                _reservation: buffer.reservation,
            }),
        );
        pages.lru.push_back(buffer.ticket.page);
        Ok(())
    }
    /// Caller verifies source identity before and after the read and passes that generation.
    pub fn publish(
        &self,
        ticket: PageTicket,
        bytes: &[u8],
        verified: Generation,
    ) -> Result<(), Error> {
        if ticket.generation != self.0.generation
            || verified != self.0.generation
            || self.0.changed.load(Ordering::Acquire)
            || self.0.cancelled.load(Ordering::Acquire)
        {
            return Err(Error::StaleRevision);
        }
        let start = ticket
            .page
            .checked_mul(self.0.page_size as u64)
            .ok_or(Error::OutOfBounds)?;
        if start >= self.0.length
            || bytes.len() != (self.0.length - start).min(self.0.page_size as u64) as usize
        {
            return Err(Error::OutOfBounds);
        }
        let mut pages = self.0.pages.lock().map_err(|_| Error::StaleRevision)?;
        if self.0.changed.load(Ordering::Acquire) || self.0.cancelled.load(Ordering::Acquire) {
            return Err(Error::StaleRevision);
        }
        if pages.pages.contains_key(&ticket.page) {
            return Ok(());
        }
        if self.0.kind == SourceKind::Paged {
            while pages.resident_bytes + bytes.len() > self.0.cache_limit {
                let Some(old) = pages.lru.pop_front() else {
                    break;
                };
                if let Some(page) = pages.pages.remove(&old) {
                    pages.resident_bytes -= page.bytes.len();
                }
            }
        }
        // Outstanding PageSlices keep their reservations after eviction; the global budget
        // therefore includes snapshot readers instead of hiding their retained memory.
        let reservation = self.0.budget.reserve(bytes.len())?;
        let page = Arc::new(Page {
            bytes: bytes.into(),
            _reservation: reservation,
        });
        pages.resident_bytes += bytes.len();
        pages.pages.insert(ticket.page, page);
        pages.lru.push_back(ticket.page);
        Ok(())
    }
    pub fn mark_changed(&self) {
        let Ok(_pages) = self.0.pages.lock() else {
            self.0.changed.store(true, Ordering::Release);
            return;
        };
        // Sealed Resident bytes are independent of the original disk generation.
        if !self.0.sealed.load(Ordering::Acquire) {
            self.0.changed.store(true, Ordering::Release);
        }
    }
    pub fn cancel(&self) {
        let _pages = self.0.pages.lock();
        self.0.cancelled.store(true, Ordering::Release);
    }
    pub fn seal(&self, verified: Generation) -> Result<(), Error> {
        let pages = self.0.pages.lock().map_err(|_| Error::StaleRevision)?;
        if self.0.kind != SourceKind::Resident
            || verified != self.0.generation
            || self.0.changed.load(Ordering::Acquire)
            || self.0.cancelled.load(Ordering::Acquire)
        {
            return Err(Error::StaleRevision);
        }
        if pages.resident_bytes as u64 != self.0.length {
            return Err(Error::OutOfBounds);
        }
        self.0.sealed.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eviction_and_generation_changes_never_fabricate_source_bytes() {
        let budget = Budget::new(12);
        let (source, publisher) =
            MemorySource::new(12, Generation(1), SourceKind::Paged, 4, 4, budget.clone()).unwrap();
        let publish = |page, bytes: &[u8]| {
            publisher
                .publish(
                    PageTicket {
                        generation: Generation(1),
                        page,
                    },
                    bytes,
                    Generation(1),
                )
                .unwrap()
        };
        assert!(matches!(source.read(0..4), SourceRead::Pending(_)));
        publish(0, b"old0");
        let SourceRead::Ready(owned) = source.read(0..4) else {
            panic!("page must be ready")
        };
        publish(1, b"old1");
        assert_eq!(owned.bytes(), b"old0");
        assert_eq!(budget.used(), 8);
        publisher.mark_changed();
        assert!(matches!(
            source.read(0..4),
            SourceRead::Unavailable(Unavailable::SourceChanged)
        ));
        assert!(matches!(source.read(4..8), SourceRead::Ready(_)));
        assert_eq!(
            publisher.publish(
                PageTicket {
                    generation: Generation(1),
                    page: 2
                },
                b"new2",
                Generation(2)
            ),
            Err(Error::StaleRevision)
        );
        drop(owned);
        assert_eq!(budget.used(), 4);
    }
    #[test]
    fn resident_seals_only_after_full_verified_fill() {
        let (source, publisher) =
            MemorySource::new(8, Generation(1), SourceKind::Resident, 4, 8, Budget::new(8))
                .unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(1),
                    page: 1,
                },
                b"tail",
                Generation(1),
            )
            .unwrap();
        assert_eq!(publisher.seal(Generation(1)), Err(Error::OutOfBounds));
        publisher.mark_changed();
        assert!(matches!(
            source.read(0..4),
            SourceRead::Unavailable(Unavailable::SourceChanged)
        ));
        let (source, publisher) =
            MemorySource::new(4, Generation(2), SourceKind::Resident, 4, 4, Budget::new(4))
                .unwrap();
        publisher
            .publish(
                PageTicket {
                    generation: Generation(2),
                    page: 0,
                },
                b"safe",
                Generation(2),
            )
            .unwrap();
        publisher.seal(Generation(2)).unwrap();
        publisher.mark_changed();
        assert!(source.sealed());
        assert!(matches!(source.read(0..4), SourceRead::Ready(_)));
    }
}
