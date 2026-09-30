// SPDX-License-Identifier: MPL-2.0
//! Cancellable sparse refinement. Each poll inspects one bounded text window,
//! finding terminators with `memchr` instead of testing every byte (PED-08).
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
/// Terminators in `bytes` and whether `bytes` ends with a CR. A CRLF counts once,
/// including one whose CR ended the previous window (`preceding_cr`); a lone CR
/// and a lone LF each end a line, as in the resident document.
pub(crate) fn count_breaks(bytes: &[u8], preceding_cr: bool) -> (usize, bool) {
    let mut breaks = 0;
    for found in memchr::memchr2_iter(b'\r', b'\n', bytes) {
        let second_of_crlf = bytes[found] == b'\n'
            && if found == 0 {
                preceding_cr
            } else {
                bytes[found - 1] == b'\r'
            };
        breaks += usize::from(!second_of_crlf);
    }
    (breaks, bytes.last().map_or(preceding_cr, |byte| *byte == b'\r'))
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
    /// No window crosses this offset, so a scan lands exactly on it (the index's
    /// first checkpoint shifted by an edit, whose line delta it then learns).
    stop: Option<usize>,
    scanned: usize,
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
        stop: Option<TextOffset>,
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
            stop: stop.map(|stop| stop.0).filter(|stop| *stop > checkpoint.offset.0),
            scanned: 0,
            budget,
            request: None,
            cancelled: false,
            finished: false,
        })
    }
    pub fn matches_snapshot(&self, snapshot: &PagedSnapshot) -> bool {
        self.snapshot.same_document(snapshot) && self.snapshot.content_state == snapshot.content_state
    }
    /// Last verified UTF-8 boundary; includes CR state across window boundaries.
    pub fn verified_checkpoint(&self) -> Option<LineCheckpoint> {
        (!self.cancelled).then_some(self.verified)
    }
    /// Text bytes this lookup has inspected so far.
    pub fn scanned_bytes(&self) -> usize {
        self.scanned
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
    /// Moves the cursor to a scalar boundary whose line state is current.
    fn settle(&mut self, at: usize) {
        self.cursor = at;
        self.verified = LineCheckpoint {
            offset: TextOffset(at),
            breaks: self.breaks,
            preceding_cr: self.preceding_cr,
        };
    }
    pub fn poll(&mut self) -> LineLookupPoll {
        if self.cancelled {
            return LineLookupPoll::Cancelled;
        }
        if self.finished {
            return LineLookupPoll::Finished;
        }
        let result = self.step();
        if !matches!(result, LineLookupPoll::Progress(_) | LineLookupPoll::Pending(_)) {
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
                LineTarget::Line(_) => self.start.map_or(LineLookupPoll::Failed(Error::OutOfBounds), |start| {
                    LineLookupPoll::Range(TextOffset(start)..TextOffset(self.cursor))
                }),
            };
        }
        if self.request.is_none() {
            let bytes = match self.stop {
                Some(stop) if stop > self.cursor => self.window_bytes.min(stop - self.cursor),
                _ => self.window_bytes,
            };
            match self
                .snapshot
                .begin_viewport(TextOffset(self.cursor), bytes, &self.budget)
            {
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
        let text = window.text().as_bytes();
        let base = self.cursor;
        match self.target {
            LineTarget::Byte(offset) if offset.0 < base + text.len() => {
                let at = offset.0 - base;
                self.scanned += at;
                if !window.text().is_char_boundary(at) {
                    return LineLookupPoll::Failed(Error::InvalidBoundary);
                }
                let (breaks, preceding_cr) = count_breaks(&text[..at], self.preceding_cr);
                self.breaks += breaks;
                self.preceding_cr = preceding_cr;
                self.settle(offset.0);
                // The LF of a CRLF belongs to the line its CR ended.
                return LineLookupPoll::Line(self.breaks - usize::from(self.preceding_cr && text[at] == b'\n'));
            }
            LineTarget::Byte(_) => {
                let (breaks, preceding_cr) = count_breaks(text, self.preceding_cr);
                self.breaks += breaks;
                self.preceding_cr = preceding_cr;
            }
            LineTarget::Line(_) => {
                if let Some(result) = self.scan_lines(text, base) {
                    return result;
                }
            }
        }
        // A ready window is valid UTF-8, so its end is a scalar boundary.
        self.scanned += text.len();
        self.settle(base + text.len());
        LineLookupPoll::Progress(TextOffset(self.cursor))
    }
    /// Visits each line start of one window for a line target, jumping between
    /// terminators. Returns the target's range once its end is known.
    fn scan_lines(&mut self, text: &[u8], base: usize) -> Option<LineLookupPoll> {
        let mut at = 0;
        while at < text.len() {
            if self.preceding_cr {
                if text[at] == b'\n' {
                    // The LF of a CRLF; its CR already counted the terminator.
                    self.preceding_cr = false;
                    at += 1;
                    if let Some(result) = self.boundary(base + at) {
                        self.scanned += at;
                        self.settle(base + at);
                        return Some(result);
                    }
                    continue;
                }
                // A lone CR ended the previous line here.
                if let Some(result) = self.boundary(base + at) {
                    self.scanned += at;
                    self.settle(base + at);
                    return Some(result);
                }
                self.preceding_cr = false;
            }
            let Some(found) = memchr::memchr2(b'\r', b'\n', &text[at..]) else {
                break;
            };
            let found = at + found;
            self.breaks += 1;
            at = found + 1;
            if text[found] == b'\r' {
                self.preceding_cr = true;
            } else if let Some(result) = self.boundary(base + at) {
                self.scanned += at;
                self.settle(base + at);
                return Some(result);
            }
        }
        None
    }
}
