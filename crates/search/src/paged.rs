// SPDX-License-Identifier: MPL-2.0
//! Full-source bounded paged literal search. Page fulfillment runs on the caller's worker.
use super::*;
use bareline_document::{
    Budget, Document,
    paged::{PagedSnapshot, TextWindow, WindowPoll},
    source::PageTicket,
};
const WINDOW: usize = 1024 * 1024;
pub struct PagedResults {
    pub source: PagedSnapshot,
    pub query: SearchQuery,
    pub job: SearchJobId,
    pub matches: Vec<SearchMatch>,
    pub completeness: Completeness,
    pub count: usize,
    pub count_complete: bool,
}
impl PagedResults {
    pub fn prepare_replace(
        &self,
        current: &PagedSnapshot,
        replacement: &str,
        scope: ReplaceScope,
        job: &SearchJob,
        mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    ) -> Result<EditTransaction, ReplaceError> {
        if self.completeness != Completeness::Complete {
            return Err(ReplaceError::Incomplete);
        }
        if !self.source.same_document(current)
            || self.source.revision != current.revision
            || self.source.content_state != current.content_state
        {
            return Err(ReplaceError::Stale);
        }
        if job.is_cancelled() {
            return Err(ReplaceError::Cancelled);
        }
        let template = decode_replacement(replacement, self.query.mode)?;
        if self.query.mode == SearchMode::Regex {
            let mut subject = String::with_capacity(current.len());
            while subject.len() < current.len() {
                let part =
                    window(current, subject.len(), WINDOW, job, &mut resolve).map_err(|_| {
                        if job.is_cancelled() {
                            ReplaceError::Cancelled
                        } else {
                            ReplaceError::Stale
                        }
                    })?;
                if part.text().is_empty() {
                    return Err(ReplaceError::Stale);
                }
                subject.push_str(part.text());
            }
            let document = Document::from_utf8(
                &subject,
                Budget::new(regex::CONTEXT_LIMIT * 2),
                Budget::new(1),
            )
            .map_err(|_| ReplaceError::StagingLimit)?;
            drop(subject);
            let snapshot = document.snapshot();
            let found = scan(&snapshot, &self.query, job, |_| {});
            if found.matches() != self.matches {
                return Err(ReplaceError::Stale);
            }
            let mut transaction =
                found.prepare_replace_scoped(&snapshot, &template, MAX_RESULT_BYTES, scope, job)?;
            transaction.base_revision = current.revision;
            return Ok(transaction);
        }
        let mut edits = Vec::new();
        let mut used = 0usize;
        for found in &self.matches {
            if let ReplaceScope::One(range) = &scope
                && range != &found.range
            {
                continue;
            }
            if job.is_cancelled() {
                return Err(ReplaceError::Cancelled);
            }
            used = used
                .checked_add(found.range.end.0 - found.range.start.0)
                .and_then(|bytes| bytes.checked_add(template.len() + std::mem::size_of::<Edit>()))
                .ok_or(ReplaceError::StagingLimit)?;
            if used > MAX_RESULT_BYTES {
                return Err(ReplaceError::StagingLimit);
            }
            edits.push(Edit {
                range: found.range.clone(),
                insert: template.clone(),
            });
        }
        if edits.is_empty() {
            return Err(ReplaceError::NoMatch);
        }
        Ok(EditTransaction {
            base_revision: current.revision,
            edits,
        })
    }
}
fn window(
    snapshot: &PagedSnapshot,
    start: usize,
    size: usize,
    job: &SearchJob,
    resolve: &mut impl FnMut(PageTicket) -> Result<bool, String>,
) -> Result<TextWindow, Completeness> {
    let mut request = snapshot
        .begin_viewport(TextOffset(start), size, &Budget::new(size))
        .map_err(|_| Completeness::Unsupported)?;
    loop {
        if job.is_cancelled() {
            return Err(Completeness::Cancelled);
        }
        match request.poll() {
            WindowPoll::Ready(value) => return Ok(value),
            WindowPoll::Pending(ticket) => {
                if !resolve(ticket).map_err(|_| Completeness::Unsupported)? {
                    std::thread::yield_now();
                }
            }
            _ => return Err(Completeness::Unsupported),
        }
    }
}
pub fn excerpt(
    snapshot: &PagedSnapshot,
    at: TextOffset,
    job: &SearchJob,
    mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
) -> Result<String, Completeness> {
    window(snapshot, at.0, 160, job, &mut resolve).map(|window| window.text().into())
}
/// No editor-surface dependency: callers provide their generation-checked page resolver.
/// Regex requires a complete bounded subject; larger contexts explicitly remain incomplete.
pub fn scan_paged(
    snapshot: &PagedSnapshot,
    query: &SearchQuery,
    job: &SearchJob,
    mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    mut emit: impl FnMut(&[SearchMatch]),
) -> PagedResults {
    let mut result = PagedResults {
        source: snapshot.clone(),
        query: query.clone(),
        job: job.id,
        matches: Vec::new(),
        completeness: Completeness::Complete,
        count: 0,
        count_complete: false,
    };
    let status = (|| {
        let selection = query
            .selection
            .clone()
            .unwrap_or(TextOffset(0)..TextOffset(snapshot.len()));
        if selection.start > selection.end || selection.end.0 > snapshot.len() {
            return Err(Completeness::InvalidQuery);
        }
        if query.mode == SearchMode::Regex && snapshot.len() > regex::CONTEXT_LIMIT {
            return Err(Completeness::UnsupportedStreaming);
        }
        if query.mode == SearchMode::Regex {
            let mut subject = String::with_capacity(snapshot.len());
            while subject.len() < snapshot.len() {
                let part = window(snapshot, subject.len(), WINDOW, job, &mut resolve)?;
                if part.text().is_empty() {
                    return Err(Completeness::Unsupported);
                }
                subject.push_str(part.text());
            }
            let document = Document::from_utf8(
                &subject,
                Budget::new(regex::CONTEXT_LIMIT * 2),
                Budget::new(1),
            )
            .map_err(|_| Completeness::Unsupported)?;
            drop(subject);
            let found = scan(&document.snapshot(), query, job, |_| {});
            result.count = found.count();
            result.count_complete = found.count_complete();
            result.completeness = found.completeness();
            result.matches = found.matches().to_vec();
            for batch in result.matches.chunks(BATCH_SIZE) {
                emit(batch);
            }
            return Ok(());
        }
        let context = if query.mode == SearchMode::Regex {
            snapshot.len()
        } else {
            query.pattern.len().saturating_mul(12).saturating_add(8)
        };
        if context > regex::CONTEXT_LIMIT {
            return Err(Completeness::InvalidQuery);
        }
        let mut next = selection.start.0;
        let capacity =
            query.results_ram_bytes.min(MAX_RESULT_BYTES) / std::mem::size_of::<SearchMatch>();
        while next <= selection.end.0 {
            if job.is_cancelled() {
                return Err(Completeness::Cancelled);
            }
            let start = if query.mode == SearchMode::Regex {
                0
            } else {
                next.saturating_sub(4)
            };
            let size = if query.mode == SearchMode::Regex {
                snapshot.len()
            } else {
                WINDOW + context
            };
            let text = window(snapshot, start, size, job, &mut resolve)?;
            let base = text.range().start.0;
            let end = text.range().end.0;
            if end < next || (end == next && end < snapshot.len()) {
                return Err(Completeness::Unsupported);
            }
            let document = Document::from_utf8(
                text.text(),
                Budget::new(size.saturating_mul(2).max(1)),
                Budget::new(1),
            )
            .map_err(|_| Completeness::Unsupported)?;
            let mut local = query.clone();
            local.selection =
                Some(TextOffset(next - base)..TextOffset(selection.end.0.min(end) - base));
            local.results_ram_bytes = MAX_RESULT_BYTES;
            local.count_beyond_limit = false;
            let matches = scan(&document.snapshot(), &local, job, |_| {});
            if matches.completeness() != Completeness::Complete {
                return Err(matches.completeness());
            }
            let mut cutoff = if end >= selection.end.0 {
                end
            } else {
                end.saturating_sub(context)
            };
            while cutoff < end && !text.text().is_char_boundary(cutoff - base) {
                cutoff += 1;
            }
            let mut advance = next;
            for found in matches.matches() {
                let range =
                    TextOffset(found.range.start.0 + base)..TextOffset(found.range.end.0 + base);
                if range.start.0 >= cutoff && end != snapshot.len() {
                    break;
                }
                result.count += 1;
                advance = range.end.0;
                if result.matches.len() < capacity {
                    result.matches.push(SearchMatch { range });
                    emit(&result.matches[result.matches.len() - 1..]);
                } else {
                    result.completeness = Completeness::ResultLimit;
                    if !query.count_beyond_limit {
                        result.count -= 1;
                        return Err(Completeness::ResultLimit);
                    }
                }
            }
            if end >= selection.end.0 || query.mode == SearchMode::Regex {
                break;
            }
            next = advance.max(cutoff);
        }
        result.count_complete = true;
        Ok(())
    })();
    if let Err(error) = status {
        result.completeness = error;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::source::{Generation, MemorySource, SourceKind};
    #[test]
    fn paged_folded_match_crosses_window_with_one_page_cache_and_absolute_selection() {
        let mut text = "x".repeat(WINDOW - 2);
        text.push_str("Straße!");
        text.push_str(&"x".repeat(WINDOW));
        let generation = Generation(987);
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
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let mut query = SearchQuery::literal("STRASSE");
        query.case = Case::Folded;
        let result = scan_paged(
            &snapshot,
            &query,
            &SearchJob::default(),
            |ticket| {
                let start = ticket.page as usize * page_size;
                publisher
                    .publish(
                        ticket,
                        &text.as_bytes()[start..(start + page_size).min(text.len())],
                        generation,
                    )
                    .map_err(|error| format!("{error:?}"))?;
                Ok(true)
            },
            |_| {},
        );
        assert_eq!(result.completeness, Completeness::Complete);
        assert_eq!(
            result.matches[0].range,
            TextOffset(WINDOW - 2)..TextOffset(WINDOW + 5)
        );
        assert_eq!(result.count, 1);
        assert!(result.count_complete);
    }
}
