// SPDX-License-Identifier: MPL-2.0
//! Paged language reads run only on this worker. UI snapshots are projections.
use bareline_document::{
    Budget, ContentStateId, DocumentSnapshot, TextOffset,
    paged::{PagedSnapshot, WindowPoll},
};
use bareline_editor_surface::paged_view::{PagedReadHandle, ViewportSegment};
use bareline_syntax::{
    Cancellation, Language, LexerPreference, MAX_REQUEST_BYTES, SyntaxResult,
    folding::{AnchoredFold, FoldAccumulator},
    stream::{StreamCheckpoint, StreamLexer, ViewportProjection},
};
use std::sync::{
    Arc, Mutex, PoisonError,
    mpsc::{self, Receiver},
};
/// Source bytes past a view whose folds one styling pass gathers (LNX-PERF-001).
pub(super) const FOLD_MARGIN_BYTES: usize = 2 << 20;
/// Resume points kept for one document; past this every other one is dropped,
/// so the rest stay spread over the text.
pub(super) const MAX_RESUME_POINTS: usize = 4096;
/// Documents whose resume points are kept, the most recently styled last.
const MAX_RESUME_DOCUMENTS: usize = 8;
type Definition = Option<Arc<bareline_syntax::udl::Definition>>;
/// ADR-17/SRC-14 for paged views: the verified stream-lexer states that earlier
/// styling passes over one document left at line starts, ascending. A pass
/// starts from the last one at or before its view instead of from byte 0, so a
/// keystroke or a scroll deep in a large file lexes about one window, not the
/// whole text before it again (LNX-PERF-001). Every pane and pass of the
/// document shares them; they are only ever a cache.
struct ResumePoints {
    /// Identity and content of the text the points describe.
    source: ((u64, u64), ContentStateId),
    language: Language,
    preference: LexerPreference,
    definition: Definition,
    points: Vec<StreamCheckpoint>,
}
impl ResumePoints {
    fn styles(&self, language: Language, preference: LexerPreference, definition: &Definition) -> bool {
        self.language == language
            && self.preference == preference
            && match (&self.definition, definition) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}
static RESUME_POINTS: Mutex<Vec<ResumePoints>> = Mutex::new(Vec::new());
/// First pre-edit byte the step from `previous` to `next` touches; `None` when
/// `next` does not directly follow it.
fn first_change(previous: ((u64, u64), ContentStateId), next: &PagedSnapshot) -> Option<usize> {
    if previous.0 == next.identity_token() && previous.1 == next.content_state {
        return Some(usize::MAX);
    }
    let change = next
        .applied_change()
        .filter(|change| change.matches_before(previous.0, previous.1))?;
    // Receipts list disjoint edits in ascending pre-edit order.
    Some(change.edits().first().map_or(usize::MAX, |edit| edit.before.start.0))
}
/// Carries the resume points of `source`'s document to `source` and returns
/// the last one at or before `view_start`. A point survives an edit only when
/// the edit starts after it, so the text before it and the byte at it are
/// unchanged (as `Checkpoint::rebase` keeps a resident checkpoint). A step
/// that cannot be followed (several revisions at once) drops them all; a view
/// still showing an older revision than another pane resumes from none and
/// leaves them for the newer one.
fn resume_point(
    source: &PagedSnapshot,
    language: Language,
    preference: LexerPreference,
    definition: &Definition,
    view_start: TextOffset,
) -> Option<StreamCheckpoint> {
    let mut documents = RESUME_POINTS.lock().unwrap_or_else(PoisonError::into_inner);
    let identity = source.identity_token();
    let index = documents.iter().position(|entry| entry.source.0.0 == identity.0);
    let mut entry = match index {
        Some(index) if documents[index].source.0.1 > identity.1 => return None,
        Some(index) => documents.remove(index),
        None => ResumePoints {
            source: (identity, source.content_state),
            language,
            preference,
            definition: definition.clone(),
            points: Vec::new(),
        },
    };
    match first_change(entry.source, source) {
        Some(first) if entry.styles(language, preference, definition) => {
            entry.points.retain(|point| point.offset().0 < first);
        }
        _ => entry.points.clear(),
    }
    entry.source = (identity, source.content_state);
    entry.language = language;
    entry.preference = preference;
    entry.definition = definition.clone();
    let point = entry
        .points
        .iter()
        .rev()
        .find(|point| point.offset() <= view_start)
        .cloned();
    documents.push(entry);
    if documents.len() > MAX_RESUME_DOCUMENTS {
        documents.remove(0);
    }
    point
}
/// Keeps `point`, which a pass over the text `identity` names reached, for
/// later passes; a pass over text that has changed since keeps nothing.
fn keep_resume_point(
    identity: (u64, u64),
    language: Language,
    preference: LexerPreference,
    definition: &Definition,
    point: StreamCheckpoint,
) {
    let mut documents = RESUME_POINTS.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(entry) = documents
        .iter_mut()
        .find(|entry| entry.source.0 == identity && entry.styles(language, preference, definition))
    else {
        return;
    };
    if let Err(index) = entry
        .points
        .binary_search_by_key(&point.offset(), StreamCheckpoint::offset)
    {
        entry.points.insert(index, point);
        if entry.points.len() > MAX_RESUME_POINTS {
            let mut position = 0_usize;
            entry.points.retain(|_| {
                position += 1;
                position % 2 == 1
            });
        }
    }
}
/// Drops `document`'s resume points, so its next pass lexes from byte 0.
#[cfg(test)]
pub(super) fn forget_resume_points(document: u64) {
    RESUME_POINTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|entry| entry.source.0.0 != document);
}
pub(super) struct ResultWindow {
    pub syntax: Option<SyntaxResult>,
    pub folds: Vec<AnchoredFold>,
    pub first_line: usize,
    pub partial: bool,
}
pub(super) struct Job {
    pub identity: (u64, u64),
    pub local: DocumentSnapshot,
    pub origin: TextOffset,
    pub segments: Vec<ViewportSegment>,
    pub language: Language,
    pub preference: LexerPreference,
    pub definition: Option<Arc<bareline_syntax::udl::Definition>>,
    pub cancel: Cancellation,
    pub receiver: Receiver<Result<ResultWindow, String>>,
    /// Where the pass starts lexing: byte 0 or the resume point it continues.
    #[cfg(test)]
    pub start: TextOffset,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn(
    handle: PagedReadHandle,
    local: DocumentSnapshot,
    origin: TextOffset,
    segments: Vec<ViewportSegment>,
    language: Language,
    preference: LexerPreference,
    definition: Option<Arc<bareline_syntax::udl::Definition>>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Result<Job, String> {
    if segments.len() > 8193 {
        return Err("Syntax projection exceeds segment budget".into());
    }
    if !segments.is_empty() {
        let mut local_end = 0;
        let mut source_end = 0;
        for segment in &segments {
            if segment.local.start.0 != local_end
                || segment.local.end.0 < local_end
                || segment.source.start.0 < source_end
                || segment.source.end.0 < segment.source.start.0
                || segment.source.end.0 > handle.snapshot().len()
                || segment.local.end.0 - segment.local.start.0 != segment.source.end.0 - segment.source.start.0
            {
                return Err("Invalid syntax projection map".into());
            }
            local_end = segment.local.end.0;
            source_end = segment.source.end.0;
        }
        if local_end != local.len() {
            return Err("Incomplete syntax projection map".into());
        }
    }
    // Verified styling continues a pass from the last resume point before the
    // view, or lexes from the start of the text (ADR-17, LNX-PERF-001).
    let view_start = segments.first().map_or(origin, |segment| segment.source.start);
    let resume = resume_point(handle.snapshot(), language, preference, &definition, view_start);
    let source_identity = handle.snapshot().identity_token();
    let view_end = segments
        .last()
        .map_or(origin.0 + local.len(), |segment| segment.source.end.0);
    let fold_limit = view_end.saturating_add(FOLD_MARGIN_BYTES);
    let (tx, receiver) = mpsc::sync_channel(1);
    let cancel = Cancellation::default();
    let job = Job {
        identity: handle.snapshot().identity_token(),
        local: local.clone(),
        origin,
        segments: segments.clone(),
        language,
        preference,
        definition: definition.clone(),
        cancel: cancel.clone(),
        receiver,
        #[cfg(test)]
        start: resume.as_ref().map_or(TextOffset(0), StreamCheckpoint::offset),
    };
    std::thread::Builder::new()
        .name("bareline-paged-syntax".into())
        .spawn(move || {
            let run = (|| {
                let mut lexer = match &resume {
                    Some(point) => StreamLexer::resume(point),
                    None => StreamLexer::new(language, preference, definition.clone()),
                };
                let mut folds = resume.as_ref().map_or_else(FoldAccumulator::default, |point| {
                    FoldAccumulator::resuming_at(point.offset())
                });
                let mut folds_active = true;
                let mut projection =
                    Some(ViewportProjection::for_projection(local.clone(), language).map_err(|e| e.to_string())?);
                let mut first_line = 0;
                loop {
                    if cancel.is_cancelled() {
                        return Err("Syntax cancelled".into());
                    }
                    // Folds are gathered for the view and a bounded margin past it;
                    // the rest wait for a view that reaches them (LNX-PERF-001).
                    if projection.is_none() && lexer.next().0 >= fold_limit {
                        let last = ResultWindow {
                            syntax: None,
                            folds: folds.anchored().to_vec(),
                            first_line,
                            partial: true,
                        };
                        if tx.send(Ok(last)).is_ok() {
                            notify();
                        }
                        return Ok(());
                    }
                    let start = lexer.next();
                    let mut text = read(&handle, start, MAX_REQUEST_BYTES, &cancel)?;
                    let eof = start.0 + text.len() == handle.snapshot().len();
                    if !eof {
                        let end = text
                            .rfind('\n')
                            .map(|i| i + 1)
                            .or_else(|| {
                                text.as_bytes()[..text.len().saturating_sub(1)]
                                    .iter()
                                    .rposition(|byte| *byte == b'\r')
                                    .map(|i| i + 1)
                            })
                            .ok_or("Syntax unavailable: line exceeds bounded window")?;
                        text.truncate(end);
                    }
                    let window = lexer.advance(&text, start, eof, &cancel).map_err(|e| e.to_string())?;
                    if let Some(point) = lexer.checkpoint() {
                        keep_resume_point(source_identity, language, preference, &definition, point);
                    }
                    if folds_active {
                        match folds.advance_stream(&window, 8192) {
                            Ok(()) => (),
                            Err(bareline_syntax::Error::BudgetExceeded) => folds_active = false,
                            Err(error) => return Err(error.to_string()),
                        }
                    }

                    if let Some(view) = &mut projection {
                        if segments.is_empty() {
                            view.accept_segment(
                                &window,
                                origin..TextOffset(origin.0 + local.len()),
                                TextOffset(0)..TextOffset(local.len()),
                            )
                            .map_err(|e| e.to_string())?;
                        } else {
                            for segment in &segments {
                                view.accept_segment(&window, segment.source.clone(), segment.local.clone())
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                    }
                    let syntax = if projection.is_some()
                        && lexer.next().0
                            >= segments
                                .last()
                                .map_or(origin.0 + local.len(), |segment| segment.source.end.0)
                    {
                        let (syntax, line) = projection.take().unwrap().finish().map_err(|e| e.to_string())?;
                        first_line = line;
                        Some(syntax)
                    } else {
                        None
                    };
                    if syntax.is_none() && projection.is_none() && !eof {
                        if tx
                            .try_send(Ok(ResultWindow {
                                syntax: None,
                                folds: folds.anchored().to_vec(),
                                first_line,
                                partial: true,
                            }))
                            .is_ok()
                        {
                            notify();
                        }
                    }
                    if syntax.is_some() || eof {
                        let mut value = Ok(ResultWindow {
                            syntax,
                            folds: folds.anchored().to_vec(),
                            first_line,
                            partial: !eof || !folds_active || !folds.context_complete(),
                        });
                        loop {
                            match tx.try_send(value) {
                                Ok(()) => break,
                                Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                                Err(mpsc::TrySendError::Full(v)) => {
                                    value = v;
                                    if cancel.is_cancelled() {
                                        return Ok(());
                                    }
                                    std::thread::yield_now();
                                }
                            }
                        }
                        notify();
                    }
                    if eof {
                        return Ok(());
                    }
                }
            })();
            if let Err(error) = run {
                let _ = tx.try_send(Err(error));
                notify();
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(job)
}
pub(super) fn read(
    handle: &PagedReadHandle,
    start: TextOffset,
    limit: usize,
    cancel: &Cancellation,
) -> Result<String, String> {
    let mut request = handle
        .snapshot()
        .begin_viewport(start, limit, &Budget::new(MAX_REQUEST_BYTES))
        .map_err(|e| e.to_string())?;
    for _ in 0..4096 {
        if cancel.is_cancelled() {
            return Err("Syntax cancelled".into());
        }
        match request.poll() {
            WindowPoll::Ready(window) if window.range().start == start => {
                return Ok(window.text().to_owned());
            }
            WindowPoll::Pending(ticket) => {
                if !handle.resolve_page(ticket).map_err(|error| error.to_string())? {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Syntax source window unavailable".into()),
        }
    }
    Err("Syntax source busy; retry after viewport changes".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// ADR-17: a stream pass resumed from a resume point inside a block comment
    /// styles what follows exactly as the pass from byte 0 did. A primary
    /// (Lexilla) pass leaves none below its session bound: its state is opaque.
    #[test]
    fn a_resumed_stream_pass_styles_like_a_pass_from_zero() {
        let cancel = Cancellation::default();
        let first = format!("fn f() {{\n    /* open\n{}", "     * x\n".repeat(50));
        let second = "     */\n    let s = \"text\";\n}\n";
        let mut zero = StreamLexer::new(Language::Rust, LexerPreference::Native, None);
        zero.advance(&first, TextOffset(0), false, &cancel).unwrap();
        let point = zero.checkpoint().expect("a native pass leaves a resume point");
        assert_eq!((point.offset(), point.line()), (TextOffset(first.len()), 52));
        let expected = zero.advance(second, TextOffset(first.len()), true, &cancel).unwrap();
        assert!(zero.checkpoint().is_none(), "a finished pass has no next window");
        let mut resumed = StreamLexer::resume(&point);
        let actual = resumed.advance(second, TextOffset(first.len()), true, &cancel).unwrap();
        assert_eq!(actual.first_line, expected.first_line);
        assert_eq!(actual.syntax.spans, expected.syntax.spans);
        assert!(
            actual
                .syntax
                .spans
                .iter()
                .any(|span| { span.kind == bareline_syntax::StyleKind::Comment && span.range.start == TextOffset(0) })
        );
        let mut primary = StreamLexer::new(Language::Rust, LexerPreference::Lexilla, None);
        primary.advance(&first, TextOffset(0), false, &cancel).unwrap();
        assert!(primary.checkpoint().is_none());
    }
}
