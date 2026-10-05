// SPDX-License-Identifier: MPL-2.0
//! Bounded log of committed change receipts shared by the views of one document.
//!
//! A view that falls several revisions behind its peer remaps its selection and
//! anchors through every change in order instead of clamping them to the new
//! length (EDT-10, PED-11, PED-13). Only a trimmed or broken log falls back.
use bareline_document::{ContentStateId, change::AppliedChange};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

/// Receipts kept. Linked views refresh on their next pump, so a lagging view
/// rarely needs more than a few; one undo of a long macro run needs one per
/// entry. A single-edit receipt is about 150 bytes.
const MAX_CHANGES: usize = 1024;
/// Receipt bytes kept (receipts hold document budget); the newest receipt is
/// kept even when it alone is larger, since the current snapshot holds it anyway.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// A document identity token with its content state.
pub(crate) type LogPoint = ((u64, u64), ContentStateId);

#[derive(Default)]
pub(crate) struct ChangeLog {
    changes: VecDeque<Arc<AppliedChange>>,
    bytes: usize,
}
impl ChangeLog {
    /// Appends a committed receipt. A receipt that does not follow the newest
    /// one starts a new chain, so a lookup never steps over an unseen change.
    pub(crate) fn record(&mut self, change: &Arc<AppliedChange>) {
        if let Some(last) = self.changes.back() {
            // Every view that installs a receipt reports it; keep the first.
            if last.document_id == change.document_id && change.after_revision.0 <= last.after_revision.0 {
                return;
            }
            if last.document_id != change.document_id
                || last.after_revision != change.before_revision
                || last.after_state != change.before_state
            {
                self.clear();
            }
        }
        self.bytes = self.bytes.saturating_add(change.charged_bytes());
        self.changes.push_back(change.clone());
        while self.changes.len() > MAX_CHANGES || (self.changes.len() > 1 && self.bytes > MAX_BYTES) {
            if let Some(oldest) = self.changes.pop_front() {
                self.bytes = self.bytes.saturating_sub(oldest.charged_bytes());
            }
        }
    }
    pub(crate) fn clear(&mut self) {
        self.changes.clear();
        self.bytes = 0;
    }
    /// The receipts leading from `from` to `to`, oldest first. `None` when the
    /// log no longer holds that whole chain; an empty chain when they match.
    pub(crate) fn chain(&self, from: LogPoint, to: LogPoint) -> Option<Vec<Arc<AppliedChange>>> {
        if from == to {
            return Some(Vec::new());
        }
        let first = self
            .changes
            .iter()
            .position(|change| change.matches_before(from.0, from.1))?;
        let mut chain = Vec::new();
        for change in self.changes.iter().skip(first) {
            chain.push(change.clone());
            if (change.document_id, change.after_revision.0) == to.0 {
                return (change.after_state == to.1).then_some(chain);
            }
        }
        None
    }
}
/// The log shared by all views of one document.
pub(crate) type SharedChangeLog = Arc<Mutex<ChangeLog>>;
pub(crate) fn lock(log: &SharedChangeLog) -> MutexGuard<'_, ChangeLog> {
    log.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, Edit, EditTransaction, TextOffset};
    fn insert(document: &mut Document, at: usize, text: &str) -> Arc<AppliedChange> {
        let base_revision = document.snapshot().revision;
        document
            .apply(EditTransaction {
                base_revision,
                edits: vec![Edit {
                    range: TextOffset(at)..TextOffset(at),
                    insert: text.into(),
                }],
            })
            .unwrap();
        document.snapshot().applied_change().unwrap().clone()
    }
    fn point(document: &Document) -> LogPoint {
        let snapshot = document.snapshot();
        (snapshot.identity_token(), snapshot.content_state)
    }
    #[test]
    fn chains_every_change_and_ignores_repeated_reports() {
        let mut document = Document::from_utf8("abc", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let start = point(&document);
        let mut log = ChangeLog::default();
        let first = insert(&mut document, 0, "x");
        log.record(&first);
        log.record(&first);
        let middle = point(&document);
        log.record(&insert(&mut document, 1, "y"));
        let end = point(&document);
        let chain = log.chain(start, end).unwrap();
        assert_eq!(chain.len(), 2);
        assert!(Arc::ptr_eq(&chain[0], &first));
        assert_eq!(log.chain(middle, end).unwrap().len(), 1);
        assert!(log.chain(end, end).unwrap().is_empty());
        // Nothing leads back to an older state.
        assert!(log.chain(end, start).is_none());
    }
    #[test]
    fn an_unseen_change_breaks_the_chain_instead_of_being_skipped() {
        let mut document = Document::from_utf8("abc", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let start = point(&document);
        let mut log = ChangeLog::default();
        log.record(&insert(&mut document, 0, "x"));
        // This change is never reported.
        insert(&mut document, 0, "y");
        let unseen = point(&document);
        log.record(&insert(&mut document, 0, "z"));
        let end = point(&document);
        assert!(log.chain(start, end).is_none());
        assert_eq!(log.chain(unseen, end).unwrap().len(), 1);
    }
    #[test]
    fn a_trimmed_log_reports_no_chain_and_stays_bounded() {
        let mut document = Document::from_utf8("", Budget::new(16 << 20), Budget::new(16 << 20)).unwrap();
        let start = point(&document);
        let mut log = ChangeLog::default();
        for _ in 0..MAX_CHANGES + 1 {
            log.record(&insert(&mut document, 0, "x"));
        }
        let end = point(&document);
        assert_eq!(log.changes.len(), MAX_CHANGES);
        assert!(log.bytes <= MAX_BYTES);
        assert!(log.chain(start, end).is_none());
        let first = log.changes[0].clone();
        let from = ((first.document_id, first.before_revision.0), first.before_state);
        assert_eq!(log.chain(from, end).unwrap().len(), MAX_CHANGES);
    }
}
