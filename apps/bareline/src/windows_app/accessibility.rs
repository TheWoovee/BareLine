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
fn compose_snapshot(
    title: &str,
    width: f64,
    height: f64,
    editor: Option<&bareline_editor_surface::EditorSurface>,
    chrome: Vec<AccessibilityNode>,
    focus: u64,
    active_layer: Option<u64>,
) -> AccessibilitySnapshot {
    let mut snapshot = bareline_app::accessibility::snapshot(title, width, height, editor, chrome, focus);
    apply_modal_layer(&mut snapshot, active_layer);
    snapshot
}
// Shared by native publication and complete headless semantic fixtures.
fn apply_modal_layer(snapshot: &mut AccessibilitySnapshot, active_layer: Option<u64>) {
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
        self.accessibility_snapshot_with_editor_bounds(width, height, scale, self.editor_bounds())
    }

    fn accessibility_snapshot_with_editor_bounds(
        &self,
        width: f64,
        height: f64,
        scale: f64,
        editor_bounds: bareline_renderer::Rect,
    ) -> AccessibilitySnapshot {
        let mut focus = 2;
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
        semantic_group(&mut chrome, 90_000_025, "Compare", self.compare_accessibility_nodes());
        chrome.extend(self.panels_accessibility_nodes());
        chrome.extend(self.extensions_accessibility_nodes());
        chrome.extend(self.language_accessibility_nodes());
        semantic_group(&mut chrome, 90_000_026, "Utilities and print options", self.utilities_accessibility_nodes());
        if !self.settings.controller.open {
            if let Some(id) = self.views_accessibility_focus() { focus = id; }
            if let Some(id) = self.recovery_accessibility_focus() { focus = id; }
            if let Some(id) = self.compare_accessibility_focus() { focus = id; }
            if let Some(id) = self.panels_accessibility_focus() { focus = id; }
            if let Some(id) = self.extensions_accessibility_focus() { focus = id; }
            if let Some(id) = self.language_accessibility_focus() { focus = id; }
            if let Some(id) = self.utilities_accessibility_focus() { focus = id; }
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
        let active_layer = if self.palette.open { Some(90_000_002) }
            else if self.shortcuts.open { Some(90_000_004) }
            else if self.power.open { Some(90_000_006) }
            else if self.settings.controller.open { Some(90_000_012) }
            else if self.extensions.open { Some(60_000) }
            else if self.utilities.has_input_focus() { Some(90_000_026) }
            else if self.macros.controller.manager.open { Some(90_000_014) }
            else { None };
        let mut snapshot = compose_snapshot(
            "Bareline",
            width,
            height,
            editor.map(|v| &**v),
            chrome,
            if self.palette.open { self.palette.semantics().iter().find(|n|n.focused).map_or(11000, |n|n.id.0) } else { focus },
            active_layer,
        );
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
                if self.utilities_accessibility(el, &action) { continue; }
                if self.utilities.has_input_focus() { continue; }
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
                        if !editor.page_by(id == PAGE_NEXT_ID) { editor.scroll(
                            if id == PAGE_PREVIOUS_ID {
                                -(height as f64)
                            } else {
                                height as f64
                            },
                            height,
                        ); }
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{launch, views, compare, utilities, shortcuts};
    type ShellSetup = fn(&mut Shell, &str);
    type SetupCases = (&'static str, ShellSetup, &'static [&'static str], u64);

    fn headless_shell() -> Shell {
        let launch = launch::LaunchConfig {
            performance: None, portable: false, settings_path: None, session_path: None,
            recovery_path: None, extensions_path: None, diagnostics_path: None, paths: vec![],
            line: None, column: None, read_only: false, monitor: false, no_session: true,
            no_extensions: true, new_instance: true, help: false, version: false,
            software: true, hardware: false, smoke: false, prototype: false, perf: false,
        };
        Shell {
            renderer: None, platform: None, accessibility: None, shell_integration: Default::default(),
            window: None, app: Default::default(), palette: Default::default(), ui_focus: Default::default(),
            ui_router: Default::default(), ledger: Default::default(), modifiers: Default::default(),
            software: true, first_frame: false, smoke: false, failed: false, prototype: None,
            workspace: None, notify: std::sync::Arc::new(|| {}), pointer: Default::default(),
            editor_caret: None, perf: false, idle_at: None, frames: 0, log: None, log_directory: None,
            startup_paths: vec![], session: Default::default(), settings: Default::default(),
            views: Default::default(), macros: Default::default(), watch: Default::default(),
            panels: Default::default(), launch: launch::LaunchRuntime::new(&launch), applied_settings: None,
            update: Default::default(), language: Default::default(), extensions: Default::default(),
            toolbar: Default::default(), compare: Default::default(), instance: Default::default(),
            recovery_root: None, shortcuts: Default::default(), recovery: Default::default(),
            lifecycle: Default::default(), performance: Default::default(), power: Default::default(),
            utilities: Default::default(), migration: Default::default(), search: Default::default(),
            encoding: Default::default(), inventory: Default::default(),
        }
    }

    fn dump(errors: &mut Vec<String>, name: &str, snapshot: &AccessibilitySnapshot) -> serde_json::Value {
        if let Err(error)=snapshot.validate() {
            let mut counts=std::collections::BTreeMap::new();
            for node in &snapshot.nodes {*counts.entry(node.id).or_insert(0usize)+=1;}
            let duplicates:Vec<_>=counts.iter().filter(|(_,count)|**count>1).collect();
            errors.push(format!("{name}: {error}; root={} focus={} focus_present={} duplicates={duplicates:?}; nodes={:?}",snapshot.root,snapshot.focus,counts.contains_key(&snapshot.focus),snapshot.nodes.iter().map(|n|(n.id,n.parent,n.name.as_str())).collect::<Vec<_>>()));
        }
        let mut value = serde_json::to_value(snapshot).unwrap();
        // Document identity is process-allocated, not UI. Preserve revision and all
        // semantic/text fields; source-token correctness has separate COM tests.
        if let Some(context) = value.get_mut("text_context").and_then(|v| v.as_object_mut()) {
            context.get_mut("source_identity").unwrap()[0] = serde_json::json!(0);
        }
        value
    }

    fn shell_snapshot(shell: &Shell) -> AccessibilitySnapshot {
        let bounds = bareline_ui::rect(shell.panels.width_left(), shell.toolbar.controller.height(),
            (1000.0-shell.panels.width_left()-shell.panels.width_right()).max(0.0),
            (800.0-shell.macros.height()-shell.toolbar.controller.height()).max(0.0));
        shell.accessibility_snapshot_with_editor_bounds(1000.0, 800.0, 1.0, bounds)
    }

    fn app_shell(scenario: &str) -> Shell {
        let mut shell = headless_shell();
        views::accessibility_test_setup(&mut shell, "open");
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        let context = bareline_commands::CommandContext::default();
        let keymap = bareline_commands::Keymap::defaults(&shell.app.commands);
        // Native layout reserves the toolbar before laying out editor-local
        // Find/search controls. Keep the headless dimensions in that order.
        if scenario.starts_with("toolbar") || scenario == "all_app_panels" {
            shell.toolbar.controller.model.visible = true;
        }
        let editor_height=800.0-shell.toolbar.controller.height();
        if scenario.starts_with("find") || scenario == "all_app_panels" {
            let workspace = shell.workspace.as_mut().unwrap();
            workspace.find.show_replace();
            workspace.find.field.insert("needle");
            workspace.find.replacement.insert("replacement");
            if scenario == "find.replace_focus" { workspace.find.accessibility_action(6001, false); }
            if scenario == "find.match_case_focus" { let id=workspace.find.semantics(1000.0).iter().find(|n|n.command_id=="search.match_case").unwrap().id.0; workspace.find.accessibility_action(id, true); }
            workspace.find.draw(&mut backend, 1000.0, &mut operations).unwrap();
        }
        if scenario.starts_with("search") || scenario == "all_app_panels" {
            let workspace = shell.workspace.as_mut().unwrap();
            workspace.search_panel.open = true;
            workspace.search_panel.field.insert("workspace");
            workspace.search_panel.draw(&mut backend,1000.0,editor_height,&[],&mut operations).unwrap();
        }
        if scenario.starts_with("settings") || scenario == "all_app_panels" {
            shell.settings.controller.show();
            if scenario == "settings.query_value" { assert!(shell.settings.controller.accessibility_set_value(8000,"font")); }
            shell.settings.controller.draw(bareline_ui::rect(0.0,0.0,1000.0,800.0),&mut backend,&mut operations).unwrap();
        }
        if scenario.starts_with("palette") || scenario == "all_app_panels" {
            shell.palette.show(&shell.app.commands,&context,&keymap);
            shell.palette.draw(&mut backend,1000.0,800.0,&mut operations).unwrap();
            if scenario == "palette.result_selection" { assert!(shell.palette.key(bareline_ui::controls::Key::Down,&shell.app.commands,&context).is_none()); }
        }
        if scenario.starts_with("toolbar") || scenario == "all_app_panels" {
            shell.toolbar.controller.model.visible = true;
            shell.toolbar.controller.refresh(&shell.app.commands,&context,&keymap,1000.0);
            if scenario == "toolbar.focus" { shell.toolbar.controller.focus(); }
            if scenario == "toolbar.customize" { shell.toolbar.controller.customize(&shell.app.commands); }
            shell.toolbar.controller.draw(1000.0,800.0,Default::default(),&mut operations);
        }
        shell
    }

    #[test]
    fn complete_native_semantic_json_golden() {
        let mut errors = Vec::new();
        let mut cases = std::collections::BTreeMap::new();
        cases.insert("default".to_owned(), dump(&mut errors,"default",&shell_snapshot(&headless_shell())));
        for scenario in ["default_document","find.open","find.replace_focus","find.match_case_focus","search.open","settings.open","settings.query_value","palette.open","palette.result_selection","toolbar.open","toolbar.focus","toolbar.customize","all_app_panels"] {
            let snapshot = shell_snapshot(&app_shell(scenario));
            if !snapshot.nodes.iter().any(|n|n.id==2) {errors.push(format!("{scenario}: document fixture must expose editor"));}
            let target=if scenario.starts_with("find") {Some(6000)} else if scenario.starts_with("search") {Some(7000)} else if scenario.starts_with("settings") {Some(90_000_012)} else if scenario.starts_with("palette") {Some(90_000_002)} else if scenario.starts_with("toolbar") {Some(90_000_003)} else {None};
            if let Some(target)=target {if !snapshot.nodes.iter().any(|n|n.id==target) {errors.push(format!("{scenario}: missing actual surface {target}"));}}
            if scenario=="all_app_panels" && snapshot.nodes.iter().any(|node|(7000..=7003).contains(&node.id)&&node.bounds[1]+node.bounds[3]>800.0) {
                errors.push("all_app_panels: search layout must reserve toolbar height".into());
            }
            if scenario=="palette.result_selection" && (snapshot.focus!=11000 || !snapshot.nodes.iter().any(|node|node.id==11000&&node.focusable&&!node.disabled) || !snapshot.nodes.iter().any(|node|node.id==11002&&node.selected&&node.invokable&&!node.focusable&&!node.disabled)) {
                errors.push("palette.result_selection: Down must select second invokable row while query retains focus".into());
            }
            cases.insert(scenario.to_owned(),dump(&mut errors,scenario,&snapshot));
        }
        let setups: &[SetupCases] = &[
            ("views", views::accessibility_test_setup, &["closed","open","populated","focus_close","focus_overflow","mru","vertical"],90_000_001),
            ("compare", compare::accessibility_test_setup, &["closed","open","populated","options","colors","focus","value"],90_000_025),
            ("utilities", utilities::accessibility_test_setup, &["closed","open","populated","options","focus","value"],90_000_026),
            ("shortcuts", shortcuts::accessibility_test_setup, &["closed","open","filtered","focus_binding","error"],90_000_004),
        ];
        let mut all_chrome = Vec::new();
        for (prefix, setup, scenarios, group) in setups {
            for scenario in *scenarios {
                let mut shell = headless_shell();
                setup(&mut shell, scenario);
                let snapshot = shell_snapshot(&shell);
                if *scenario != "closed" {
                    if !snapshot.nodes.iter().any(|n|n.id==*group) {errors.push(format!("{prefix}.{scenario}: missing actual container {group}"));}
                    if !snapshot.nodes.iter().any(|n|n.id!=*group && n.parent==*group) {errors.push(format!("{prefix}.{scenario}: missing actual controls"));}
                }
                if *scenario == "populated" {
                    // Retain complete owner subtrees, excluding other shell surfaces.
                    let parents: std::collections::BTreeMap<_,_> = snapshot.nodes.iter().map(|n|(n.id,n.parent)).collect();
                    all_chrome.extend(snapshot.nodes.iter().filter(|n| {
                        let mut id=n.id;
                        for _ in 0..parents.len() { if id==*group{return true;} let Some(parent)=parents.get(&id) else {break}; if *parent==1{break;} id=*parent; }
                        false
                    }).cloned());
                }
                cases.insert(format!("{prefix}.{scenario}"),dump(&mut errors,&format!("{prefix}.{scenario}"),&snapshot));
            }
        }
        let fixture_groups = [
            ("power",super::super::power::accessibility_test_cases(), Some((90_000_006,"Column editor and clipboard history"))),
            ("recovery",super::super::recovery::accessibility_test_cases(), None),
            ("panels",super::super::workspace_panels::accessibility_test_cases(), None),
            ("language",super::super::language::accessibility_test_cases(), None),
            ("extensions",super::super::extensions::accessibility_test_cases(), None),
        ];
        for (prefix,fixtures, group) in fixture_groups {
            let mut representative: Option<Vec<AccessibilityNode>> = None;
            for (name, nodes, focus) in fixtures {
                if !name.ends_with("closed") && nodes.is_empty() {errors.push(format!("{prefix}/{name}: missing actual controls"));}
                let mut chrome=Vec::new();
                if let Some((id,label))=group { semantic_group(&mut chrome,id,label,nodes); } else {chrome=nodes;}
                if !chrome.is_empty() {
                    let root=match prefix {"power"=>90_000_006,"recovery"=>100900,"panels"=>90_000_009,"language"=>70000,"extensions"=>60000,_=>unreachable!()};
                    if !chrome.iter().any(|n|n.id==root) {errors.push(format!("{prefix}/{name}: missing owner hierarchy {root}"));}
                }
                if representative.as_ref().is_none_or(|previous|chrome.len()>previous.len()) || name.ends_with("all_open") {representative=Some(chrome.clone());}
                let layer=group.map(|(id,_)|id).filter(|_|!chrome.is_empty());
                let snapshot=compose_snapshot("Bareline",1000.0,800.0,None,chrome,focus.unwrap_or(1),layer);
                if cases.insert(format!("{prefix}/{name}"),dump(&mut errors,&format!("{prefix}/{name}"),&snapshot)).is_some() {errors.push(format!("duplicate fixture case {prefix}/{name}"));}
            }
            if let Some(nodes)=representative {all_chrome.extend(nodes);}
        }
        let mut macro_manager = Vec::new();
        let mut macro_output = Vec::new();
        for (name,nodes,focus) in super::super::macros::accessibility_test_cases() {
            let (output,manager): (Vec<_>,Vec<_>) = nodes.into_iter().partition(|n|n.id>=2_000_000);
            let mut chrome=Vec::new();
            semantic_group(&mut chrome,90_000_014,"Macro and Run manager",manager.clone());
            semantic_group(&mut chrome,90_000_015,"Command output",output.clone());
            // The shared status (23300) remains present with output alone;
            // production gates the modal layer on manager.open, not that alert.
            let layer=manager.iter().any(|node|node.id!=23300).then_some(90_000_014);
            let snapshot=compose_snapshot("Bareline",1000.0,800.0,None,chrome,focus.unwrap_or(1),layer);
            if name=="macros_output_link_focus" && (snapshot.focus!=2_000_000 || !snapshot.nodes.iter().any(|node|node.id==2_000_000&&node.focusable&&node.invokable&&!node.disabled)) {
                errors.push("macros_output_link_focus: actual link must retain focus and actions with manager closed".into());
            }
            cases.insert(name.into(),dump(&mut errors,name,&snapshot));
            if manager.len()>macro_manager.len() {macro_manager=manager;}
            if output.len()>macro_output.len() {macro_output=output;}
        }
        if macro_manager.is_empty() {errors.push("actual macro manager fixture required".into());}
        if !macro_output.iter().any(|n|n.focusable) {errors.push("actual populated output row fixture required".into());}
        semantic_group(&mut all_chrome,90_000_014,"Macro and Run manager",macro_manager);
        semantic_group(&mut all_chrome,90_000_015,"Command output",macro_output);
        let app = app_shell("all_app_panels");
        let app_snapshot = shell_snapshot(&app);
        // Full retained app controls join native owner subtrees. Their modal
        // flags are reset by taking each surface's own nonmodal snapshot below.
        for scenario in ["find.open","search.open","settings.open","palette.open","toolbar.open"] {
            let shell=app_shell(scenario);
            let surface=shell_snapshot(&shell);
            let allowed=match scenario {
                "find.open"=>shell.workspace.as_ref().unwrap().find.semantics(1000.0).iter().map(|n|n.id.0).chain([9001]).collect(),
                "search.open"=>shell.workspace.as_ref().unwrap().search_panel.semantics().iter().map(|n|n.id.0).chain([9002]).collect(),
                "settings.open"=>vec![90_000_012],"palette.open"=>vec![90_000_002],_=>vec![90_000_003]
            };
            let parents:std::collections::BTreeMap<_,_>=surface.nodes.iter().map(|n|(n.id,n.parent)).collect();
            all_chrome.extend(surface.nodes.into_iter().filter(|n| {
                let mut id=n.id;
                for _ in 0..parents.len() {if allowed.contains(&id){return true;} let Some(parent)=parents.get(&id) else{break}; if *parent==1{break;} id=*parent;}
                false
            }));
        }
        all_chrome.extend(app_snapshot.nodes.into_iter().filter(|n|n.id==90_000_020||n.parent==90_000_020));
        let editor=app.workspace.as_ref().unwrap().editors.first().map(|e|&**e);
        let mut shortcuts_shell=headless_shell();
        shortcuts::accessibility_test_setup(&mut shortcuts_shell,"open");
        all_chrome.extend(shell_snapshot(&shortcuts_shell).nodes.into_iter().filter(|n|n.id==90_000_004||n.parent==90_000_004));
        for layer in [90_000_002,90_000_004,90_000_006,90_000_012,60_000,90_000_026,90_000_014] {
            if !all_chrome.iter().any(|n|n.id==layer) {errors.push(format!("all_panels.modal_{layer}: missing actual layer"));}
            let snapshot=compose_snapshot("Bareline",1000.0,800.0,editor,all_chrome.clone(),2,Some(layer));
            if !snapshot.nodes.iter().any(|n|n.id==snapshot.focus&&n.focusable&&!n.disabled) {errors.push(format!("all_panels.modal_{layer}: focus {} must be enabled and focusable",snapshot.focus));}
            cases.insert(format!("all_panels.modal_{layer}"),dump(&mut errors,&format!("all_panels.modal_{layer}"),&snapshot));
        }
        assert!(errors.is_empty(), "semantic fixture validation failures:\n{}", errors.join("\n"));
        let actual=serde_json::to_string_pretty(&cases).unwrap()+"\n";
        let path=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/a11y/native-semantic.json");
        if let Some(candidate)=std::env::var_os("BARELINE_CAPTURE_ACCESSIBILITY_GOLDEN") {
            std::fs::write(candidate, &actual).expect("write explicitly requested candidate");
            panic!("candidate captured; review full JSON and install baseline, then rerun without capture");
        }
        let expected=std::fs::read_to_string(&path).expect("reviewed full semantic baseline must exist");
        let actual: serde_json::Value = serde_json::from_str(&actual).expect("serialized actual semantics");
        let expected: serde_json::Value = serde_json::from_str(&expected).expect("valid reviewed semantic baseline");
        assert_eq!(actual,expected,"full semantic fields, hierarchy, focus and action capabilities changed");
    }
}
