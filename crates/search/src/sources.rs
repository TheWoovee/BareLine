// SPDX-License-Identifier: MPL-2.0
//! Worker-side grouped searches over already-open stable document snapshots.
use super::*;

pub const MAX_OPEN_DOCUMENTS: usize = 4096;

pub struct OpenDocumentResults {
    documents: Vec<SearchResults>,
    completeness: Completeness,
}
impl OpenDocumentResults {
    pub fn documents(&self) -> &[SearchResults] {
        &self.documents
    }
    /// Exact only when Complete; an incomplete collection is never a replace plan.
    pub fn count(&self) -> usize {
        self.documents.iter().map(SearchResults::count).sum()
    }
    pub fn completeness(&self) -> Completeness {
        self.completeness
    }
    pub fn retained_bytes(&self) -> usize {
        self.documents
            .iter()
            .map(SearchResults::retained_bytes)
            .sum()
    }
}
/// Searches input order, retaining source identity/revision per group. The iterator must
/// yield snapshots without blocking I/O. Result storage shares one hard aggregate budget;
/// consumers retaining callback copies must budget those independently. No edits are made.
/// Selection bounds, if supplied, apply to each source; use None for Find in Open Documents.
pub fn scan_open_documents(
    documents: impl IntoIterator<Item = DocumentSnapshot>,
    query: &SearchQuery,
    job: &SearchJob,
    mut emit: impl FnMut(SearchBatch<'_>),
) -> OpenDocumentResults {
    let mut output = OpenDocumentResults {
        documents: Vec::new(),
        completeness: Completeness::Complete,
    };
    let mut remaining = query.results_ram_bytes.min(MAX_RESULT_BYTES);
    for snapshot in documents {
        if job.is_cancelled() {
            output.completeness = Completeness::Cancelled;
            break;
        }
        let overhead = std::mem::size_of::<SearchResults>();
        if output.documents.len() == MAX_OPEN_DOCUMENTS || remaining < overhead {
            output.completeness = Completeness::ResultLimit;
            break;
        }
        let mut scoped = query.clone();
        scoped.results_ram_bytes = remaining - overhead;
        let result = scan(&snapshot, &scoped, job, &mut emit);
        remaining = remaining.saturating_sub(result.retained_bytes());
        let status = result.completeness();
        output.documents.reserve_exact(1);
        output.documents.push(result);
        if status != Completeness::Complete {
            output.completeness = status;
            break;
        }
    }
    if job.is_cancelled() {
        output.completeness = Completeness::Cancelled;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, DocumentBuilder};
    fn snapshot(text: &str) -> DocumentSnapshot {
        Document::from_utf8(text, Budget::new(1024 * 1024), Budget::new(1024 * 1024))
            .unwrap()
            .snapshot()
    }
    #[test]
    fn grouped_identity_and_aggregate_budget_are_preserved() {
        let first = snapshot("a a");
        let second = snapshot("a");
        let mut q = SearchQuery::literal("a");
        let job = SearchJob::default();
        let results = scan_open_documents([first.clone(), second.clone()], &q, &job, |_| {});
        assert_eq!(results.count(), 3);
        assert_eq!(results.completeness(), Completeness::Complete);
        assert!(results.documents()[0].source().same_document(&first));
        assert!(results.documents()[1].source().same_document(&second));
        q.results_ram_bytes =
            std::mem::size_of::<SearchResults>() + 2 * std::mem::size_of::<SearchMatch>();
        let results = scan_open_documents([first, second], &q, &job, |_| {});
        assert_eq!(results.count(), 2);
        assert_eq!(results.completeness(), Completeness::ResultLimit);
        assert!(results.retained_bytes() <= q.results_ram_bytes);
    }
    #[test]
    fn cancellation_and_partial_prefixes_cannot_be_complete() {
        let job = SearchJob::default();
        let results = scan_open_documents(
            [snapshot("a"), snapshot("a")],
            &SearchQuery::literal("a"),
            &job,
            |_| job.cancel(),
        );
        assert_eq!(results.completeness(), Completeness::Cancelled);
        assert_eq!(results.documents().len(), 1);
        let mut builder = DocumentBuilder::new(Budget::new(4096), Budget::new(4096)).unwrap();
        builder.append("a").unwrap();
        let prefix = builder.prefix();
        let result = scan(
            &prefix,
            &SearchQuery::literal("a"),
            &SearchJob::default(),
            |_| {},
        );
        assert_eq!(result.completeness(), Completeness::Unsupported);
        assert!(matches!(
            result.prepare_replace(&prefix, "b", 4096),
            Err(ReplaceError::Incomplete)
        ));
    }
}
