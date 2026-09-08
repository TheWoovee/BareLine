// SPDX-License-Identifier: MPL-2.0
//! Worker-side grouped searches over already-open stable document snapshots.
use super::*;

pub const MAX_OPEN_DOCUMENTS: usize = 4096;

pub struct PagedOpenDocument {
    pub snapshot: bareline_document::paged::PagedSnapshot,
    pub label: String,
    pub resolve:
        Box<dyn FnMut(bareline_document::source::PageTicket) -> Result<bool, String> + Send>,
}
pub struct PagedOpenResults {
    pub results: super::paged::PagedResults,
    pub label: String,
    pub excerpts: Vec<String>,
}
pub struct OpenDocumentResults {
    pub paged: Vec<PagedOpenResults>,
    documents: Vec<SearchResults>,
    completeness: Completeness,
}
impl OpenDocumentResults {
    pub fn documents(&self) -> &[SearchResults] {
        &self.documents
    }
    /// Exact only when Complete; an incomplete collection is never a replace plan.
    pub fn count(&self) -> usize {
        self.documents
            .iter()
            .map(SearchResults::count)
            .sum::<usize>()
            + self
                .paged
                .iter()
                .map(|group| group.results.count)
                .sum::<usize>()
    }
    pub fn completeness(&self) -> Completeness {
        self.completeness
    }
    pub fn retained_bytes(&self) -> usize {
        self.documents
            .iter()
            .map(SearchResults::retained_bytes)
            .sum::<usize>()
            + self
                .paged
                .iter()
                .map(|group| {
                    std::mem::size_of::<PagedOpenResults>()
                        + group.label.len()
                        + group.results.matches.capacity() * std::mem::size_of::<SearchMatch>()
                        + group
                            .excerpts
                            .iter()
                            .map(|value| value.len() + std::mem::size_of::<String>())
                            .sum::<usize>()
                })
                .sum::<usize>()
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
        paged: Vec::new(),
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

/// Both storage modes are scanned on the same worker with a shared result budget.
pub fn scan_mixed_open_documents(
    resident: Vec<DocumentSnapshot>,
    paged: Vec<PagedOpenDocument>,
    query: &SearchQuery,
    job: &SearchJob,
) -> OpenDocumentResults {
    let mut output = scan_open_documents(resident, query, job, |_| {});
    if output.completeness != Completeness::Complete {
        return output;
    }
    let mut remaining = query
        .results_ram_bytes
        .min(MAX_RESULT_BYTES)
        .saturating_sub(output.retained_bytes());
    for mut source in paged {
        if job.is_cancelled() {
            output.completeness = Completeness::Cancelled;
            break;
        }
        let overhead = std::mem::size_of::<PagedOpenResults>() + source.label.len();
        if output.documents.len() + output.paged.len() >= MAX_OPEN_DOCUMENTS || remaining < overhead
        {
            output.completeness = Completeness::ResultLimit;
            break;
        }
        remaining -= overhead;
        let mut scoped = query.clone();
        scoped.results_ram_bytes = remaining / 2;
        let mut results =
            super::paged::scan_paged(&source.snapshot, &scoped, job, &mut source.resolve, |_| {});
        remaining = remaining
            .saturating_sub(results.matches.capacity() * std::mem::size_of::<SearchMatch>());
        let mut excerpts = Vec::new();
        for found in &results.matches {
            match super::paged::excerpt(
                &source.snapshot,
                found.range.start,
                job,
                &mut source.resolve,
            ) {
                Ok(value) if value.len() + std::mem::size_of::<String>() <= remaining => {
                    remaining -= value.len() + std::mem::size_of::<String>();
                    excerpts.reserve_exact(1);
                    excerpts.push(value);
                }
                Ok(_) => {
                    results.completeness = Completeness::ResultLimit;
                    break;
                }
                Err(error) => {
                    results.completeness = error;
                    break;
                }
            }
        }
        results.matches.truncate(excerpts.len());
        let status = results.completeness;
        output.paged.reserve_exact(1);
        output.paged.push(PagedOpenResults {
            results,
            label: source.label,
            excerpts,
        });
        if status != Completeness::Complete {
            output.completeness = status;
            break;
        }
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
    fn mixed_results_scan_beyond_viewport_with_global_offsets_and_cancellation() {
        use bareline_document::source::{Generation, MemorySource, SourceKind};
        let text = format!("{}needle", "x".repeat(2 * 1024 * 1024));
        let generation = Generation(654);
        let page_size = 65536;
        let (source, publisher) = MemorySource::new(
            text.len() as u64,
            generation,
            SourceKind::Paged,
            page_size,
            page_size,
            Budget::new(page_size * 2),
        )
        .unwrap();
        let paged = bareline_document::paged::PagedSnapshot::utf8(source, 0).unwrap();
        let captured = paged.clone();
        let input = PagedOpenDocument {
            snapshot: paged,
            label: "large.txt".into(),
            resolve: Box::new(move |ticket| {
                let start = ticket.page as usize * page_size;
                publisher
                    .publish(
                        ticket,
                        &text.as_bytes()[start..(start + page_size).min(text.len())],
                        generation,
                    )
                    .map_err(|error| format!("{error:?}"))?;
                Ok(true)
            }),
        };
        let query = SearchQuery::literal("needle");
        let output = scan_mixed_open_documents(
            vec![snapshot("needle")],
            vec![input],
            &query,
            &SearchJob::default(),
        );
        assert_eq!(output.completeness(), Completeness::Complete);
        assert_eq!(output.count(), 2);
        assert!(output.paged[0].results.source.same_document(&captured));
        assert_eq!(
            output.paged[0].results.matches[0].range.start,
            TextOffset(2 * 1024 * 1024)
        );
        assert_eq!(output.paged[0].excerpts[0], "needle");
        assert!(output.retained_bytes() <= query.results_ram_bytes);
        let job = SearchJob::default();
        job.cancel();
        let cancelled =
            scan_mixed_open_documents(vec![snapshot("needle")], Vec::new(), &query, &job);
        assert_eq!(cancelled.completeness(), Completeness::Cancelled);
        assert_eq!(cancelled.count(), 0);
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
