// SPDX-License-Identifier: MPL-2.0
//! Native file commands. Save All retains document identity while dialogs change focus.
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState};
use std::collections::VecDeque;
pub(super) enum Identity {
    Resident(bareline_document::DocumentSnapshot),
    Paged(bareline_document::paged::PagedSnapshot),
}
impl Identity {
    pub(super) fn capture(editor: &WorkspaceEditor) -> Self {
        match editor {
            WorkspaceEditor::Resident(e) => Self::Resident(e.snapshot().clone()),
            WorkspaceEditor::Paged(e) => Self::Paged(e.snapshot().clone()),
        }
    }
    pub(super) fn matches(&self, editor: &WorkspaceEditor) -> bool {
        match (self, editor) {
            (Self::Resident(a), WorkspaceEditor::Resident(b)) => a.same_document(b.snapshot()),
            (Self::Paged(a), WorkspaceEditor::Paged(b)) => a.same_document(b.snapshot()),
            _ => false,
        }
    }
}
#[derive(Default)]
pub(super) struct LifecycleRuntime {
    queue: VecDeque<Identity>,
    pending: Option<Identity>,
    saved: usize,
    skipped: usize,
    failed: usize,
    running: bool,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title, shortcut) in [
        ("file.save_copy", "Save Copy…", ""),
        ("file.save_all", "Save All", ""),
        (
            "file.restore_closed",
            "Restore Last Closed Tab",
            "Ctrl+Shift+T",
        ),
        ("file.read_only", "Set Read-Only", ""),
        ("file.cancel_save_all", "Cancel Save All", ""),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "File",
            shortcut,
            action: Action::Contributed(id),
        });
    }
}
impl LifecycleRuntime {
    pub(super) fn annotate_context(
        &self,
        context: &mut CommandContext,
        workspace: Option<&Workspace>,
        active: usize,
    ) {
        context.states.insert(
            CommandId("file.read_only"),
            CommandState {
                checked: workspace
                    .and_then(|w| w.editors.get(active))
                    .is_some_and(|e| e.read_only()),
                ..Default::default()
            },
        );
        for (id, disabled) in [
            (
                "file.restore_closed",
                !workspace.is_some_and(|w| w.can_restore_closed()),
            ),
            ("file.cancel_save_all", !self.running),
            (
                "file.save_all",
                self.running || !workspace.is_some_and(|w| !w.save_all_targets().is_empty()),
            ),
        ] {
            context.states.insert(
                CommandId(id),
                if disabled {
                    CommandState::disabled("No applicable file operation")
                } else {
                    CommandState::default()
                },
            );
        }
    }
}
impl Shell {
    pub(super) fn lifecycle_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "file.save_copy" => match self.platform.as_ref().map(|p| p.save_file()) {
                Some(Ok(Some(path))) => {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.save_copy(self.app.active, path);
                    }
                }
                Some(Err(error)) => {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some(error);
                    }
                }
                _ => {}
            },
            "file.save_all" => {
                if self.lifecycle.running {
                    return true;
                }
                if let Some(workspace) = &self.workspace {
                    self.lifecycle.queue = workspace
                        .save_all_targets()
                        .iter()
                        .map(|(index, _)| Identity::capture(&workspace.editors[*index]))
                        .collect();
                    self.lifecycle.pending = None;
                    self.lifecycle.saved = 0;
                    self.lifecycle.skipped = 0;
                    self.lifecycle.failed = 0;
                    self.lifecycle.running = true;
                }
                self.lifecycle_pump(el);
            }
            "file.cancel_save_all" => {
                self.lifecycle.queue.clear();
                self.lifecycle.pending = None;
                self.lifecycle.running = false;
                if let Some(workspace) = &mut self.workspace {
                    workspace.message =
                        Some("Save All cancelled; an already submitted save will finish.".into());
                }
            }
            "file.restore_closed" => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(index) = workspace.restore_last_closed()
                {
                    self.app.active = index;
                    self.app.tabs = workspace.titles();
                }
            }
            "file.read_only" => {
                if let Some(editor) = self
                    .workspace
                    .as_mut()
                    .and_then(|w| w.editors.get_mut(self.app.active))
                {
                    editor.set_read_only(!editor.read_only());
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn lifecycle_pump(&mut self, _el: &ActiveEventLoop) {
        if !self.lifecycle.running {
            return;
        }
        let Some(workspace) = &mut self.workspace else {
            self.lifecycle.running = false;
            return;
        };
        if let Some(identity) = &self.lifecycle.pending {
            if let Some(index) = workspace.editors.iter().position(|e| identity.matches(e)) {
                if workspace.document_busy(index) {
                    return;
                }
                if workspace.editors[index].dirty() {
                    self.lifecycle.failed += 1;
                } else {
                    self.lifecycle.saved += 1;
                }
            } else {
                self.lifecycle.skipped += 1;
            }
            self.lifecycle.pending = None;
        }
        while let Some(identity) = self.lifecycle.queue.pop_front() {
            let Some(index) = workspace.editors.iter().position(|e| identity.matches(e)) else {
                self.lifecycle.skipped += 1;
                continue;
            };
            if workspace.document_busy(index) {
                self.lifecycle.queue.push_front(identity);
                return;
            }
            let path = if let Some(path) = workspace.path(index) {
                Some(path.to_path_buf())
            } else {
                match self.platform.as_ref().map(|p| p.save_file()) {
                    Some(Ok(path)) => path,
                    Some(Err(error)) => {
                        self.lifecycle.failed += 1;
                        workspace.message = Some(error);
                        continue;
                    }
                    None => {
                        self.lifecycle.failed += 1;
                        continue;
                    }
                }
            };
            let Some(path) = path else {
                self.lifecycle.skipped += 1;
                continue;
            };
            workspace.save(index, path);
            self.lifecycle.pending = Some(identity);
            return;
        }
        self.lifecycle.running = false;
        workspace.message = Some(format!(
            "Save All: {} saved; {} cancelled or closed; {} failed or changed during save.",
            self.lifecycle.saved, self.lifecycle.skipped, self.lifecycle.failed
        ));
    }
}
