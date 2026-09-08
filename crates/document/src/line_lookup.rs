// SPDX-License-Identifier: MPL-2.0
//! Cancellable sparse refinement. Each poll inspects one bounded text window.
use crate::{
    Budget, Error, TextOffset,
    paged::{LineCheckpoint, PagedSnapshot, WindowPoll, WindowRequest},
    source::{PageTicket, Unavailable},
};
use std::ops::Range;
#[derive(Clone, Copy, Debug)]
pub enum LineTarget {
    Byte(TextOffset),
    Line(usize),
}
#[derive(Debug)]
pub enum LineLookupPoll {
    Line(usize),
    Range(Range<TextOffset>),
    Pending(PageTicket),
    Progress(TextOffset),
    Unavailable(Unavailable),
    Failed(Error),
    Cancelled,
    Finished,
}
pub struct LineLookupRequest {
    snapshot: PagedSnapshot,
    verified: LineCheckpoint,
    target: LineTarget,
    cursor: usize,
    breaks: usize,
    preceding_cr: bool,
    start: Option<usize>,
    window_bytes: usize,
    budget: Budget,
    request: Option<WindowRequest>,
    cancelled: bool,
    finished: bool,
}
impl LineLookupRequest {
    pub(crate) fn new(
        snapshot: PagedSnapshot,
        checkpoint: LineCheckpoint,
        target: LineTarget,
        window_bytes: usize,
        budget: Budget,
    ) -> Result<Self, Error> {
        if window_bytes < 4 {
            return Err(Error::BudgetExceeded);
        }
        if let LineTarget::Byte(offset) = target {
            if offset.0 > snapshot.len() {
                return Err(Error::OutOfBounds);
            }
        }
        Ok(Self {
            snapshot,
            verified: checkpoint,
            target,
            cursor: checkpoint.offset.0,
            breaks: checkpoint.breaks,
            preceding_cr: checkpoint.preceding_cr,
            start: matches!(target, LineTarget::Line(0)).then_some(0),
            window_bytes,
            budget,
            request: None,
            cancelled: false,
            finished: false,
        })
    }
    pub fn matches_snapshot(&self, snapshot: &PagedSnapshot) -> bool {
        self.snapshot.same_document(snapshot)
            && self.snapshot.content_state == snapshot.content_state
    }
    /// Last verified UTF-8 boundary; includes CR state across window boundaries.
    pub fn verified_checkpoint(&self) -> Option<LineCheckpoint> {
        (!self.cancelled).then_some(self.verified)
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.request = None;
    }
    fn boundary(&mut self, at: usize) -> Option<LineLookupPoll> {
        if let LineTarget::Line(line) = self.target {
            if self.breaks == line {
                self.start = Some(at);
            } else if self.breaks > line
                && let Some(start) = self.start
            {
                return Some(LineLookupPoll::Range(TextOffset(start)..TextOffset(at)));
            }
        }
        None
    }
    pub fn poll(&mut self) -> LineLookupPoll {
        if self.cancelled {
            return LineLookupPoll::Cancelled;
        }
        if self.finished {
            return LineLookupPoll::Finished;
        }
        let result = self.step();
        if !matches!(
            result,
            LineLookupPoll::Progress(_) | LineLookupPoll::Pending(_)
        ) {
            self.finished = true;
            self.request = None;
        }
        result
    }
    fn step(&mut self) -> LineLookupPoll {
        if self.cursor == self.snapshot.len() {
            if self.preceding_cr
                && let Some(result) = self.boundary(self.cursor)
            {
                return result;
            }
            return match self.target {
                LineTarget::Byte(_) => LineLookupPoll::Line(self.breaks),
                LineTarget::Line(_) => self
                    .start
                    .map_or(LineLookupPoll::Failed(Error::OutOfBounds), |start| {
                        LineLookupPoll::Range(TextOffset(start)..TextOffset(self.cursor))
                    }),
            };
        }
        if self.request.is_none() {
            match self.snapshot.begin_viewport(
                TextOffset(self.cursor),
                self.window_bytes,
                &self.budget,
            ) {
                Ok(request) => self.request = Some(request),
                Err(error) => return LineLookupPoll::Failed(error),
            }
        }
        let window = match self.request.as_mut().expect("active lookup").poll() {
            WindowPoll::Ready(window) => window,
            WindowPoll::Pending(ticket) => return LineLookupPoll::Pending(ticket),
            WindowPoll::Unavailable(reason) => return LineLookupPoll::Unavailable(reason),
            WindowPoll::InvalidUtf8 => return LineLookupPoll::Failed(Error::InvalidBoundary),
            WindowPoll::Finished => return LineLookupPoll::Failed(Error::IncompleteSource),
        };
        self.request = None;
        if window.range().start.0 != self.cursor || window.text().is_empty() {
            return LineLookupPoll::Failed(Error::InvalidBoundary);
        }
        for (local, byte) in window.text().bytes().enumerate() {
            if let LineTarget::Byte(offset) = self.target
                && offset.0 == self.cursor
            {
                if !window.text().is_char_boundary(local) {
                    return LineLookupPoll::Failed(Error::InvalidBoundary);
                }
                return LineLookupPoll::Line(
                    self.breaks - usize::from(self.preceding_cr && byte == b'\n'),
                );
            }
            if self.preceding_cr
                && byte != b'\n'
                && let Some(result) = self.boundary(self.cursor)
            {
                return result;
            }
            if byte == b'\r' {
                self.breaks += 1;
            } else if byte == b'\n' {
                if !self.preceding_cr {
                    self.breaks += 1;
                }
                if let Some(result) = self.boundary(self.cursor + 1) {
                    return result;
                }
            }
            self.preceding_cr = byte == b'\r';
            self.cursor += 1;
            if window.text().is_char_boundary(local + 1) {
                self.verified = LineCheckpoint {
                    offset: TextOffset(self.cursor),
                    breaks: self.breaks,
                    preceding_cr: self.preceding_cr,
                };
            }
        }
        LineLookupPoll::Progress(TextOffset(self.cursor))
    }
}
