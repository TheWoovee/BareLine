// SPDX-License-Identifier: MPL-2.0
//! A lazy worker with one pending request. Dropping tickets cancels their work.
use crate::{Cancellation, Checkpoint, Error, Language, SyntaxResult, lex_with_session};
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
        std::thread::Builder::new()
            .name("syntax".into())
            .spawn(move || {
                // Construct and release the !Send native handle on this worker.
                let mut native: Option<(
                    DocumentSnapshot,
                    Language,
                    TextOffset,
                    bareline_lexilla_bridge::LexerSession,
                )> = None;
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
                    if native.as_ref().is_some_and(|(source, language, next, _)| {
                        !source.same_document(&request.source)
                            || source.revision != request.source.revision
                            || *language != request.language
                            || *next != request.range.start
                    }) {
                        native = None;
                    }
                    if native.is_none()
                        && request.range.start.0 == 0
                        && request.language != Language::PlainText
                    {
                        use bareline_lexilla_bridge::CppMode;
                        let mode = match request.language {
                            Language::JavaScript | Language::TypeScript => CppMode::JavaScript,
                            Language::Go => CppMode::Go,
                            Language::Java => CppMode::Java,
                            Language::CSharp => CppMode::CSharp,
                            _ => CppMode::Default,
                        };
                        if let Ok(session) = bareline_lexilla_bridge::LexerSession::new(
                            request.language.metadata().lexilla,
                            request.language.metadata().keywords,
                            mode,
                        ) {
                            native = Some((
                                request.source.clone(),
                                request.language,
                                request.range.start,
                                session,
                            ));
                        }
                    }
                    let next = request.range.end;
                    let result = if let Some(definition) = request.definition {
                        crate::lex_udl(
                            request.source,
                            definition,
                            request.range,
                            request.checkpoint.as_ref(),
                            &request.cancel,
                        )
                    } else {
                        lex_with_session(
                            request.source,
                            request.language,
                            request.range,
                            request.checkpoint.as_ref(),
                            &request.cancel,
                            native.as_mut().map(|(_, _, _, session)| session),
                        )
                    };
                    if result
                        .as_ref()
                        .is_ok_and(|result| result.fold_levels.is_some())
                    {
                        if let Some((_, _, offset, _)) = &mut native {
                            *offset = next;
                        }
                    } else {
                        native = None;
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
        self.submit_configured(source, language, range, checkpoint, notify, None)
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
            Some(definition),
        )
    }
    fn submit_configured(
        &self,
        source: DocumentSnapshot,
        language: Language,
        range: Range<TextOffset>,
        checkpoint: Option<Checkpoint>,
        notify: Notify,
        definition: Option<Arc<crate::udl::Definition>>,
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
                definition,
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
