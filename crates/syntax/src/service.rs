// SPDX-License-Identifier: MPL-2.0
//! A lazy worker with one pending request. Dropping tickets cancels their work.
use crate::{
    Cancellation, Checkpoint, Error, ForwardLexer, Language, LexerPreference, MAX_REQUEST_BYTES, SyntaxResult,
};
use bareline_document::{DocumentSnapshot, TextOffset};
use std::{
    ops::Range,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
};
type Notify = Arc<dyn Fn() + Send + Sync>;
struct Request {
    preference: LexerPreference,
    definition: Option<Arc<crate::udl::Definition>>,
    source: DocumentSnapshot,
    language: Language,
    range: Range<TextOffset>,
    checkpoint: Option<Checkpoint>,
    cancel: Cancellation,
    reply: SyncSender<Result<SyntaxResult, Error>>,
    notify: Notify,
}
#[derive(Default)]
struct State {
    pending: Option<Request>,
    running: Option<Cancellation>,
    stop: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct SyntaxWorker {
    shared: Arc<Shared>,
}
pub struct SyntaxTicket {
    receiver: Receiver<Result<SyntaxResult, Error>>,
    cancel: Cancellation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Stopped,
}
impl SyntaxTicket {
    pub fn try_recv(&self) -> Result<Result<SyntaxResult, Error>, TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}
impl Drop for SyntaxTicket {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl SyntaxWorker {
    pub fn new() -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new().name("syntax".into()).spawn(move || {
            // Construct and release the !Send native handle on this worker.
            let mut pass: Option<ForwardLexer> = None;
            loop {
                let request = {
                    let Ok(mut state) = worker.state.lock() else {
                        return;
                    };
                    while state.pending.is_none() && !state.stop {
                        let Ok(next) = worker.wake.wait(state) else {
                            return;
                        };
                        state = next;
                    }
                    if state.stop {
                        return;
                    }
                    let request = state.pending.take().unwrap();
                    state.running = Some(request.cancel.clone());
                    request
                };
                let matches_definition = |old: &Option<Arc<crate::udl::Definition>>| match (old, &request.definition) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                };
                let anchor = request
                    .source
                    .line_at(request.range.start)
                    .and_then(|line| request.source.line_range(line))
                    .map_or(TextOffset(0), |range| range.start);
                if !pass.as_ref().is_some_and(|old| {
                    old.source.same_document(&request.source)
                        && old.source.revision == request.source.revision
                        && old.language == request.language
                        && old.options.preference == request.preference
                        && matches_definition(&old.options.definition)
                        && old.next <= anchor
                        && (old.next.0 == 0 || old.checkpoint.is_some())
                }) {
                    pass = Some(ForwardLexer::configured(
                        request.source.clone(),
                        request.language,
                        request.preference,
                        request.definition.clone(),
                    ));
                }
                let result = (|| {
                    if request.range.start > request.range.end
                        || request.range.end.0 > request.source.len()
                        || request.range.end.0 - request.range.start.0 > MAX_REQUEST_BYTES
                    {
                        return Err(Error::InvalidRange);
                    }
                    let pass = pass.as_mut().unwrap();
                    // Native checkpoints are safe restarts only for the native grammar.
                    if request.preference == LexerPreference::Native
                        && let Some(checkpoint) = &request.checkpoint
                    {
                        if checkpoint.source.same_document(&request.source)
                            && checkpoint.source.revision == request.source.revision
                            && checkpoint.language == request.language
                            && checkpoint.offset == request.range.start
                            && matches_definition(&checkpoint.definition)
                        {
                            pass.next = checkpoint.offset;
                            pass.checkpoint = Some(checkpoint.clone());
                        }
                    }
                    loop {
                        request.cancel.check()?;
                        let target = if pass.next < anchor {
                            anchor.0
                        } else {
                            request.range.end.0
                        };
                        let mut end = target.min(pass.next.0.saturating_add(MAX_REQUEST_BYTES));
                        if end < request.range.end.0 {
                            let line = request
                                .source
                                .line_at(TextOffset(end))
                                .map_err(|_| Error::InvalidRange)?;
                            let boundary = request
                                .source
                                .line_range(line)
                                .map_err(|_| Error::InvalidRange)?
                                .start
                                .0;
                            if boundary > pass.next.0 {
                                end = boundary;
                            }
                        }
                        while !request.source.is_boundary(TextOffset(end)) {
                            end -= 1;
                        }
                        let result = pass.advance(TextOffset(end), &request.cancel)?;
                        if end == request.range.end.0 {
                            return Ok(result);
                        }
                        if result.checkpoint.is_none() {
                            return Err(Error::BudgetExceeded);
                        }
                    }
                })();
                if result.is_err() {
                    pass = None;
                }
                let _ = request.reply.try_send(result);
                (request.notify)();
                if let Ok(mut state) = worker.state.lock() {
                    state.running = None;
                } else {
                    return;
                }
            }
        })?;
        Ok(Self { shared })
    }
    /// Supersedes running and queued work; no queue growth or UI-thread lexing.
    pub fn submit(
        &self,
        source: DocumentSnapshot,
        language: Language,
        range: Range<TextOffset>,
        checkpoint: Option<Checkpoint>,
        notify: Notify,
    ) -> Result<SyntaxTicket, SubmitError> {
        self.submit_configured(
            source,
            language,
            range,
            checkpoint,
            notify,
            crate::LexOptions::default(),
        )
    }
    pub fn submit_preferred(
        &self,
        source: DocumentSnapshot,
        language: Language,
        range: Range<TextOffset>,
        checkpoint: Option<Checkpoint>,
        notify: Notify,
        preference: LexerPreference,
    ) -> Result<SyntaxTicket, SubmitError> {
        self.submit_configured(
            source,
            language,
            range,
            checkpoint,
            notify,
            crate::LexOptions {
                line_origin: 0,
                preference,
                definition: None,
            },
        )
    }
    pub fn submit_udl(
        &self,
        source: DocumentSnapshot,
        definition: Arc<crate::udl::Definition>,
        range: Range<TextOffset>,
        checkpoint: Option<Checkpoint>,
        notify: Notify,
    ) -> Result<SyntaxTicket, SubmitError> {
        self.submit_configured(
            source,
            Language::PlainText,
            range,
            checkpoint,
            notify,
            crate::LexOptions {
                line_origin: 0,
                definition: Some(definition),
                preference: LexerPreference::Native,
            },
        )
    }
    fn submit_configured(
        &self,
        source: DocumentSnapshot,
        language: Language,
        range: Range<TextOffset>,
        checkpoint: Option<Checkpoint>,
        notify: Notify,
        options: crate::LexOptions,
    ) -> Result<SyntaxTicket, SubmitError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let cancel = Cancellation::default();
        let previous = {
            let mut state = self.shared.state.lock().map_err(|_| SubmitError::Stopped)?;
            if state.stop {
                return Err(SubmitError::Stopped);
            }
            if let Some(running) = &state.running {
                running.cancel();
            }
            state.pending.replace(Request {
                preference: options.preference,
                definition: options.definition,
                source,
                language,
                range,
                checkpoint,
                cancel: cancel.clone(),
                reply,
                notify,
            })
        };
        if let Some(previous) = previous {
            previous.cancel.cancel();
            let _ = previous.reply.try_send(Err(Error::Cancelled));
            (previous.notify)();
        }
        self.shared.wake.notify_one();
        Ok(SyntaxTicket { receiver, cancel })
    }
}
impl Drop for SyntaxWorker {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.stop = true;
            if let Some(running) = &state.running {
                running.cancel();
            }
            if let Some(pending) = state.pending.take() {
                pending.cancel.cancel();
                let _ = pending.reply.try_send(Err(Error::Cancelled));
            }
        }
        self.shared.wake.notify_one();
    }
}
