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
    pub fn preserve_case(
        &self,
        transaction: &mut EditTransaction,
        job: &SearchJob,
        mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    ) -> Result<(), ReplaceError> {
        let mut used = 0usize;
        for edit in &mut transaction.edits {
            let length = edit.range.end.0 - edit.range.start.0;
            if length > MAX_RESULT_BYTES {
                return Err(ReplaceError::StagingLimit);
            }
            let original = window(&self.source, edit.range.start.0, length, job, &mut resolve).map_err(|_| {
                if job.is_cancelled() {
                    ReplaceError::Cancelled
                } else {
                    ReplaceError::Stale
                }
            })?;
            edit.insert = preserve_replacement_case(original.text(), &edit.insert);
            used = used.saturating_add(length).saturating_add(edit.insert.len());
            if used > MAX_RESULT_BYTES {
                return Err(ReplaceError::StagingLimit);
            }
        }
        Ok(())
    }
    pub fn prepare_replace(
        &self,
        current: &PagedSnapshot,
        replacement: &str,
        scope: ReplaceScope,
        job: &SearchJob,
        resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    ) -> Result<EditTransaction, ReplaceError> {
        self.prepare_replace_internal(current, replacement, scope, job, resolve, true)
    }
    /// Exact reviewed edits for disk-backed inverse staging; payloads remain bounded.
    pub fn prepare_replace_streaming(
        &self,
        current: &PagedSnapshot,
        replacement: &str,
        scope: ReplaceScope,
        job: &SearchJob,
        resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    ) -> Result<EditTransaction, ReplaceError> {
        self.prepare_replace_internal(current, replacement, scope, job, resolve, false)
    }
    fn prepare_replace_internal(
        &self,
        current: &PagedSnapshot,
        replacement: &str,
        scope: ReplaceScope,
        job: &SearchJob,
        mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
        include_inverse: bool,
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
        if self.query.mode == SearchMode::Regex && regex::streamable(&self.query) {
            let names = regex::capture_names(&self.query).map_err(|_| ReplaceError::Incomplete)?;
            let resolver = std::cell::RefCell::new(&mut resolve);
            let mut matched = 0usize;
            let mut used = 0usize;
            let mut edits = Vec::new();
            let mut replacement_error = None;
            let selection = self
                .query
                .selection
                .clone()
                .unwrap_or(TextOffset(0)..TextOffset(current.len()));
            let outcome = regex::scan_stream(
                current.len(),
                &self.query,
                job,
                |start, size| {
                    let part = window(current, start, size, job, &mut **resolver.borrow_mut())?;
                    if part.range().start.0 != start {
                        return Err(Completeness::Unsupported);
                    }
                    Ok(part.text().into())
                },
                |captures| {
                    let prepare = (|| -> Result<(), ReplaceError> {
                        let range = captures.first().and_then(Clone::clone).ok_or(ReplaceError::Stale)?;
                        if range.start < selection.start || range.end > selection.end {
                            return Ok(());
                        }
                        if self.matches.get(matched).map(|found| &found.range) != Some(&range) {
                            return Err(ReplaceError::Stale);
                        }
                        matched += 1;
                        if matches!(&scope, ReplaceScope::One(selected) if selected != &range) {
                            return Ok(());
                        }
                        if job.is_cancelled() {
                            return Err(ReplaceError::Cancelled);
                        }
                        used = used
                            .checked_add(std::mem::size_of::<Edit>())
                            .and_then(|value| {
                                value.checked_add(if include_inverse {
                                    range.end.0 - range.start.0
                                } else {
                                    0
                                })
                            })
                            .ok_or(ReplaceError::StagingLimit)?;
                        let remaining = MAX_RESULT_BYTES.checked_sub(used).ok_or(ReplaceError::StagingLimit)?;
                        let insert = regex::expand_ranges(&template, &captures, &names, remaining, |range, limit| {
                            read_capture_range(current, range, limit, job, &mut **resolver.borrow_mut())
                        })?;
                        used += insert.len();
                        edits.reserve_exact(1);
                        edits.push(Edit { range, insert });
                        Ok(())
                    })();
                    if let Err(error) = prepare {
                        replacement_error = Some(error);
                        return Err(Completeness::Unsupported);
                    }
                    Ok(())
                },
            );
            if let Some(error) = replacement_error {
                return Err(error);
            }
            if job.is_cancelled() {
                return Err(ReplaceError::Cancelled);
            }
            outcome.map_err(|_| ReplaceError::Incomplete)?;
            if matched != self.matches.len() {
                return Err(ReplaceError::Stale);
            }
            if edits.is_empty() {
                return Err(ReplaceError::NoMatch);
            }
            return Ok(EditTransaction {
                base_revision: current.revision,
                edits,
            });
        }
        if self.query.mode == SearchMode::Regex {
            if current.len() > regex::CONTEXT_LIMIT {
                return Err(ReplaceError::Incomplete);
            }
            let mut subject = String::with_capacity(current.len());
            while subject.len() < current.len() {
                let part = window(current, subject.len(), WINDOW, job, &mut resolve).map_err(|_| {
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
            let document = Document::from_utf8(&subject, Budget::new(regex::CONTEXT_LIMIT * 2), Budget::new(1))
                .map_err(|_| ReplaceError::StagingLimit)?;
            drop(subject);
            let snapshot = document.snapshot();
            let found = scan(&snapshot, &self.query, job, |_| {});
            if found.matches() != self.matches {
                return Err(ReplaceError::Stale);
            }
            let mut transaction =
                found.prepare_replace_ranges(&snapshot, &template, MAX_RESULT_BYTES, scope, job, include_inverse)?;
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
                .checked_add(if include_inverse {
                    found.range.end.0 - found.range.start.0
                } else {
                    0
                })
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
fn read_capture_range(
    snapshot: &PagedSnapshot,
    range: Range<TextOffset>,
    limit: usize,
    job: &SearchJob,
    resolve: &mut impl FnMut(PageTicket) -> Result<bool, String>,
) -> Result<String, ReplaceError> {
    let length = range.end.0.checked_sub(range.start.0).ok_or(ReplaceError::Stale)?;
    if length > limit {
        return Err(ReplaceError::StagingLimit);
    }
    if length == 0 {
        return Ok(String::new());
    }
    let mut request = snapshot
        .begin_read(range, limit, &Budget::new(limit))
        .map_err(|_| ReplaceError::StagingLimit)?;
    loop {
        if job.is_cancelled() {
            return Err(ReplaceError::Cancelled);
        }
        match request.poll() {
            WindowPoll::Ready(value) => return Ok(value.text().into()),
            WindowPoll::Pending(ticket) => {
                if !resolve(ticket).map_err(|_| ReplaceError::Stale)? {
                    std::thread::yield_now();
                }
            }
            _ => return Err(ReplaceError::Stale),
        }
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
/// Regex streams hard-partial candidates; contextual syntax uses an exact bounded fallback.
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
        if query.mode == SearchMode::Regex && regex::streamable(query) {
            let capacity = query.results_ram_bytes.min(MAX_RESULT_BYTES) / std::mem::size_of::<SearchMatch>();
            regex::scan_stream(
                snapshot.len(),
                query,
                job,
                |start, size| {
                    let part = window(snapshot, start, size, job, &mut resolve)?;
                    if part.range().start.0 != start {
                        return Err(Completeness::Unsupported);
                    }
                    Ok(part.text().into())
                },
                |captures| {
                    let range = captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
                    if range.start < selection.start || range.end > selection.end {
                        return Ok(());
                    }
                    result.count += 1;
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
                    Ok(())
                },
            )?;
            result.count_complete = true;
            return Ok(());
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
            let document = Document::from_utf8(&subject, Budget::new(regex::CONTEXT_LIMIT * 2), Budget::new(1))
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
        let capacity = query.results_ram_bytes.min(MAX_RESULT_BYTES) / std::mem::size_of::<SearchMatch>();
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
            let document = Document::from_utf8(text.text(), Budget::new(size.saturating_mul(2).max(1)), Budget::new(1))
                .map_err(|_| Completeness::Unsupported)?;
            let mut local = query.clone();
            local.selection = Some(TextOffset(next - base)..TextOffset(selection.end.0.min(end) - base));
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
                let range = TextOffset(found.range.start.0 + base)..TextOffset(found.range.end.0 + base);
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
    fn paged_partial_regex_crosses_windows_beyond_former_subject_limit() {
        let length = regex::CONTEXT_LIMIT + regex::STREAM_WINDOW;
        let begin = regex::CONTEXT_LIMIT - regex::STREAM_WINDOW - 3;
        let finish = regex::CONTEXT_LIMIT + 5;
        let generation = Generation(1987);
        let page_size = 65536;
        let (source, publisher) = MemorySource::new(
            length as u64,
            generation,
            SourceKind::Paged,
            page_size,
            page_size,
            Budget::new(page_size * 2),
        )
        .unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let mut query = SearchQuery::literal("(?s)BEGIN\\n.*?\\nEND");
        query.mode = SearchMode::Regex;
        let mut supplied = 0usize;
        let result = scan_paged(
            &snapshot,
            &query,
            &SearchJob::default(),
            |ticket| {
                let start = ticket.page as usize * page_size;
                let end = (start + page_size).min(length);
                let mut page = vec![b'x'; end - start];
                for (at, marker) in [(begin, b"BEGIN\n".as_slice()), (finish, b"\nEND".as_slice())] {
                    for (offset, byte) in marker.iter().enumerate() {
                        let global = at + offset;
                        if global >= start && global < end {
                            page[global - start] = *byte;
                        }
                    }
                }
                supplied += page.len();
                publisher
                    .publish(ticket, &page, generation)
                    .map_err(|e| format!("{e:?}"))?;
                Ok(true)
            },
            |_| {},
        );
        assert_eq!(result.completeness, Completeness::Complete);
        assert!(result.count_complete);
        assert_eq!(result.count, 1);
        assert_eq!(result.matches[0].range, TextOffset(begin)..TextOffset(finish + 4));
        assert!(supplied >= length);
    }
    #[test]
    fn streamed_capture_replacement_beyond_sixty_four_mib_reads_only_bounded_ranges() {
        let length = 65 * 1024 * 1024;
        let page_size = 65536;
        let generation = Generation(123456);
        let (source, publisher) = MemorySource::new(
            length as u64,
            generation,
            SourceKind::Paged,
            page_size,
            page_size,
            Budget::new(page_size * 2),
        )
        .unwrap();
        let snapshot = PagedSnapshot::utf8(source, 0).unwrap();
        let first = 64 * 1024 * 1024 - 1;
        let second = first + 100;
        let mut resolve = |ticket: PageTicket| {
            let base = ticket.page as usize * page_size;
            let mut bytes = vec![b'x'; page_size.min(length - base)];
            for (start, value) in [(first, b"ABC".as_slice()), (second, b"AC".as_slice())] {
                for (index, byte) in value.iter().enumerate() {
                    if let Some(local) = (start + index).checked_sub(base).filter(|local| *local < bytes.len()) {
                        bytes[local] = *byte;
                    }
                }
            }
            publisher
                .publish(ticket, &bytes, generation)
                .map_err(|error| format!("{error:?}"))?;
            Ok(true)
        };
        let mut query = SearchQuery::literal("(A)(B?)C");
        query.mode = SearchMode::Regex;
        let result = scan_paged(&snapshot, &query, &SearchJob::default(), &mut resolve, |_| {});
        assert_eq!(result.completeness, Completeness::Complete);
        assert_eq!(result.matches.len(), 2);
        let transaction = result
            .prepare_replace_streaming(
                &snapshot,
                "$2-$1-$0",
                ReplaceScope::All,
                &SearchJob::default(),
                &mut resolve,
            )
            .unwrap();
        assert_eq!(transaction.edits[0].range.start, TextOffset(first));
        assert_eq!(transaction.edits[0].insert, "B-A-ABC");
        assert_eq!(transaction.edits[1].insert, "-A-AC");
        let one = result
            .prepare_replace_streaming(
                &snapshot,
                "$1",
                ReplaceScope::One(result.matches[1].range.clone()),
                &SearchJob::default(),
                &mut resolve,
            )
            .unwrap();
        assert_eq!(one.edits.len(), 1);
        assert_eq!(one.edits[0].range.start, TextOffset(second));
        let cancelled = SearchJob::default();
        cancelled.cancel();
        assert!(matches!(
            result.prepare_replace_streaming(&snapshot, "$0", ReplaceScope::All, &cancelled, |_| panic!(
                "cancelled replacement must not read"
            )),
            Err(ReplaceError::Cancelled)
        ));
    }
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
        assert_eq!(result.matches[0].range, TextOffset(WINDOW - 2)..TextOffset(WINDOW + 5));
        assert_eq!(result.count, 1);
        assert!(result.count_complete);
    }
}

/// Stage reviewed replacement payloads and inverses as immutable disk sources on a worker.
/// The actor still validates the prepared source and journals before publication.
pub fn stage_source_replacement(
    source: &PagedSnapshot,
    transaction: EditTransaction,
    job: &SearchJob,
    mut resolve: impl FnMut(PageTicket) -> Result<bool, String>,
    platform: Arc<dyn bareline_platform::LocalFileSystem>,
    cache: &std::path::Path,
    quota: u64,
) -> Result<bareline_document::paged::PreparedSourceTransaction, String> {
    use bareline_document::paged::{OwnedTextRange, SourceEdit, SourceTransactionPoll};
    if transaction.base_revision != source.revision {
        return Err("Replacement source changed".into());
    }
    let budget = Budget::new(8 * 1024 * 1024);
    let mut builder = bareline_file_io::owned_store::StreamingStoreBuilder::new(
        cache,
        quota,
        platform,
        bareline_file_io::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 65536,
            page_cache_bytes: 1024 * 1024,
        },
        budget.clone(),
        job.io_cancel.clone(),
    )
    .map_err(|error| error.to_string())?;
    let mut ranges = Vec::new();
    if transaction.edits.len() > 4096 {
        return Err("Source transaction edit limit".into());
    }
    for edit in transaction.edits {
        if job.is_cancelled() {
            return Err("Cancelled".into());
        }
        let start = builder.len();
        let mut cursor = edit.range.start.0;
        while cursor < edit.range.end.0 {
            let part = window(source, cursor, WINDOW.min(edit.range.end.0 - cursor), job, &mut resolve)
                .map_err(|error| format!("{error:?}"))?;
            let length = part.text().len().min(edit.range.end.0 - cursor);
            if length == 0 {
                return Err("Source made no progress".into());
            }
            builder
                .append_utf8(&part.text()[..length])
                .map_err(|error| error.to_string())?;
            cursor += length;
        }
        let inverse = start..builder.len();
        let inserted = builder.append_utf8(&edit.insert).map_err(|error| error.to_string())?;
        ranges.push((edit.range, inverse, inserted));
    }
    let owned = builder.finish().map_err(|error| error.to_string())?;
    let edits = ranges
        .into_iter()
        .map(|(range, inverse, inserted)| SourceEdit {
            range,
            inverse: OwnedTextRange {
                source: owned.clone(),
                range: inverse,
            },
            inserted: OwnedTextRange {
                source: owned.clone(),
                range: inserted,
            },
        })
        .collect();
    let mut request = source
        .prepare_source_transaction(
            edits,
            bareline_document::history::EditMetadata {
                origin: bareline_document::history::EditOrigin::ReplaceAll,
                ..Default::default()
            },
            budget,
        )
        .map_err(|error| format!("{error:?}"))?;
    loop {
        if job.is_cancelled() {
            request.cancel();
            return Err("Cancelled".into());
        }
        match request.poll() {
            SourceTransactionPoll::Ready(prepared) => return Ok(prepared),
            SourceTransactionPoll::Progress => {}
            SourceTransactionPoll::Pending(ticket) => {
                if !request.resolve_owned(ticket).map_err(|error| format!("{error:?}"))? && !resolve(ticket)? {
                    std::thread::yield_now();
                }
            }
            SourceTransactionPoll::Cancelled => return Err("Cancelled".into()),
            _ => return Err("Replacement source validation failed".into()),
        }
    }
}
