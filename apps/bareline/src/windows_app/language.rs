// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_app::language::{LanguageConfiguration, LanguageController, LanguageEffect};
use bareline_renderer::{DrawOp, LayoutError, Rect};
use bareline_ui::controls::{Key as UiKey, UiEvent};

#[derive(Default)]
pub(super) struct LanguageRuntime {
    pub controller: LanguageController,
    completion_source: Option<((u64, u64), usize)>,
    restored: Option<bareline_document::DocumentSnapshot>,
    restored_language: Option<bareline_syntax::Language>,
    restored_definition: Option<std::sync::Arc<bareline_syntax::udl::Definition>>,
    restored_preference: bareline_syntax::LexerPreference,
    applied_definition: Option<std::sync::Arc<bareline_syntax::udl::Definition>>,
    definition_target: Option<bareline_document::DocumentSnapshot>,
    definition_editor: Option<bareline_document::DocumentSnapshot>,
    validated_revision: Option<u64>,
    detection:
        Option<std::sync::mpsc::Receiver<Result<((u64, u64), bareline_syntax::Language), String>>>,
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
            "language.signatures.import" => match self.platform.as_ref().unwrap().open_file() {
                Ok(Some(path)) => self
                    .language
                    .controller
                    .import_signatures(path, self.notify.clone()),
                Ok(None) => (),
                Err(error) => self.language.controller.status = error.to_string(),
            },
            "editor.completion.show" => {
                if let Some(workspace) = &self.workspace
                    && let Some(editor) = workspace.editors.get(self.app.active)
                {
                    let syntax = workspace
                        .syntax_result()
                        .filter(|syntax| syntax.is_current(editor.snapshot()))
                        .cloned();
                    if editor.paged() && syntax.is_none() {
                        self.language.controller.open = true;
                        self.language.controller.status =
                            "Syntax for this source window is still being prepared".into();
                    } else {
                        self.language.completion_source = Some((
                            bareline_app::accessibility::source_identity(editor),
                            match editor {
                                bareline_app::workspace::WorkspaceEditor::Paged(paged) => {
                                    paged.viewport_start().0
                                }
                                _ => 0,
                            },
                        ));
                        let documents = workspace
                            .editors
                            .iter()
                            .filter(|other| !other.paged())
                            .take(8)
                            .map(|other| other.snapshot().clone())
                            .collect();
                        self.language.controller.request_completion_configured(
                            editor.snapshot().clone(),
                            editor.selection.caret,
                            editor.language,
                            self.notify.clone(),
                            LanguageConfiguration {
                                policy: self.settings.effective().language_policy(
                                    editor
                                        .udl
                                        .as_ref()
                                        .map_or(editor.language.metadata().id, |definition| {
                                            definition.id.as_str()
                                        }),
                                ),
                                definition: editor.udl.clone(),
                            },
                            documents,
                            syntax,
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
                        self.language.restored = Some(editor.snapshot().clone());
                        self.language.restored_language = Some(editor.language);
                        self.language.restored_definition = editor.udl.clone();
                        self.language.restored_preference = editor.syntax_preference;
                        let level = id
                            .strip_prefix("view.fold.level")
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(1);
                        self.language.controller.request_folds_configured(
                            editor.snapshot().clone(),
                            editor.language,
                            level,
                            self.notify.clone(),
                            LanguageConfiguration {
                                policy: self.settings.effective().language_policy(
                                    editor
                                        .udl
                                        .as_ref()
                                        .map_or(editor.language.metadata().id, |definition| {
                                            definition.id.as_str()
                                        }),
                                ),
                                definition: editor.udl.clone(),
                            },
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
        if let Some(result) = self
            .language
            .detection
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.language.detection = None;
            match result {
                Ok((source, language)) => {
                    if let Some(editor) = self.workspace.as_mut().and_then(|w| {
                        w.editors.iter_mut().find(|editor| {
                            bareline_app::accessibility::source_identity(editor) == source
                        })
                    }) {
                        editor.detected_language = Some(language);
                    }
                }
                Err(error) => self.language.controller.status = error,
            }
        }
        if self.language.detection.is_none()
            && let Some(workspace) = &self.workspace
            && let Some(editor) = workspace.editors.get(self.app.active)
            && editor.detected_language.is_none()
            && editor.language_override.is_none()
        {
            let source = editor.snapshot().clone();
            let identity = bareline_app::accessibility::source_identity(editor);
            let paged = match editor {
                bareline_app::workspace::WorkspaceEditor::Paged(paged) => Some(paged.read_handle()),
                _ => None,
            };
            let associations = self.settings.effective().language_associations.clone();
            let path = workspace
                .path(self.app.active)
                .map_or_else(std::path::PathBuf::new, std::path::Path::to_path_buf);
            let notify = self.notify.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            self.language.detection = Some(rx);
            if let Err(error) = std::thread::Builder::new()
                .name("bareline-language-detect".into())
                .spawn(move || {
                    let result = (|| {
                        let association = associations.iter().find_map(|(pattern, id)| {
                            let name = path.file_name()?.to_str()?;
                            let matched = pattern.eq_ignore_ascii_case(name)
                                || pattern.strip_prefix("*.").is_some_and(|extension| {
                                    path.extension()
                                        .and_then(|value| value.to_str())
                                        .is_some_and(|actual| {
                                            actual.eq_ignore_ascii_case(extension)
                                        })
                                });
                            matched
                                .then(|| bareline_syntax::Language::from_id(id))
                                .flatten()
                        });
                        if let Some(handle) = paged {
                            let prefix = read_detection_window(&handle, 0)?;
                            let suffix = read_detection_window(
                                &handle,
                                handle.snapshot().len().saturating_sub(8192),
                            )?;
                            return Ok((
                                identity,
                                bareline_syntax::Language::detect_with_regions(
                                    &path,
                                    &prefix,
                                    &suffix,
                                    None,
                                    association,
                                ),
                            ));
                        }
                        let mut end = source.len().min(8192);
                        while !source.is_boundary(bareline_document::TextOffset(end)) {
                            end -= 1;
                        }
                        let mut start = source.len().saturating_sub(8192);
                        while !source.is_boundary(bareline_document::TextOffset(start)) {
                            start += 1;
                        }
                        let prefix = source
                            .read(
                                bareline_document::TextOffset(0)
                                    ..bareline_document::TextOffset(end),
                                8192,
                            )
                            .map_err(|e| format!("{e:?}"))?;
                        let suffix = source
                            .read(
                                bareline_document::TextOffset(start)
                                    ..bareline_document::TextOffset(source.len()),
                                8192,
                            )
                            .map_err(|e| format!("{e:?}"))?;
                        let language = bareline_syntax::Language::detect_with_regions(
                            &path,
                            &prefix,
                            &suffix,
                            None,
                            association,
                        );
                        Ok((identity, language))
                    })();
                    let _ = tx.send(result);
                    notify();
                })
            {
                self.language.detection = None;
                self.language.controller.status = error.to_string();
            }
        }
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
            && !editor.paged()
            && (editor.language != bareline_syntax::Language::PlainText || editor.udl.is_some())
            && !self.language.restored.as_ref().is_some_and(|s| {
                s.same_document(editor.snapshot())
                    && s.revision == editor.snapshot().revision
                    && self.language.restored_language == Some(editor.language)
                    && self.language.restored_preference == editor.syntax_preference
                    && match (&self.language.restored_definition, &editor.udl) {
                        (None, None) => true,
                        (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                        _ => false,
                    }
            })
        {
            self.language.restored = Some(editor.snapshot().clone());
            self.language.restored_language = Some(editor.language);
            self.language.restored_definition = editor.udl.clone();
            self.language.restored_preference = editor.syntax_preference;
            self.language.controller.request_folds_configured(
                editor.snapshot().clone(),
                editor.language,
                0,
                self.notify.clone(),
                LanguageConfiguration {
                    policy: self.settings.effective().language_policy(
                        editor
                            .udl
                            .as_ref()
                            .map_or(editor.language.metadata().id, |definition| {
                                definition.id.as_str()
                            }),
                    ),
                    definition: editor.udl.clone(),
                },
            );
        }
        let definition_snapshot = self
            .language
            .definition_editor
            .as_ref()
            .and_then(|identity| {
                self.workspace
                    .as_ref()?
                    .editors
                    .iter()
                    .find(|editor| editor.snapshot().same_document(identity))
                    .map(|editor| editor.snapshot().clone())
            });
        if !self
            .language
            .controller
            .poll_definition(definition_snapshot.as_ref())
        {
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
        if let Some(effect) = effect {
            self.language_apply_effect(effect);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        // Enter/Tab without an active suggestion retains ordinary editor semantics.
        accepted || !matches!(ui, UiEvent::Key(UiKey::Enter))
    }
    fn language_apply_effect(&mut self, effect: LanguageEffect) {
        let Some(editor) = self
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.editors.get_mut(self.app.active))
        else {
            return;
        };
        match effect {
            LanguageEffect::ChooseDefinition(definition) => {
                self.language.definition_target = Some(editor.snapshot().clone());
                self.language.applied_definition = Some(definition.clone());
                self.language.controller.definition = Some(definition.clone());
                editor.udl = Some(definition);
            }
            LanguageEffect::Choose(language) => {
                editor.udl = None;
                editor.language_override = Some(language);
                editor.language = language;
            }
            LanguageEffect::Accept(index) => {
                let origin = match editor {
                    bareline_app::workspace::WorkspaceEditor::Paged(paged) => {
                        paged.viewport_start().0
                    }
                    _ => 0,
                };
                if self.language.completion_source
                    != Some((bareline_app::accessibility::source_identity(editor), origin))
                {
                    self.language.controller.close();
                    self.language.controller.status = "Completion source changed".into();
                    return;
                }
                if let bareline_app::workspace::WorkspaceEditor::Paged(paged) = editor {
                    let Some(result) = self
                        .language
                        .controller
                        .completion
                        .as_ref()
                        .filter(|result| result.is_current(paged.surface.snapshot()))
                    else {
                        return;
                    };
                    let Some(item) = result.items.get(index) else {
                        return;
                    };
                    if paged.busy()
                        || paged.surface.selection.anchor != paged.surface.selection.caret
                        || paged.surface.selection.caret != result.replacement.end.0
                        || paged.surface.selection_set().selections.len() != 1
                    {
                        return;
                    }
                    let text = item.text.clone();
                    paged.surface.selection.anchor = result.replacement.start.0;
                    paged.enqueue(Input::Insert(text));
                    self.language.controller.close();
                } else {
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
    }
    pub(super) fn language_accessibility_nodes(
        &self,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        self.language.controller.accessibility_nodes()
    }
    pub(super) fn language_accessibility_focus(&self) -> Option<u64> {
        self.language.controller.accessibility_focus()
    }
    pub(super) fn language_accessibility(
        &mut self,
        _el: &ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            _ => return false,
        };
        if !self
            .language
            .controller
            .accessibility_nodes()
            .iter()
            .any(|node| node.id == id && (node.focusable || node.invokable))
        {
            return false;
        }
        if let Some(effect) = self.language.controller.accessibility_select(id, invoke) {
            self.language_apply_effect(effect);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

fn read_detection_window(
    handle: &bareline_editor_surface::paged_view::PagedReadHandle,
    start: usize,
) -> Result<String, String> {
    use bareline_document::{Budget, TextOffset, paged::WindowPoll};
    let mut request = handle
        .snapshot()
        .begin_viewport(TextOffset(start), 8192, &Budget::new(8192))
        .map_err(|e| format!("{e:?}"))?;
    for _ in 0..4096 {
        match request.poll() {
            WindowPoll::Ready(window) => return Ok(window.text().to_owned()),
            WindowPoll::Pending(ticket) => {
                if !handle.resolve_page(ticket)? {
                    std::thread::yield_now();
                }
            }
            _ => return Err("Language detection source unavailable".into()),
        }
    }
    Err("Language detection source busy".into())
}
