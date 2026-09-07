// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::language::{LanguageController, LanguageEffect};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::controls::{Key as UiKey, UiEvent};

#[derive(Default)]
pub(super) struct LanguageRuntime {
    pub controller: LanguageController,
    restored: Option<bareline_document::DocumentSnapshot>,
    applied_definition: Option<std::sync::Arc<bareline_syntax::udl::Definition>>,
    definition_target: Option<bareline_document::DocumentSnapshot>,
    definition_editor: Option<bareline_document::DocumentSnapshot>,
    validated_revision: Option<u64>,
}
impl LanguageRuntime {
    pub fn draw(
        &mut self,
        _renderer: &mut WindowsRenderer,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        self.controller.draw(width, height, ops);
        Ok(None)
    }
}
impl Shell {
    pub(super) fn language_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "language.choose" => self.language.controller.choose_language(),
            "language.udl.import" => {
                self.language.definition_target = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.editors.get(self.app.active))
                    .map(|e| e.snapshot().clone());
                match self.platform.as_ref().unwrap().open_file() {
                    Ok(Some(path)) => self
                        .language
                        .controller
                        .import_udl(path, self.notify.clone()),
                    Ok(None) => {}
                    Err(error) => self.language.controller.status = error.to_string(),
                }
            }
            "language.udl.edit" => {
                if let Some(definition) = &self.language.controller.definition
                    && let Ok(text) = definition.to_json()
                    && self.ensure_workspace(el)
                {
                    let workspace = self.workspace.as_mut().unwrap();
                    if workspace.new_document().is_ok() {
                        self.app.tabs = workspace.titles();
                        self.app.active = self.app.tabs.len() - 1;
                        let editor = &mut workspace.editors[self.app.active];
                        editor.language_override = Some(bareline_syntax::Language::Json);
                        editor.enqueue(Input::Insert(text));
                        self.language.definition_editor = Some(editor.snapshot().clone());
                        self.language.validated_revision = None;
                        self.language.controller.close();
                    }
                }
            }
            "language.udl.export" => match self.platform.as_ref().unwrap().save_file() {
                Ok(Some(path)) => self.language.controller.export_definition(
                    path,
                    std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
                    self.notify.clone(),
                ),
                Ok(None) => {}
                Err(error) => self.language.controller.status = error.to_string(),
            },
            "language.udl.preview" => {
                if let Some(definition) = self.language.controller.definition.clone()
                    && self.ensure_workspace(el)
                {
                    let sample = format!(
                        "{}\n{}\n42 {}\n",
                        definition
                            .keywords
                            .iter()
                            .take(12)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" "),
                        definition
                            .line_comment
                            .as_ref()
                            .map_or(String::new(), |token| format!("{token} sample comment")),
                        definition.operators
                    );
                    let workspace = self.workspace.as_mut().unwrap();
                    if workspace.new_document().is_ok() {
                        self.app.tabs = workspace.titles();
                        self.app.active = self.app.tabs.len() - 1;
                        let editor = &mut workspace.editors[self.app.active];
                        editor.udl = Some(definition);
                        editor.enqueue(Input::Insert(sample));
                        self.language.controller.close();
                    }
                }
            }
            "editor.completion.show" => {
                if let Some(editor) = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.editors.get(self.app.active))
                {
                    if editor.paged() {
                        self.language.controller.open = true;
                        self.language.controller.status =
                            "Completion is unavailable for this paged text window".into();
                    } else {
                        self.language.controller.request_completion(
                            editor.snapshot().clone(),
                            editor.selection.caret,
                            editor.language,
                            self.notify.clone(),
                        );
                    }
                }
            }
            id if id.starts_with("view.fold.") => {
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    if id == "view.fold.unfoldAll" {
                        editor.unfold_all();
                    } else if id == "view.fold.toggleCurrent" {
                        editor.toggle_current_fold();
                    } else if !editor.paged() {
                        let level = id
                            .strip_prefix("view.fold.level")
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(1);
                        self.language.controller.request_folds(
                            editor.snapshot().clone(),
                            editor.language,
                            level,
                            self.notify.clone(),
                        );
                    }
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn language_pump(&mut self, _el: &ActiveEventLoop) {
        if !self.language.controller.busy()
            && let Some(identity) = &self.language.definition_editor
            && let Some(editor) = self.workspace.as_ref().and_then(|w| {
                w.editors
                    .iter()
                    .find(|e| e.snapshot().same_document(identity))
            })
            && !editor.busy()
            && self.language.validated_revision != Some(editor.snapshot().revision.0)
        {
            self.language.validated_revision = Some(editor.snapshot().revision.0);
            self.language
                .controller
                .validate_definition(editor.snapshot().clone(), self.notify.clone());
        }
        if let Some(editor) = self
            .workspace
            .as_ref()
            .and_then(|w| w.editors.get(self.app.active))
            && editor.has_pending_folds()
            && !editor.paged()
            && editor.language != bareline_syntax::Language::PlainText
            && !self.language.restored.as_ref().is_some_and(|s| {
                s.same_document(editor.snapshot()) && s.revision == editor.snapshot().revision
            })
        {
            self.language.restored = Some(editor.snapshot().clone());
            self.language.controller.request_folds(
                editor.snapshot().clone(),
                editor.language,
                1,
                self.notify.clone(),
            );
        }
        if !self.language.controller.poll() {
            return;
        }
        if self.language.definition_editor.is_some()
            && let Some(workspace) = &mut self.workspace
        {
            workspace.message = Some(self.language.controller.status.clone());
        }
        if let (Some(previous), Some(next)) = (
            &self.language.applied_definition,
            &self.language.controller.definition,
        ) && !std::sync::Arc::ptr_eq(previous, next)
            && let Some(workspace) = &mut self.workspace
        {
            for editor in &mut workspace.editors {
                if editor
                    .udl
                    .as_ref()
                    .is_some_and(|definition| std::sync::Arc::ptr_eq(previous, definition))
                {
                    editor.udl = Some(next.clone());
                }
            }
        }
        if let Some(definition) = &self.language.controller.definition
            && !self
                .language
                .applied_definition
                .as_ref()
                .is_some_and(|old| std::sync::Arc::ptr_eq(old, definition))
            && let Some(editor) = self.workspace.as_mut().and_then(|w| {
                if let Some(target) = &self.language.definition_target {
                    w.editors
                        .iter_mut()
                        .find(|e| e.snapshot().same_document(target))
                } else {
                    w.editors.get_mut(self.app.active)
                }
            })
        {
            editor.udl = Some(definition.clone());
            self.language.applied_definition = Some(definition.clone());
        }
        if let Some((snapshot, folds, partial)) = self.language.controller.folds.take()
            && let Some(editor) = self
                .workspace
                .as_mut()
                .and_then(|w| w.editors.get_mut(self.app.active))
            && editor.snapshot().same_document(&snapshot)
            && editor.snapshot().revision == snapshot.revision
        {
            editor.set_known_folds(folds, self.language.controller.fold_level, partial);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn language_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !self.language.controller.open || self.palette.open {
            return false;
        }
        let ui = match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => Some(UiEvent::PointerDown(self.pointer)),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    Key::Named(NamedKey::ArrowUp) => Some(UiEvent::Key(UiKey::Up)),
                    Key::Named(NamedKey::ArrowDown) => Some(UiEvent::Key(UiKey::Down)),
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) => {
                        Some(UiEvent::Key(UiKey::Enter))
                    }
                    Key::Named(NamedKey::Escape) => Some(UiEvent::Key(UiKey::Escape)),
                    _ => {
                        self.language.controller.close();
                        return false;
                    }
                }
            }
            _ => None,
        };
        let Some(ui) = ui else {
            return false;
        };
        let effect = self.language.controller.event(ui);
        let accepted = effect.is_some();
        if let Some(effect) = effect
            && let Some(editor) = self
                .workspace
                .as_mut()
                .and_then(|w| w.editors.get_mut(self.app.active))
        {
            match effect {
                LanguageEffect::Choose(language) => {
                    editor.udl = None;
                    editor.language_override = Some(language);
                    editor.language = language;
                }
                LanguageEffect::Accept(index) => {
                    let result = self
                        .language
                        .controller
                        .accept(editor.snapshot(), &editor.selection_set(), index)
                        .and_then(|edit| editor.apply_power(edit));
                    if let Err(error) = result {
                        self.language.controller.status = error;
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        // Enter/Tab without an active suggestion retains ordinary editor semantics.
        accepted || !matches!(ui, UiEvent::Key(UiKey::Enter))
    }
}
