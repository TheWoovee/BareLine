// SPDX-License-Identifier: MPL-2.0
use super::Shell;
use bareline_platform::accessibility::{
    AccessibilityNode, AccessibilityRole, AccessibilitySnapshot,
};
impl Shell {
    pub(super) fn accessibility_snapshot(
        &self,
        width: f64,
        height: f64,
        scale: f64,
    ) -> AccessibilitySnapshot {
        let mut focus = 2;
        let editor_bounds = self.editor_bounds();
        let mut chrome = bareline_app::accessibility::tabs(&self.app, width as f32);
        chrome.extend(
            self.palette
                .semantics()
                .iter()
                .map(|n| bareline_app::accessibility::semantic_node(n, 1)),
        );
        let editor = self
            .workspace
            .as_ref()
            .and_then(|w| w.editors.get(self.app.active));
        for semantic in self.settings.controller.semantics() {
            if semantic.focused {
                focus = semantic.id.0;
            }
            chrome.push(bareline_app::accessibility::semantic_node(&semantic, 1));
        }
        if let Some(workspace) = &self.workspace {
            for semantic in workspace
                .find
                .semantics(editor_bounds.width)
                .into_iter()
                .chain(workspace.search_panel.semantics())
            {
                if semantic.focused && !self.settings.controller.open {
                    focus = semantic.id.0;
                }
                let mut node = bareline_app::accessibility::semantic_node(&semantic, 1);
                node.bounds[0] += editor_bounds.x as f64;
                node.bounds[1] += editor_bounds.y as f64;
                if node.role != AccessibilityRole::TextField
                    && let Some(command) = self
                        .app
                        .commands
                        .entries()
                        .find(|c| c.id.0 == semantic.command_id)
                {
                    node.name = command.title.into();
                }
                chrome.push(node);
            }
            if let Some(message) = &workspace.message {
                chrome.push(AccessibilityNode {
                    id: 9000,
                    parent: 1,
                    role: AccessibilityRole::Alert,
                    name: message.clone(),
                    value: None,
                    bounds: [0., 0., 0., 0.],
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: false,
                    invokable: false,
                });
            }
            if workspace.find.open {
                chrome.push(AccessibilityNode {
                    id: 9001,
                    parent: 1,
                    role: AccessibilityRole::Status,
                    name: "Find results".into(),
                    value: Some(workspace.find.status.clone()),
                    bounds: [0., 0., 0., 0.],
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: false,
                    invokable: false,
                });
            }
            if workspace.search_panel.open {
                chrome.push(AccessibilityNode {
                    id: 9002,
                    parent: 1,
                    role: AccessibilityRole::Status,
                    name: "Open document search".into(),
                    value: Some(workspace.search_panel.status().into()),
                    bounds: [0., 0., 0., 0.],
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: false,
                    invokable: false,
                });
            }
        }
        for semantic in self.toolbar.controller.semantics() {
            if semantic.focused && !self.settings.controller.open {
                focus = semantic.id.0;
            }
            chrome.push(bareline_app::accessibility::semantic_node(&semantic, 1));
        }
        if self.shortcuts.open {
            chrome.extend(self.shortcuts.accessibility_nodes(&self.app.commands));
            focus = if self.shortcuts.binding_focus {
                19001
            } else {
                19000
            };
        }
        let mut snapshot = bareline_app::accessibility::snapshot(
            "Bareline",
            width,
            height,
            editor.map(|v| &**v),
            chrome,
            if self.palette.open { 11000 } else { focus },
        );
        // AccessKit Windows adds native client-to-screen origin; bounds must be
        // physical client pixels, while renderer/control layout uses logical px.
        for node in &mut snapshot.nodes {
            if node.id == bareline_app::accessibility::EDITOR_ID {
                node.bounds[0] = editor_bounds.x as f64;
                node.bounds[2] = editor_bounds.width as f64;
                node.bounds[3] = (editor_bounds.height as f64 - node.bounds[1]).max(0.0);
                node.bounds[1] += editor_bounds.y as f64;
            }
            for coordinate in &mut node.bounds {
                *coordinate *= scale;
            }
        }
        snapshot
    }
}

impl Shell {
    pub(super) fn accessibility_actions(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        use bareline_app::accessibility::{EDITOR_ID, PAGE_NEXT_ID, PAGE_PREVIOUS_ID, TAB_ID_BASE};
        use bareline_app::workspace::Input;
        use bareline_platform::accessibility::AccessibilityAction;
        let actions = self
            .accessibility
            .as_mut()
            .map_or_else(Vec::new, |p| p.drain_actions());
        for action in actions {
            if self.shortcuts_accessibility(&action) {
                continue;
            }
            let toolbar_target = match &action {
                AccessibilityAction::Focus(id) | AccessibilityAction::Invoke(id) => self
                    .toolbar
                    .controller
                    .semantics()
                    .iter()
                    .any(|node| node.id.0 == *id),
                _ => false,
            };
            if toolbar_target && !self.palette.open {
                let (id, invoke) = match action {
                    AccessibilityAction::Focus(id) => (id, false),
                    AccessibilityAction::Invoke(id) => (id, true),
                    _ => unreachable!(),
                };
                let previous = self.toolbar.controller.model.commands.clone();
                let command = self.toolbar.controller.accessibility(id, invoke);
                if previous != self.toolbar.controller.model.commands {
                    self.toolbar_save();
                    self.toolbar_refresh();
                }
                if let Some(command) = command {
                    let context = self.command_context();
                    if let Ok(action) = self.app.commands.dispatch_in(command, &context) {
                        self.dispatch(el, action);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                continue;
            }
            if self.settings.controller.open && !self.palette.open {
                let effect = match &action {
                    AccessibilityAction::SetValue { id: 8000, value } => {
                        self.settings.controller.query.select_all();
                        self.settings.controller.query.insert(value);
                        self.settings.controller.query_changed();
                        None
                    }
                    AccessibilityAction::Focus(id) => {
                        self.settings.controller.accessibility_action(*id, false)
                    }
                    AccessibilityAction::Invoke(id) => {
                        self.settings.controller.accessibility_action(*id, true)
                    }
                    _ => None,
                };
                if let Some(bareline_app::settings::SettingsEffect::CopyKey(key)) = effect
                    && let Some(platform) = &self.platform
                    && let Err(error) = platform.set_clipboard_text(&key)
                {
                    self.settings.controller.error = Some(error.to_string());
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                continue;
            }
            match action {
                AccessibilityAction::SetSelection { anchor, caret } => {
                    if let Some(editor) = self
                        .workspace
                        .as_mut()
                        .and_then(|w| w.editors.get_mut(self.app.active))
                        && bareline_app::accessibility::selection_valid(editor, anchor, caret)
                    {
                        editor.enqueue(Input::SetCaret(anchor, false));
                        editor.enqueue(Input::SetCaret(caret, true));
                    }
                }
                AccessibilityAction::SetValue { id, value } => {
                    if id == 11000 && self.palette.open {
                        self.palette.field.select_all();
                        self.palette.field.insert(&value);
                        let context = self.command_context();
                        self.palette.refresh(
                            &self.app.commands,
                            &context,
                            &self.settings.keymap.keymap,
                        );
                    } else if let Some(workspace) = &mut self.workspace {
                        if (id == 6000 || id == 6001) && workspace.find.open {
                            workspace.find.accessibility_action(id, true);
                            let field = workspace.find.active_field();
                            field.select_all();
                            field.insert(&value);
                        } else if id == 7000 && workspace.search_panel.open {
                            workspace.search_focus = true;
                            workspace.search_panel.accessibility_focus(id);
                            workspace.search_panel.field.select_all();
                            workspace.search_panel.field.insert(&value);
                        }
                    }
                }
                AccessibilityAction::Focus(id) if id == EDITOR_ID => {
                    self.palette.dismiss();
                    self.app.palette = false;
                    if let Some(workspace) = &mut self.workspace {
                        workspace.find.blur();
                        workspace.search_focus = false;
                    }
                }
                AccessibilityAction::Focus(id) | AccessibilityAction::Invoke(id)
                    if id >= TAB_ID_BASE && id < TAB_ID_BASE + self.app.tabs.len() as u64 =>
                {
                    self.app.active = (id - TAB_ID_BASE) as usize;
                    if let Some(workspace) = &mut self.workspace {
                        workspace.find.blur();
                        workspace.search_focus = false;
                    }
                }
                AccessibilityAction::Invoke(id) if id == PAGE_PREVIOUS_ID || id == PAGE_NEXT_ID => {
                    let height = self.window.as_ref().map_or(600.0, |w| {
                        w.inner_size().height as f32 / w.scale_factor() as f32
                    });
                    if let Some(editor) = self
                        .workspace
                        .as_mut()
                        .and_then(|w| w.editors.get_mut(self.app.active))
                    {
                        editor.scroll(
                            if id == PAGE_PREVIOUS_ID {
                                -(height as f64)
                            } else {
                                height as f64
                            },
                            height,
                        );
                    }
                }
                AccessibilityAction::Focus(id) => {
                    if self.palette.accessibility_focus(id) {
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        continue;
                    }
                    if let Some(workspace) = &mut self.workspace {
                        if (6000..6300).contains(&id) {
                            workspace.find.accessibility_action(id, true);
                            workspace.search_focus = false;
                        } else if workspace
                            .search_panel
                            .semantics()
                            .iter()
                            .any(|n| n.id.0 == id)
                        {
                            workspace.search_focus = true;
                            workspace.search_panel.accessibility_focus(id);
                            workspace.find.blur();
                        }
                    }
                }
                AccessibilityAction::Invoke(id) => {
                    let mut command_action = None;
                    if let Some(workspace) = &mut self.workspace {
                        if (6000..6300).contains(&id) {
                            command_action = workspace
                                .find
                                .accessibility_action(id, false)
                                .map(super::find_action);
                        } else if workspace
                            .search_panel
                            .semantics()
                            .iter()
                            .any(|n| n.id.0 == id)
                            && let Some((source, range)) =
                                workspace.search_panel.accessibility_activate(id)
                            && let Some(index) = workspace.activate_search(source, range)
                        {
                            self.app.active = index;
                        }
                    }
                    if let Some(action) = command_action {
                        self.dispatch(el, action);
                    } else if (11000..TAB_ID_BASE).contains(&id) {
                        let context = self.command_context();
                        let command = self
                            .palette
                            .semantics()
                            .into_iter()
                            .find(|n| n.id.0 == id)
                            .and_then(|node| {
                                self.app
                                    .commands
                                    .entries()
                                    .find(|c| c.id.0 == node.command_id)
                                    .map(|c| c.id)
                            });
                        if let Some(action) =
                            command.and_then(|id| self.app.commands.dispatch_in(id, &context).ok())
                        {
                            self.palette.dismiss();
                            self.app.palette = false;
                            self.dispatch(el, action);
                        }
                    }
                }
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}
