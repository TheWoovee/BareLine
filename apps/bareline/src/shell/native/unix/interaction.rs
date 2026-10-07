// SPDX-License-Identifier: MPL-2.0
//! Questions and file dialogs that answer later (Linux).
//!
//! On Windows every dialog is modal: the call returns the answer, and the
//! shell's call sites (a command handler, a queued close, a click on a banner)
//! simply continue. Here neither kind can answer while the shell waits on the
//! event-loop thread: the portal's dialogs are windows of another process, and
//! the About window and message prompts are drawn by the shell itself. Waiting
//! would freeze the window, and GNOME would mark it "not responding". So the
//! call sites stay synchronous through a replay:
//!
//! 1. The shell marks code that may ask (a command, a queued close, an input
//!    event, the save pipeline's pump) as a [`Scope`] it can run again.
//! 2. A question asked inside a scope starts its prompt or dialog and returns
//!    nothing at once, so the caller takes its safe answer: nothing is opened,
//!    saved, discarded or closed. The innermost scope around the question that
//!    began before the run consumed any answer becomes the question's owner:
//!    a command run by a key runs again by itself, while a scope that began
//!    after an earlier answer was consumed could not receive that answer
//!    again, so the scope around it owns the question instead.
//! 3. When the answer is in, it is armed together with the answers that run
//!    had already consumed, and the shell runs the owner again: every question
//!    now returns its armed answer in order, so the code continues as it does
//!    after a modal dialog returns on Windows.
//! 4. Answers that run did not consume are dropped ([`Interactions::settle`]),
//!    so an answer is never reused by a later, unrelated question.
//!
//! A question asked outside any scope (a background pump) takes its safe
//! answer without showing anything, which is what these systems did before.
//! One question is open at a time; another asked meanwhile takes its safe
//! answer, as a second modal dialog could not open on Windows either.
//!
//! "Stop waiting" counts a portal dialog as cancelled, but the portal's own
//! window stays open: `LinuxDialogs` cannot close a portal request yet
//! (`org.freedesktop.portal.Request.Close`, a follow-up). A location chosen
//! there afterwards is ignored, and the next dialog may open beside it.
use super::super::prompt::{PromptButtonView, PromptLevel, PromptView};
use bareline_platform_linux::{
    InAppPrompt, LinuxDialogs,
    dialogs::{DialogRequest, DialogResult, PendingDialog},
    prompts::CANCEL_ID,
};
use std::{any::Any, cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};

/// What the shell asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Question {
    Prompt(InAppPrompt),
    Dialog(DialogRequest),
}
impl Question {
    /// Whether an armed answer to `self` answers `asked`. A Save dialog's
    /// options carry a default folder that a worker checks within a time
    /// budget, so the same run may describe it differently the second time;
    /// only that folder may differ. The kind and the suggested name, which
    /// name the document being saved, must match, so one document's Save
    /// dialog never answers another's.
    fn answers(&self, asked: &Question) -> bool {
        match (self, asked) {
            (Self::Dialog(DialogRequest::Save(armed)), Self::Dialog(DialogRequest::Save(asked))) => {
                armed.kind == asked.kind
                    && armed.default_name == asked.default_name
                    && armed.app_confirms_overwrite == asked.app_confirms_overwrite
            }
            _ => self == asked,
        }
    }
    /// A short description for the diagnostic log.
    fn describe(&self) -> String {
        match self {
            // Kinds only: prompt text, document names and paths stay out of
            // diagnostics.
            Self::Prompt(_) => "prompt".to_string(),
            Self::Dialog(DialogRequest::Save(_)) => "save-dialog".to_string(),
            Self::Dialog(DialogRequest::Open { .. }) => "open-dialog".to_string(),
            Self::Dialog(DialogRequest::PickFolder) => "folder-dialog".to_string(),
        }
    }
}
/// What the person answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    /// The id of the pressed button (`CANCEL_ID` for Escape).
    Button(i32),
    /// The chosen paths, empty when cancelled, or why the dialog failed.
    Paths(DialogResult),
}
impl Answer {
    pub(super) fn button(&self) -> Option<i32> {
        match self {
            Self::Button(id) => Some(*id),
            Self::Paths(_) => None,
        }
    }
    pub(super) fn paths(self) -> Option<DialogResult> {
        match self {
            Self::Paths(result) => Some(result),
            Self::Button(_) => None,
        }
    }
}

enum Waiting {
    /// An in-app prompt the shell draws; its button answers.
    Prompt(InAppPrompt),
    /// A portal dialog in another window; the shell shows that it waits.
    Dialog(PendingDialog),
    /// A message that only needs to be read; nothing waits for it.
    Notice(InAppPrompt),
}
struct Open {
    question: Option<Question>,
    waiting: Waiting,
    /// The answers the asking run consumed before this question.
    earlier: Vec<(Question, Answer)>,
    /// What the shell runs again once the answer is in.
    owner: Option<Box<dyn Any>>,
}

#[derive(Default)]
struct State {
    /// Scopes entered and not yet left.
    depth: usize,
    /// Answers the current run consumed, in order.
    consumed: Vec<(Question, Answer)>,
    /// Answers waiting for the run that asked for them.
    armed: VecDeque<(Question, Answer)>,
    open: Option<Open>,
    /// A question opened in the current run whose owner is not decided yet.
    unclaimed: bool,
    /// The owner to run again: its answers are armed.
    ready: Option<Box<dyn Any>>,
    /// Questions opened in a row by runs that had already consumed answers. A
    /// run that keeps asking something new (its state changed between runs)
    /// stops after `MAX_CHAIN` instead of asking forever.
    chain: usize,
}
const MAX_CHAIN: usize = 16;

/// The questions of one editor window.
pub(super) struct Interactions {
    state: Rc<RefCell<State>>,
    dialogs: RefCell<LinuxDialogs>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

/// Marks code the shell can run again; see the module documentation.
pub struct Scope {
    state: Option<Rc<RefCell<State>>>,
    /// How many answers the run had consumed when this scope began.
    entered_with: usize,
    owner: Option<Box<dyn Any>>,
}
impl Scope {
    /// A scope that records nothing (no platform yet, or nothing can ask).
    pub(super) fn inert() -> Self {
        Self {
            state: None,
            entered_with: 0,
            owner: None,
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let Some(state) = &self.state else {
            return;
        };
        // A scope never panics while it unwinds; a busy state is left alone.
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        state.depth = state.depth.saturating_sub(1);
        if state.unclaimed && self.entered_with == 0 {
            state.unclaimed = false;
            if let Some(open) = &mut state.open {
                open.owner = self.owner.take();
            }
        }
        if state.depth == 0 {
            state.consumed.clear();
            state.unclaimed = false;
        }
    }
}

impl Interactions {
    pub(super) fn new(dialogs: LinuxDialogs, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            state: Rc::default(),
            dialogs: RefCell::new(dialogs),
            notify,
        }
    }
    /// Attaches the portal's dialogs to the editor window.
    pub(super) fn set_parent(&self, parent: String) {
        self.dialogs.borrow_mut().set_parent(parent);
    }
    pub(super) fn scope(&self, owner: Box<dyn Any>) -> Scope {
        let mut state = self.state.borrow_mut();
        if state.depth == 0 {
            state.consumed.clear();
        }
        state.depth += 1;
        Scope {
            state: Some(self.state.clone()),
            entered_with: state.consumed.len(),
            owner: Some(owner),
        }
    }
    /// The armed answer to `question`, or `None` when the caller must take its
    /// safe answer (the question is now open, or cannot be asked here).
    pub(super) fn ask(&self, question: Question) -> Option<Answer> {
        let mut state = self.state.borrow_mut();
        match state.armed.front() {
            Some((armed, _)) if armed.answers(&question) => {
                let (question, answer) = state.armed.pop_front()?;
                state.consumed.push((question, answer.clone()));
                return Some(answer);
            }
            // The run asked something other than what was answered (its state
            // changed between runs): the answer stays unused and is dropped
            // when the run settles; logged so a re-asked prompt is traceable.
            Some((armed, _)) => eprintln!(
                "event=prompt_answer_mismatch armed={} asked={}",
                armed.describe(),
                question.describe()
            ),
            None => {}
        }
        if state.open.is_some() || state.ready.is_some() {
            return None;
        }
        if state.depth == 0 {
            eprintln!("event=prompt_skipped reason=no-scope");
            return None;
        }
        state.chain = if state.consumed.is_empty() { 0 } else { state.chain + 1 };
        if state.chain > MAX_CHAIN {
            eprintln!("event=prompt_skipped reason=chain");
            return None;
        }
        let waiting = match &question {
            Question::Prompt(prompt) => Waiting::Prompt(prompt.clone()),
            Question::Dialog(request) => {
                Waiting::Dialog(self.dialogs.borrow().begin(request.clone(), self.notify.clone()))
            }
        };
        let earlier = state.consumed.clone();
        state.open = Some(Open {
            question: Some(question),
            waiting,
            earlier,
            owner: None,
        });
        state.unclaimed = true;
        None
    }
    /// Shows a message nobody waits for, unless another question is open.
    pub(super) fn notice(&self, prompt: InAppPrompt) {
        let mut state = self.state.borrow_mut();
        if state.open.is_none() && state.ready.is_none() {
            state.open = Some(Open {
                question: None,
                waiting: Waiting::Notice(prompt),
                earlier: Vec::new(),
                owner: None,
            });
        }
    }
    /// Picks up a dialog's answer once its worker delivered it.
    fn poll(&self) {
        let answer = {
            let mut state = self.state.borrow_mut();
            match state.open.as_mut().map(|open| &mut open.waiting) {
                Some(Waiting::Dialog(pending)) => pending.try_result(),
                _ => None,
            }
        };
        if let Some(result) = answer {
            self.arm(Answer::Paths(result));
        }
    }
    fn arm(&self, answer: Answer) {
        let mut state = self.state.borrow_mut();
        let Some(open) = state.open.take() else {
            return;
        };
        let (Some(question), Some(owner)) = (open.question, open.owner) else {
            // A notice, or a question no scope owns: nothing runs again.
            return;
        };
        state.armed = open.earlier.into_iter().collect();
        state.armed.push_back((question, answer));
        state.ready = Some(owner);
    }
    /// Whether a question is open or its answer still waits for its run. The
    /// shell keeps queued work (a close) queued meanwhile.
    pub(super) fn waiting(&self) -> bool {
        self.poll();
        let state = self.state.borrow();
        state.ready.is_some()
            || state
                .open
                .as_ref()
                .is_some_and(|open| !matches!(open.waiting, Waiting::Notice(_)))
    }
    /// The owner to run again now that its answers are armed.
    pub(super) fn take_ready(&self) -> Option<Box<dyn Any>> {
        self.poll();
        self.state.borrow_mut().ready.take()
    }
    /// Drops the answers the last run did not consume.
    pub(super) fn settle(&self) {
        let mut state = self.state.borrow_mut();
        if state.ready.is_none() {
            state.armed.clear();
        }
    }
    /// What the shell draws while a question is open.
    pub(super) fn view(&self) -> Option<PromptView> {
        self.poll();
        let state = self.state.borrow();
        Some(match &state.open.as_ref()?.waiting {
            Waiting::Prompt(prompt) | Waiting::Notice(prompt) => PromptView::from_prompt(prompt),
            Waiting::Dialog(_) => PromptView::waiting_for_dialog(CANCEL_ID),
        })
    }
    /// The button the person pressed in the shell's prompt. For a dialog this
    /// is "Stop waiting": the dialog counts as cancelled, and an answer it gives
    /// later is ignored.
    pub(super) fn answer(&self, id: i32) {
        let answer = {
            let mut state = self.state.borrow_mut();
            match state.open.as_ref().map(|open| &open.waiting) {
                Some(Waiting::Prompt(_)) => Some(Answer::Button(id)),
                Some(Waiting::Dialog(_)) => Some(Answer::Paths(Ok(Vec::new()))),
                Some(Waiting::Notice(_)) => {
                    state.open = None;
                    None
                }
                None => None,
            }
        };
        if let Some(answer) = answer {
            self.arm(answer);
        }
    }
}

impl PromptView {
    fn from_prompt(prompt: &InAppPrompt) -> Self {
        use bareline_platform_linux::prompts::PromptSeverity;
        Self {
            title: prompt.title.clone(),
            instruction: prompt.instruction.clone(),
            content: prompt.content.clone(),
            footer: prompt.footer.clone(),
            level: match prompt.severity {
                PromptSeverity::Information => PromptLevel::Information,
                PromptSeverity::Question => PromptLevel::Question,
                PromptSeverity::Warning => PromptLevel::Warning,
                PromptSeverity::Error => PromptLevel::Error,
            },
            buttons: prompt
                .buttons
                .iter()
                .map(|button| PromptButtonView {
                    id: button.id,
                    label: button.label.clone(),
                    access_key: button.access_key,
                })
                .collect(),
            default_id: prompt.default_id,
            cancel_id: prompt.cancel_id,
        }
    }
    /// Shown while a portal dialog is open in its own window: the editor stays
    /// modal, as it is behind a Windows file dialog, but keeps painting.
    fn waiting_for_dialog(stop: i32) -> Self {
        Self {
            title: "Bareline".into(),
            instruction: "Waiting for the file dialog".into(),
            content: "Choose a location in the dialog window. If it is hidden or does not appear, stop waiting to return to the editor."
                .into(),
            footer: None,
            level: PromptLevel::Information,
            buttons: vec![PromptButtonView {
                id: stop,
                label: "Stop waiting".into(),
                access_key: Some('s'),
            }],
            default_id: stop,
            cancel_id: stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform_linux::portal::{ChooserMethod, ChooserOptions, ChooserResponse, FileChooser, PortalError};
    use std::path::PathBuf;

    /// A portal FileChooser that answers every dialog with one file.
    struct Chooser(Result<ChooserResponse, PortalError>);
    impl FileChooser for Chooser {
        fn choose(
            &self,
            _method: ChooserMethod,
            _parent: &str,
            _title: &str,
            _options: &ChooserOptions,
        ) -> Result<ChooserResponse, PortalError> {
            self.0.clone()
        }
    }
    fn interactions(answer: Result<ChooserResponse, PortalError>) -> Interactions {
        Interactions::new(LinuxDialogs::with_chooser(Arc::new(Chooser(answer))), Arc::new(|| {}))
    }
    fn chosen(uri: &str) -> Result<ChooserResponse, PortalError> {
        Ok(ChooserResponse {
            code: 0,
            uris: vec![uri.into()],
        })
    }
    fn ready(interactions: &Interactions) -> Option<&'static str> {
        // The dialog answers on its worker; wait for it as the event loop would.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(owner) = interactions.take_ready() {
                return owner.downcast::<&'static str>().ok().map(|owner| *owner);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_question_outside_any_scope_takes_its_safe_answer_and_shows_nothing() {
        let interactions = interactions(chosen("file:///tmp/a.txt"));
        let save = Question::Prompt(InAppPrompt::save_document("a.txt"));
        assert_eq!(interactions.ask(save), None);
        assert!(interactions.view().is_none());
        assert!(!interactions.waiting());
    }

    #[test]
    fn a_prompt_answer_runs_its_owner_again_with_the_answer_armed() {
        let interactions = interactions(chosen("file:///tmp/a.txt"));
        let save = Question::Prompt(InAppPrompt::save_document("a.txt"));
        {
            let _scope = interactions.scope(Box::new("close"));
            assert_eq!(interactions.ask(save.clone()), None);
            // A second question in the same run waits its turn.
            assert_eq!(interactions.ask(Question::Prompt(InAppPrompt::stop_monitoring())), None);
        }
        assert!(interactions.waiting());
        let view = interactions.view().expect("the prompt is shown");
        assert!(view.buttons.iter().any(|button| button.label == "Don't Save"));
        interactions.answer(1102);
        assert_eq!(ready(&interactions), Some("close"));
        {
            let _scope = interactions.scope(Box::new("close"));
            assert_eq!(interactions.ask(save.clone()), Some(Answer::Button(1102)));
        }
        interactions.settle();
        assert!(!interactions.waiting());
        // Consumed answers are never reused.
        let _scope = interactions.scope(Box::new("close"));
        assert_eq!(interactions.ask(save), None);
    }

    #[test]
    fn a_dialog_after_an_answered_prompt_replays_both_answers_in_order() {
        let interactions = interactions(chosen("file:///tmp/b.txt"));
        let save = Question::Prompt(InAppPrompt::save_document("Untitled 1"));
        let dialog = Question::Dialog(DialogRequest::Open { multiple: false });
        {
            let _scope = interactions.scope(Box::new("close"));
            interactions.ask(save.clone());
        }
        interactions.answer(1101);
        assert_eq!(ready(&interactions), Some("close"));
        {
            let _scope = interactions.scope(Box::new("close"));
            assert_eq!(interactions.ask(save.clone()), Some(Answer::Button(1101)));
            assert_eq!(interactions.ask(dialog.clone()), None);
        }
        interactions.settle();
        assert_eq!(ready(&interactions), Some("close"));
        let _scope = interactions.scope(Box::new("close"));
        assert_eq!(interactions.ask(save), Some(Answer::Button(1101)));
        assert_eq!(
            interactions.ask(dialog),
            Some(Answer::Paths(Ok(vec![PathBuf::from("/tmp/b.txt")])))
        );
    }

    #[test]
    fn a_save_dialog_answer_survives_a_changed_default_folder() {
        use bareline_platform::{SaveDialogOptions, SaveFileKind};
        let interactions = interactions(chosen("file:///tmp/e.txt"));
        let first = Question::Dialog(DialogRequest::Save(SaveDialogOptions::new(SaveFileKind::Text)));
        {
            let _scope = interactions.scope(Box::new("save"));
            interactions.ask(first);
        }
        assert_eq!(ready(&interactions), Some("save"));
        let _scope = interactions.scope(Box::new("save"));
        let again = Question::Dialog(DialogRequest::Save(
            SaveDialogOptions::new(SaveFileKind::Text).in_directory(Some(PathBuf::from("/tmp"))),
        ));
        assert_eq!(
            interactions.ask(again),
            Some(Answer::Paths(Ok(vec![PathBuf::from("/tmp/e.txt")])))
        );
    }

    #[test]
    fn one_documents_save_dialog_answer_never_answers_anothers() {
        use bareline_platform::{SaveDialogOptions, SaveFileKind};
        let save = |name: &str| {
            Question::Dialog(DialogRequest::Save(
                SaveDialogOptions::new(SaveFileKind::Text).named(name),
            ))
        };
        let interactions = interactions(chosen("file:///tmp/x.txt"));
        {
            let _scope = interactions.scope(Box::new("save"));
            interactions.ask(save("Untitled 1"));
        }
        assert_eq!(ready(&interactions), Some("save"));
        {
            let _scope = interactions.scope(Box::new("save"));
            // Another document's dialog (its own suggested name) is asked
            // afresh instead of taking the location chosen for "Untitled 1".
            assert_eq!(interactions.ask(save("b.txt")), None);
        }
        interactions.settle();
        assert!(interactions.waiting(), "the other document's dialog is open");
        assert!(!save("a").answers(&Question::Dialog(DialogRequest::Save(
            SaveDialogOptions::new(SaveFileKind::Html).named("a")
        ))));
    }

    #[test]
    fn a_run_that_keeps_asking_something_new_stops() {
        let interactions = interactions(chosen("file:///tmp/f.txt"));
        let question = |index: usize| Question::Prompt(InAppPrompt::save_document(&format!("doc{index}")));
        let mut opened = 0;
        let mut refused = false;
        for round in 0..MAX_CHAIN + 4 {
            {
                let _scope = interactions.scope(Box::new("run"));
                for asked in 0..=round {
                    if interactions.ask(question(asked)).is_none() {
                        break;
                    }
                }
            }
            if interactions.view().is_none() {
                refused = true;
                break;
            }
            opened += 1;
            interactions.answer(1102);
            assert!(interactions.take_ready().is_some());
        }
        assert!(refused, "the chain ends");
        assert_eq!(opened, MAX_CHAIN + 1);
    }

    #[test]
    fn the_inner_scope_that_asked_first_owns_the_question() {
        let interactions = interactions(chosen("file:///tmp/c.txt"));
        {
            let _input = interactions.scope(Box::new("input"));
            let _command = interactions.scope(Box::new("command"));
            interactions.ask(Question::Dialog(DialogRequest::PickFolder));
        }
        assert_eq!(ready(&interactions), Some("command"));
    }

    #[test]
    fn no_portal_reports_unsupported_and_stop_waiting_cancels() {
        let interactions = interactions(Err(PortalError::Unavailable("no portal".into())));
        let dialog = Question::Dialog(DialogRequest::Open { multiple: true });
        {
            let _scope = interactions.scope(Box::new("open"));
            interactions.ask(dialog.clone());
        }
        assert_eq!(ready(&interactions), Some("open"));
        let _scope = interactions.scope(Box::new("open"));
        let Some(Answer::Paths(Err(message))) = interactions.ask(dialog) else {
            panic!("the dialog reports why it could not open");
        };
        assert!(message.starts_with("This system does not support"), "{message}");
        drop(_scope);
        interactions.settle();
        // A dialog the person stops waiting for answers "cancelled".
        let slow = interactions_waiting_forever();
        let folder = Question::Dialog(DialogRequest::PickFolder);
        {
            let _scope = slow.scope(Box::new("pick"));
            slow.ask(folder.clone());
        }
        assert!(slow.view().is_some_and(|view| view.buttons.len() == 1));
        slow.answer(CANCEL_ID);
        assert_eq!(
            slow.take_ready()
                .and_then(|owner| owner.downcast::<&str>().ok())
                .map(|o| *o),
            Some("pick")
        );
        let _scope = slow.scope(Box::new("pick"));
        assert_eq!(slow.ask(folder), Some(Answer::Paths(Ok(Vec::new()))));
    }

    /// A chooser that never answers within the test.
    fn interactions_waiting_forever() -> Interactions {
        struct Never;
        impl FileChooser for Never {
            fn choose(
                &self,
                _method: ChooserMethod,
                _parent: &str,
                _title: &str,
                _options: &ChooserOptions,
            ) -> Result<ChooserResponse, PortalError> {
                std::thread::sleep(std::time::Duration::from_secs(30));
                Err(PortalError::Failed("late".into()))
            }
        }
        Interactions::new(LinuxDialogs::with_chooser(Arc::new(Never)), Arc::new(|| {}))
    }

    #[test]
    fn a_notice_shows_without_an_owner_and_closes_on_any_button() {
        let interactions = interactions(chosen("file:///tmp/d.txt"));
        interactions.notice(InAppPrompt::operation_failed("disk full"));
        assert!(!interactions.waiting());
        assert!(
            interactions
                .view()
                .is_some_and(|view| view.content.contains("disk full"))
        );
        interactions.answer(1);
        assert!(interactions.view().is_none());
        assert!(interactions.take_ready().is_none());
    }
}
