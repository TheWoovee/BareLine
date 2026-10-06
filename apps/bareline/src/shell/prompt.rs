// SPDX-License-Identifier: MPL-2.0
//! Prompts the shell draws itself, and the replay that lets a prompt or a file
//! dialog answer later.
//!
//! Where the system has no modal dialog the editor can wait on (Linux: the
//! portal's dialogs are another process's windows, and message prompts are
//! drawn here), a question asked inside a [`Replay`] scope opens and returns its
//! safe answer at once; once the person answers, the scope runs again and gets
//! the answer (see `native::interaction_scope`). Meanwhile the question is this
//! modal: it keeps the editor inert as a native modal dialog would, and the
//! event loop keeps painting. On Windows and macOS every prompt is a native
//! modal dialog, the seam never returns a view, and nothing here shows.
use super::*;
use crate::shell::native::{PromptLevel, PromptView};
use bareline_renderer::{DrawOp, Rect, TextBackend};
use bareline_ui::theme::{ToastLevel, UiTheme};

pub(super) const PROMPT_GROUP_ID: u64 = 91_000_300;
pub(super) const PROMPT_TEXT_ID: u64 = 91_000_301;
/// Buttons take consecutive ids from here, in display order.
pub(super) const PROMPT_BUTTON_ID: u64 = 91_000_310;
const PANEL_WIDTH: f32 = 520.0;
const PADDING: f32 = 20.0;
const BUTTON_HEIGHT: f32 = 30.0;
const BUTTON_GAP: f32 = 8.0;

/// What the shell runs again once a prompt or dialog started inside it has
/// its answer.
pub(super) enum Replay {
    /// A command, which asks again and receives the armed answers.
    Action(Action),
    /// The queued close or exit, drained again.
    Close,
    /// The save pipeline's pump, for Save All's destinations.
    Lifecycle,
    /// A click or key the shell handled directly (a banner button, a picker),
    /// with the pointer and modifiers it had.
    Input {
        window: WindowId,
        event: WindowEvent,
        pointer: Point,
        modifiers: ModifiersState,
    },
}

/// What assistive technology reads of the prompt: its title, its complete
/// text and where it is drawn, and each button's id, label and bounds.
pub(super) struct PromptSemantics {
    pub name: String,
    pub text: String,
    pub text_bounds: Rect,
    pub buttons: Vec<(u64, String, Rect)>,
}

/// The question on screen and where its buttons were drawn.
#[derive(Default)]
pub(super) struct PromptRuntime {
    view: Option<PromptView>,
    focus: usize,
    bounds: Rect,
    text_bounds: Rect,
    buttons: Vec<Rect>,
}

impl PromptRuntime {
    fn button_id(&self, index: usize) -> Option<i32> {
        self.view.as_ref()?.buttons.get(index).map(|button| button.id)
    }
    /// The button for an access key (the underlined letter of a Windows
    /// dialog button).
    fn access_key(&self, typed: &str) -> Option<i32> {
        let mut characters = typed.chars();
        let (Some(typed), None) = (characters.next(), characters.next()) else {
            return None;
        };
        let typed = typed.to_lowercase().next()?;
        self.view
            .as_ref()?
            .buttons
            .iter()
            .find(|button| button.access_key == Some(typed))
            .map(|button| button.id)
    }
    fn move_focus(&mut self, backwards: bool) {
        let count = self.view.as_ref().map_or(0, |view| view.buttons.len());
        if count > 0 {
            self.focus = if backwards {
                (self.focus + count - 1) % count
            } else {
                (self.focus + 1) % count
            };
        }
    }
    /// The accessibility id of the focused button.
    pub(super) fn focused_id(&self) -> u64 {
        PROMPT_BUTTON_ID + self.focus as u64
    }
    /// The prompt's name, its complete text and its buttons, for assistive
    /// technology.
    pub(super) fn semantics(&self) -> Option<PromptSemantics> {
        let view = self.view.as_ref()?;
        let text = [view.instruction.as_str(), view.content.as_str()]
            .into_iter()
            .chain(view.footer.as_deref())
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let buttons = view
            .buttons
            .iter()
            .enumerate()
            .map(|(index, button)| {
                (
                    PROMPT_BUTTON_ID + index as u64,
                    button.label.clone(),
                    self.buttons.get(index).copied().unwrap_or_default(),
                )
            })
            .collect();
        Some(PromptSemantics {
            name: view.title.clone(),
            text,
            text_bounds: self.text_bounds,
            buttons,
        })
    }

    pub(super) fn draw(
        &mut self,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        theme: UiTheme,
        ops: &mut Vec<DrawOp>,
    ) {
        let Some(view) = &self.view else {
            self.buttons.clear();
            return;
        };
        let palette = theme.toast(match view.level {
            PromptLevel::Information | PromptLevel::Question => ToastLevel::Info,
            PromptLevel::Warning => ToastLevel::Warning,
            PromptLevel::Error => ToastLevel::Error,
        });
        let panel_width = (width - 32.0).clamp(80.0, PANEL_WIDTH).min(width.max(0.0));
        let text_width = (panel_width - 2.0 * PADDING).max(20.0);
        let instruction = if view.instruction.is_empty() {
            Vec::new()
        } else {
            super::toast::wrap_measured(renderer, &view.instruction, 16.0, text_width)
        };
        let content = if view.content.is_empty() {
            Vec::new()
        } else {
            super::toast::wrap_measured(renderer, &view.content, 13.0, text_width)
        };
        let footer = view
            .footer
            .as_deref()
            .map(|footer| super::toast::wrap_measured(renderer, footer, 11.0, text_width))
            .unwrap_or_default();
        let lines = instruction.len() as f32 * 22.0 + content.len() as f32 * 18.0 + footer.len() as f32 * 15.0;
        let wanted = PADDING + 22.0 + lines + 12.0 + BUTTON_HEIGHT + PADDING;
        let panel_height = wanted.min((height - 16.0).max(BUTTON_HEIGHT + 2.0 * PADDING));
        let bounds = bareline_ui::rect(
            (width - panel_width).max(0.0) / 2.0,
            (height - panel_height).max(0.0) / 2.0,
            panel_width,
            panel_height,
        );
        self.bounds = bounds;
        ops.push(DrawOp::FillRounded(bounds, palette.surface, 8.0));
        ops.push(DrawOp::StrokeRounded(bounds, palette.accent, 8.0, 2.0));
        let buttons_top = bounds.y + panel_height - PADDING - BUTTON_HEIGHT;
        let text_top = bounds.y + PADDING;
        self.text_bounds = bareline_ui::rect(
            bounds.x + PADDING,
            text_top,
            text_width,
            (buttons_top - text_top - 8.0).max(0.0),
        );
        ops.push(DrawOp::PushClip(self.text_bounds));
        bareline_ui::text(ops, bounds.x + PADDING, text_top, &view.title, 12.0, palette.muted);
        let mut y = text_top + 22.0;
        for line in &instruction {
            bareline_ui::text(ops, bounds.x + PADDING, y, line, 16.0, palette.text);
            y += 22.0;
        }
        for line in &content {
            bareline_ui::text(ops, bounds.x + PADDING, y, line, 13.0, palette.text);
            y += 18.0;
        }
        if !footer.is_empty() {
            y += 4.0;
        }
        for line in &footer {
            bareline_ui::text(ops, bounds.x + PADDING, y, line, 11.0, palette.muted);
            y += 15.0;
        }
        ops.push(DrawOp::PopClip);
        // Buttons in display order, right-aligned, as a dialog's command row.
        let widths: Vec<f32> = view
            .buttons
            .iter()
            .map(|button| {
                renderer
                    .measure_text(&button.label, 13.0)
                    .map_or(button.label.chars().count() as f32 * 7.5, |measured| measured.0)
                    .max(48.0)
                    + 28.0
            })
            .collect();
        let total = widths.iter().sum::<f32>() + BUTTON_GAP * widths.len().saturating_sub(1) as f32;
        let mut x = (bounds.x + panel_width - PADDING - total).max(bounds.x + PADDING);
        self.buttons.clear();
        for (index, (button, button_width)) in view.buttons.iter().zip(widths).enumerate() {
            let rect = bareline_ui::rect(x, buttons_top, button_width, BUTTON_HEIGHT);
            let focused = index == self.focus;
            ops.push(DrawOp::FillRounded(
                rect,
                if focused { palette.accent } else { palette.surface },
                5.0,
            ));
            ops.push(DrawOp::StrokeRounded(
                rect,
                if focused { palette.accent } else { palette.border },
                5.0,
                if focused { 2.0 } else { 1.0 },
            ));
            bareline_ui::text(
                ops,
                rect.x + 14.0,
                rect.y + 7.0,
                &button.label,
                13.0,
                if focused { palette.surface } else { palette.text },
            );
            self.buttons.push(rect);
            x += button_width + BUTTON_GAP;
        }
    }
}

impl Shell {
    /// Shows the question the seam has open, or retires the modal once it was
    /// answered.
    pub(super) fn prompt_sync(&mut self) {
        let view = native::in_app_prompt(self.platform.as_ref());
        let showing = self
            .modal
            .is_some_and(|modal| modal.surface == modal::ModalSurface::Prompt);
        match view {
            Some(view) => {
                let changed = self.prompt.view.as_ref() != Some(&view);
                if changed {
                    eprintln!(
                        "event=prompt_shown title={:?} instruction={:?} buttons={:?}",
                        view.title,
                        view.instruction,
                        view.buttons
                            .iter()
                            .map(|button| button.label.as_str())
                            .collect::<Vec<_>>()
                    );
                    self.prompt.focus = view
                        .buttons
                        .iter()
                        .position(|button| button.id == view.default_id)
                        .unwrap_or(0);
                    self.prompt.view = Some(view);
                }
                if !showing {
                    self.activate_modal(modal::ModalSurface::Prompt);
                    self.modal_focus(modal::ModalSurface::Prompt, self.prompt.focused_id());
                } else if changed {
                    self.modal_changed(modal::ModalSurface::Prompt);
                }
                if (changed || !showing)
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
            }
            None => {
                let stale = self.prompt.view.take().is_some();
                if showing {
                    self.dismiss_modal(modal::ModalSurface::Prompt);
                }
                if (stale || showing)
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
            }
        }
    }
    /// The modal closed by other means (a window close, another modal): the
    /// open question is cancelled so nothing waits for it.
    pub(super) fn prompt_dismissed(&mut self) {
        if let Some(view) = self.prompt.view.take() {
            native::answer_prompt(self.platform.as_ref(), view.cancel_id);
        }
    }
    fn prompt_answer(&mut self, id: i32) {
        eprintln!("event=prompt_answered id={id}");
        native::answer_prompt(self.platform.as_ref(), id);
        self.prompt_sync();
    }
    /// Focus from assistive technology.
    pub(super) fn prompt_focus(&mut self, id: u64) {
        if let Some(index) = id
            .checked_sub(PROMPT_BUTTON_ID)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| self.prompt.button_id(*index).is_some())
        {
            self.prompt.focus = index;
            self.modal_focus(modal::ModalSurface::Prompt, id);
        }
    }
    /// A button pressed by assistive technology.
    pub(super) fn prompt_invoke(&mut self, id: u64) {
        if let Some(answer) = id
            .checked_sub(PROMPT_BUTTON_ID)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.prompt.button_id(index))
        {
            self.prompt_answer(answer);
        }
    }
    /// Keys and clicks while the prompt is up. Other input stays inert (the
    /// modal swallows it); window events such as redraws and resizes go on.
    pub(super) fn prompt_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let answer = match &event.logical_key {
                    Key::Named(NamedKey::Escape) => self.prompt.view.as_ref().map(|view| view.cancel_id),
                    Key::Named(NamedKey::Enter) => self.prompt.button_id(self.prompt.focus),
                    Key::Character(value) if value == " " => self.prompt.button_id(self.prompt.focus),
                    Key::Named(NamedKey::Tab) => {
                        self.prompt.move_focus(self.modifiers.shift_key());
                        None
                    }
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                        self.prompt.move_focus(true);
                        None
                    }
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                        self.prompt.move_focus(false);
                        None
                    }
                    Key::Character(value) if !self.modifiers.control_key() => self.prompt.access_key(value),
                    _ => None,
                };
                match answer {
                    Some(id) => self.prompt_answer(id),
                    None => {
                        let focused = self.prompt.focused_id();
                        self.modal_focus(modal::ModalSurface::Prompt, focused);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(index) = self.prompt.buttons.iter().position(|rect| rect.contains(self.pointer))
                    && let Some(id) = self.prompt.button_id(index)
                {
                    self.prompt_answer(id);
                }
            }
            _ => return false,
        }
        true
    }
    pub(super) fn prompt_contains(&self, point: Point) -> bool {
        self.prompt.bounds.contains(point)
    }
    pub(super) fn draw_prompt(
        &mut self,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
        let theme = self.settings.ui_theme();
        self.prompt.draw(renderer, width, height, theme, ops);
    }
    /// Runs a scope again once the prompt or dialog it opened has its answer,
    /// then drops answers that run did not use, and shows the next question.
    pub(super) fn interactions_poll(&mut self, el: &ActiveEventLoop) {
        if let Some(replay) = native::interaction_replay::<Replay>(self.platform.as_ref()) {
            // The answered prompt's modal goes first: commands refuse to run
            // behind a modal.
            self.prompt_sync();
            eprintln!(
                "event=interaction_replay run={}",
                match &replay {
                    Replay::Action(_) => "command",
                    Replay::Close => "close",
                    Replay::Lifecycle => "save-all",
                    Replay::Input { .. } => "input",
                }
            );
            match replay {
                Replay::Action(action) => self.dispatch(el, action),
                Replay::Close => self.drain_pending_close(el),
                Replay::Lifecycle => self.lifecycle_pump(el),
                Replay::Input {
                    window,
                    event,
                    pointer,
                    modifiers,
                } => {
                    self.pointer = pointer;
                    self.modifiers = modifiers;
                    let again = event.clone();
                    let _scope = native::interaction_scope(self.platform.as_ref(), move || Replay::Input {
                        window,
                        event: again,
                        pointer,
                        modifiers,
                    });
                    self.window_event(el, window, event);
                }
            }
            native::interaction_settle(self.platform.as_ref());
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        self.prompt_sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::native::PromptButtonView;
    use bareline_renderer_recording::RecordingBackend;

    fn view() -> PromptView {
        let button = |id, label: &str, key| PromptButtonView {
            id,
            label: label.into(),
            access_key: Some(key),
        };
        PromptView {
            title: "Bareline".into(),
            instruction: "Save changes to notes.txt?".into(),
            content: "Your changes will be lost if you don't save them.".into(),
            footer: None,
            level: PromptLevel::Question,
            buttons: vec![
                button(1101, "Save", 's'),
                button(1102, "Don't Save", 'n'),
                button(2, "Cancel", 'c'),
            ],
            default_id: 1101,
            cancel_id: 2,
        }
    }

    #[test]
    fn the_prompt_draws_every_button_inside_its_panel_and_keys_choose_them() {
        let mut prompt = PromptRuntime {
            view: Some(view()),
            ..Default::default()
        };
        let mut ops = Vec::new();
        prompt.draw(
            &mut RecordingBackend::default(),
            800.0,
            600.0,
            UiTheme::default(),
            &mut ops,
        );
        assert_eq!(prompt.buttons.len(), 3);
        for button in &prompt.buttons {
            assert!(prompt.bounds.contains(Point {
                x: button.x,
                y: button.y
            }));
            assert!(button.x + button.width <= prompt.bounds.x + prompt.bounds.width);
        }
        assert!(prompt.buttons.windows(2).all(|pair| pair[0].x < pair[1].x));
        let text: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                DrawOp::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        for expected in ["Save changes to notes.txt?", "Save", "Don't Save", "Cancel"] {
            assert!(text.contains(&expected), "{expected} is drawn: {text:?}");
        }
        assert_eq!(prompt.access_key("N"), Some(1102));
        assert_eq!(prompt.access_key("x"), None);
        assert_eq!(prompt.button_id(prompt.focus), Some(1101));
        prompt.move_focus(true);
        assert_eq!(prompt.button_id(prompt.focus), Some(2));
        prompt.move_focus(false);
        prompt.move_focus(false);
        assert_eq!(prompt.button_id(prompt.focus), Some(1102));
        let semantics = prompt.semantics().unwrap();
        assert_eq!(semantics.name, "Bareline");
        assert!(semantics.text.starts_with("Save changes to notes.txt?"));
        assert_eq!(semantics.buttons[1].0, PROMPT_BUTTON_ID + 1);
    }

    #[test]
    fn a_narrow_window_keeps_the_panel_on_screen() {
        let mut prompt = PromptRuntime {
            view: Some(view()),
            ..Default::default()
        };
        prompt.draw(
            &mut RecordingBackend::default(),
            200.0,
            120.0,
            UiTheme::default(),
            &mut Vec::new(),
        );
        assert!(prompt.bounds.x >= 0.0 && prompt.bounds.y >= 0.0);
        assert!(prompt.bounds.width <= 200.0 && prompt.bounds.height <= 120.0);
    }

    /// Closing a document with unsaved changes asks Save / Don't Save / Cancel
    /// in the shell's own prompt and honours Don't Save (Linux).
    #[cfg(target_os = "linux")]
    #[test]
    fn closing_a_dirty_document_asks_in_the_shell_and_honours_dont_save() {
        use super::super::{CloseTarget, PendingClose, accessibility::tests::headless_shell};
        use bareline_app::workspace::{Input, Workspace};
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(crate::shell::native::FileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("unsaved".into()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !workspace.editors[0].dirty() || workspace.editors[0].busy() {
            workspace.pump();
            assert!(Instant::now() < deadline, "the edit never landed");
            std::thread::yield_now();
        }
        let identity = workspace.editors[0].document_identity();
        let mut shell = headless_shell();
        shell.app.tabs = workspace.titles();
        shell.workspace = Some(workspace);
        shell.platform = Some(crate::shell::native::Platform::for_tests());
        let target = CloseTarget {
            index: 0,
            identity,
            tab: None,
            saving: false,
            discarding: false,
            was_read_only: false,
            deferred: true,
        };
        let mut renderer = RecordingBackend::default();
        {
            let _scope = native::interaction_scope(shell.platform.as_ref(), || Replay::Close);
            shell.close_document_with_renderer(target, &mut renderer);
        }
        // The prompt is open; the close stays queued and nothing was closed.
        assert!(matches!(shell.pending_close, Some(PendingClose::Document(_))));
        assert_eq!(shell.workspace.as_ref().unwrap().editors.len(), 2);
        shell.prompt_sync();
        assert_eq!(
            shell.modal.map(|modal| modal.surface),
            Some(modal::ModalSurface::Prompt)
        );
        let labels: Vec<_> = shell
            .prompt
            .view
            .as_ref()
            .unwrap()
            .buttons
            .iter()
            .map(|b| b.label.clone())
            .collect();
        assert_eq!(labels, ["Save", "Don't Save", "Cancel"]);
        // "n" is Don't Save's access key.
        let dont_save = shell.prompt.access_key("n").unwrap();
        shell.prompt_answer(dont_save);
        assert!(shell.modal.is_none());
        let Some(Replay::Close) = native::interaction_replay::<Replay>(shell.platform.as_ref()) else {
            panic!("the queued close runs again");
        };
        let Some(PendingClose::Document(target)) = shell.pending_close.take() else {
            panic!("the close is still queued");
        };
        {
            let _scope = native::interaction_scope(shell.platform.as_ref(), || Replay::Close);
            shell.close_document_with_renderer(target, &mut renderer);
        }
        native::interaction_settle(shell.platform.as_ref());
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(PendingClose::Document(target)) = shell.pending_close.take() {
            shell.workspace.as_mut().unwrap().pump();
            shell.close_document_with_renderer(target, &mut renderer);
            assert!(Instant::now() < deadline, "the discarded document never closed");
            std::thread::yield_now();
        }
        let workspace = shell.workspace.as_ref().unwrap();
        assert_eq!(workspace.editors.len(), 1, "Don't Save closed the document");
        assert_ne!(workspace.editors[0].document_identity().0, identity.0);
    }
}
