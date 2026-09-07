// SPDX-License-Identifier: MPL-2.0
//! Paged language reads run only on this worker. UI snapshots are projections.
use bareline_document::{Budget, DocumentSnapshot, TextOffset, paged::WindowPoll};
use bareline_editor_surface::paged_view::PagedReadHandle;
use bareline_syntax::{
    Cancellation, Language, LexerPreference, MAX_REQUEST_BYTES, SyntaxResult,
    folding::{Fold, FoldAccumulator},
    stream::{StreamLexer, ViewportProjection},
};
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};
pub(super) struct ResultWindow {
    pub syntax: Option<SyntaxResult>,
    pub folds: Vec<Fold>,
    pub first_line: usize,
    pub partial: bool,
}
pub(super) struct Job {
    pub identity: (u64, u64),
    pub local: DocumentSnapshot,
    pub origin: TextOffset,
    pub language: Language,
    pub preference: LexerPreference,
    pub definition: Option<Arc<bareline_syntax::udl::Definition>>,
    pub cancel: Cancellation,
    pub receiver: Receiver<Result<ResultWindow, String>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub(super) fn spawn(
    handle: PagedReadHandle,
    local: DocumentSnapshot,
    origin: TextOffset,
    language: Language,
    preference: LexerPreference,
    definition: Option<Arc<bareline_syntax::udl::Definition>>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Result<Job, String> {
    let (tx, receiver) = mpsc::sync_channel(1);
    let cancel = Cancellation::default();
    let job = Job {
        identity: handle.snapshot().identity_token(),
        local: local.clone(),
        origin,
        language,
        preference,
        definition: definition.clone(),
        cancel: cancel.clone(),
        receiver,
    };
    std::thread::Builder::new()
        .name("bareline-paged-syntax".into())
        .spawn(move || {
            let run = (|| {
                let mut lexer = StreamLexer::new(language, preference, definition);
                let mut folds = FoldAccumulator::default();
                let mut projection = Some(
                    ViewportProjection::new(local.clone(), origin, language)
                        .map_err(|e| format!("{e:?}"))?,
                );
                let mut first_line = 0;
                loop {
                    if cancel.is_cancelled() {
                        return Err("Syntax cancelled".into());
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
                    let window = lexer
                        .advance(&text, start, eof, &cancel)
                        .map_err(|e| format!("{e:?}"))?;
                    folds
                        .advance_stream(&window, 8192)
                        .map_err(|e| format!("{e:?}"))?;
                    if let Some(view) = &mut projection {
                        view.accept(&window).map_err(|e| format!("{e:?}"))?;
                    }
                    let syntax = if projection.is_some() && lexer.next().0 >= origin.0 + local.len()
                    {
                        let (syntax, line) = projection
                            .take()
                            .unwrap()
                            .finish()
                            .map_err(|e| format!("{e:?}"))?;
                        first_line = line;
                        Some(syntax)
                    } else {
                        None
                    };
                    if syntax.is_none() && projection.is_none() && !eof {
                        if tx
                            .try_send(Ok(ResultWindow {
                                syntax: None,
                                folds: folds.known().to_vec(),
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
                            folds: folds.known().to_vec(),
                            first_line,
                            partial: !eof,
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
        .map_err(|e| format!("{e:?}"))?;
    for _ in 0..4096 {
        if cancel.is_cancelled() {
            return Err("Syntax cancelled".into());
        }
        match request.poll() {
            WindowPoll::Ready(window) if window.range().start == start => {
                return Ok(window.text().to_owned());
            }
            WindowPoll::Pending(ticket) => {
                if !handle.resolve_page(ticket)? {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Syntax source window unavailable".into()),
        }
    }
    Err("Syntax source busy; retry after viewport changes".into())
}
