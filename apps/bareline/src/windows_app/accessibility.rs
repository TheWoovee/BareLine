// SPDX-License-Identifier: MPL-2.0
use super::Shell;
use bareline_platform::accessibility::{
    AccessibilityNode, AccessibilityRole, AccessibilitySnapshot,
};
fn semantic_group(nodes: &mut Vec<AccessibilityNode>, id: u64, name: &str, mut children: Vec<AccessibilityNode>) {
    if children.is_empty() { return; }
    let x = children.iter().map(|n| n.bounds[0]).fold(f64::INFINITY, f64::min);
    let y = children.iter().map(|n| n.bounds[1]).fold(f64::INFINITY, f64::min);
    let right = children.iter().map(|n| n.bounds[0]+n.bounds[2]).fold(x, f64::max);
    let bottom = children.iter().map(|n| n.bounds[1]+n.bounds[3]).fold(y, f64::max);
    for node in &mut children { if node.parent == 1 { node.parent = id; } }
    nodes.push(AccessibilityNode { id, parent: 1, role: AccessibilityRole::Group, name: name.into(), value: None,
        bounds: [x,y,right-x,bottom-y], disabled: false, selected: false, expanded: None, focusable: false, invokable: false });
    nodes.extend(children);
}
impl Shell {
    pub(super) fn accessibility_text_source(&self) -> Option<std::sync::Arc<dyn bareline_platform::accessibility::AccessibilityTextSource>> {
        self.workspace.as_ref().and_then(|w| w.editors.get(self.app.active)).map(|editor| bareline_app::accessibility::text_source(editor, self.notify.clone()))
    }
    pub(super) fn accessibility_snapshot(
        &self,
        width: f64,
        height: f64,
        scale: f64,
    ) -> AccessibilitySnapshot {
        let mut focus = 2;
        let editor_bounds = self.editor_bounds();
        let mut chrome = Vec::new();
        semantic_group(&mut chrome, 90_000_001, "Document tabs", self.views_accessibility_nodes());
        semantic_group(&mut chrome, 90_000_002, "Command palette",
            self.palette
                .semantics()
                .iter()
                .map(|n| bareline_app::accessibility::semantic_node(n, 1)).collect(),
        );
        let editor = self
            .workspace
            .as_ref()
            .and_then(|w| w.editors.get(self.app.active));
        if let Some(editor) = editor {
            let mut status = bareline_app::accessibility::status(editor, editor_bounds.width as f64, editor_bounds.height as f64);
            for node in &mut status { node.bounds[0] += editor_bounds.x as f64; node.bounds[1] += editor_bounds.y as f64; }
            semantic_group(&mut chrome, 90_000_020, "Status bar", status);
        }
        let mut settings_nodes = Vec::new();
        for semantic in self.settings.controller.semantics() {
            if semantic.focused {
                focus = semantic.id.0;
            }
            settings_nodes.push(bareline_app::accessibility::semantic_node(&semantic, 1));
        }
        semantic_group(&mut chrome, 90_000_012, "Settings", settings_nodes);
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
        let mut toolbar_nodes = Vec::new();
        for semantic in self.toolbar.controller.semantics() {
            if semantic.focused && !self.settings.controller.open {
                focus = semantic.id.0;
            }
            toolbar_nodes.push(bareline_app::accessibility::semantic_node(&semantic, 1));
        }
        semantic_group(&mut chrome, 90_000_003, "Toolbar", toolbar_nodes);
        chrome.extend(self.recovery_accessibility_nodes());
        chrome.extend(self.compare_accessibility_nodes());
        chrome.extend(self.panels_accessibility_nodes());
        chrome.extend(self.extensions_accessibility_nodes());
        chrome.extend(self.language_accessibility_nodes());
        if !self.settings.controller.open {
            if let Some(id) = self.views_accessibility_focus() { focus = id; }
            if let Some(id) = self.recovery_accessibility_focus() { focus = id; }
            if let Some(id) = self.compare_accessibility_focus() { focus = id; }
            if let Some(id) = self.panels_accessibility_focus() { focus = id; }
            if let Some(id) = self.extensions_accessibility_focus() { focus = id; }
            if let Some(id) = self.language_accessibility_focus() { focus = id; }
        }
        let mut manager = Vec::new();
        let mut output = Vec::new();
        for semantic in self.macros.controller.semantics() {
            if semantic.focused && !self.settings.controller.open { focus = semantic.id.0; }
            let node = bareline_app::accessibility::semantic_node(&semantic, 1);
            if semantic.id.0 >= 2_000_000 { output.push(node); } else { manager.push(node); }
        }
        semantic_group(&mut chrome, 90_000_014, "Macro and Run manager", manager);
        semantic_group(&mut chrome, 90_000_015, "Command output", output);
        let power_nodes = self.power.accessibility_nodes();
        if let Some(node) = power_nodes.iter().find(|n| n.selected && n.focusable) { focus = node.id; }
        semantic_group(&mut chrome, 90_000_006, "Column editor and clipboard history", power_nodes);
        if self.shortcuts.open {
            semantic_group(&mut chrome, 90_000_004, "Keyboard shortcuts", self.shortcuts.accessibility_nodes(&self.app.commands));
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
            if self.palette.open { self.palette.semantics().iter().find(|n|n.focused).map_or(11000, |n|n.id.0) } else { focus },
        );
        let active_layer = if self.palette.open { Some(90_000_002) }
            else if self.shortcuts.open { Some(90_000_004) }
            else if self.power.open { Some(90_000_006) }
            else if self.settings.controller.open { Some(90_000_012) }
            else if self.extensions.open { Some(60_000) }
            else if self.macros.controller.manager.open { Some(90_000_014) }
            else { None };
        if let Some(layer) = active_layer {
            let parents: std::collections::BTreeMap<_,_> = snapshot.nodes.iter().map(|n|(n.id,n.parent)).collect();
            let belongs = |mut id| {
                for _ in 0..parents.len() {
                    if id == layer { return true; }
                    let Some(parent) = parents.get(&id) else { break; };
                    id = *parent;
                    if id == 1 { break; }
                }
                false
            };
            for node in &mut snapshot.nodes {
                if node.id != 1 && !belongs(node.id) {
                    node.disabled = true; node.focusable = false; node.invokable = false;
                }
            }
            if !belongs(snapshot.focus) {
                snapshot.focus = snapshot.nodes.iter().find(|n| belongs(n.id) && n.focusable && !n.disabled).map_or(layer, |n|n.id);
            }
        }
        if let (Some(editor), Some(renderer)) = (editor, self.renderer.as_ref()) {
            snapshot.text_geometry = editor.accessibility_geometry(renderer, editor_bounds.width, editor_bounds.height).into_iter().map(|(range, rect)| bareline_platform::accessibility::AccessibilityTextBox {
                start: range.start, end: range.end,
                bounds: [(rect.x as f64+editor_bounds.x as f64)*scale, (rect.y as f64+editor_bounds.y as f64)*scale, rect.width as f64*scale, rect.height as f64*scale],
            }).collect();
        }
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = editor {
            let base = editor.viewport_start().0;
            if let Some(text) = &mut snapshot.text { text.start_byte += base; }
            for rect in &mut snapshot.text_geometry { rect.start += base; rect.end += base; }
            if let Some(context) = &mut snapshot.text_context {
                context.source_identity = editor.snapshot().identity_token();
                context.selection.0 += base;
                context.selection.1 += base;
            }
        }
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
            if self.palette.open {
                let id = match &action {
                    AccessibilityAction::Focus(id) | AccessibilityAction::Invoke(id) | AccessibilityAction::SetValue { id, .. } => Some(*id),
                    _ => None,
                };
                if !id.is_some_and(|id| self.palette.semantics().iter().any(|n|n.id.0==id && !n.disabled)) { continue; }
            }
            if self.power_accessibility(&action) { continue; }
            if self.power.open && !self.palette.open { continue; }
            if !self.palette.open && !self.power.open && !self.settings.controller.open {
                if self.extensions_accessibility(el, &action) { continue; }
                if self.extensions.open { continue; }
                if self.language_accessibility(el, &action) { continue; }
                if !self.macros.controller.manager.open && (self.views_accessibility(el, &action) || self.recovery_accessibility(el, &action) || self.compare_accessibility(el, &action) || self.panels_accessibility(el, &action)) { continue; }
            }
            if !self.palette.open && !self.power.open && !self.settings.controller.open {
                let target = match &action {
                    AccessibilityAction::Focus(id) => Some((*id, false, None)),
                    AccessibilityAction::Invoke(id) => Some((*id, true, None)),
                    AccessibilityAction::SetValue { id, value } => Some((*id, false, Some(value.clone()))),
                    _ => None,
                };
                if let Some((id, invoke, value)) = target {
                    if self.macros_accessibility(el, id, invoke, value) { continue; }
                }
                if self.macros.controller.manager.open { continue; }
            }
            if self.shortcuts_accessibility(&action) {
                continue;
            }
            if self.shortcuts.open && !self.palette.open { continue; }
            let toolbar_target = match &action {
                AccessibilityAction::Focus(id) | AccessibilityAction::Invoke(id) => self
                    .toolbar
                    .controller
                    .semantics()
                    .iter()
                    .any(|node| node.id.0 == *id),
                _ => false,
            };
            if toolbar_target && !self.palette.open && !self.settings.controller.open && !self.macros.controller.manager.open {
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
                    AccessibilityAction::SetValue { id, value } => {
                        self.settings.controller.accessibility_set_value(*id, value);
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
                AccessibilityAction::ScrollToText { source_identity, offset } => {
                    let height = self.editor_bounds().height;
                    if let Some(editor) = self.workspace.as_mut().and_then(|w| w.editors.get_mut(self.app.active)) {
                        if bareline_app::accessibility::source_identity(editor) != source_identity || editor.busy() { continue; }
                        match editor {
                            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                                if let Err(error) = editor.request_viewport(bareline_document::TextOffset(offset)) { editor.error = Some(error); }
                            }
                            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                                editor.accessibility_scroll_to(offset, height);
                            }
                        }
                    }
                }
                AccessibilityAction::SetSelection { source_identity, anchor, caret } => {
                    if let Some(editor) = self
                        .workspace
                        .as_mut()
                        .and_then(|w| w.editors.get_mut(self.app.active))
                    {
                        if bareline_app::accessibility::source_identity(editor) != source_identity || editor.busy() { continue; }
                        match editor {
                            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                                if let Err(error) = editor.restore_selection(bareline_document::TextOffset(anchor), bareline_document::TextOffset(caret)) { editor.error = Some(error); }
                            }
                            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                                if bareline_app::accessibility::selection_valid(editor, anchor, caret) {
                                    editor.enqueue(Input::SetCaret(anchor, false));
                                    editor.enqueue(Input::SetCaret(caret, true));
                                }
                            }
                        }
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
