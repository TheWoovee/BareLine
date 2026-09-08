// SPDX-License-Identifier: MPL-2.0
//! Native search scope commands and fingerprint-bound file-result navigation.
use super::*;
mod replace;
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_document::TextOffset;
use bareline_file_io::lifecycle::Fingerprint;
use bareline_search::folders::FolderScope;
use std::{ops::Range, sync::Arc};
#[derive(Default)]
pub(super) struct SearchRuntime {
    replace: replace::ReplaceRuntime,
    navigation: Option<(PathBuf, Fingerprint, Range<TextOffset>)>,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    replace::register(registry);
    let commands = [
        ("search.folder", "Find in Folder…"),
        ("search.scope.selection", "Find in Selection"),
        ("search.scope.current", "Find in Current Document"),
        ("search.mark.style1", "Mark All — Style 1"),
        ("search.mark.style2", "Mark All — Style 2"),
        ("search.mark.style3", "Mark All — Style 3"),
        ("search.mark.style4", "Mark All — Style 4"),
        ("search.mark.style5", "Mark All — Style 5"),
        ("search.mark.clearStyle1", "Clear Mark Style 1"),
        ("search.mark.clearStyle2", "Clear Mark Style 2"),
        ("search.mark.clearStyle3", "Clear Mark Style 3"),
        ("search.mark.clearStyle4", "Clear Mark Style 4"),
        ("search.mark.clearStyle5", "Clear Mark Style 5"),
        ("search.mark.clearAll", "Clear All Marks"),
    ];
    for (id, title) in commands {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "Search",
            shortcut: "",
            action: Action::Contributed(id),
        });
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: format!("Search > {title}"),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}
impl SearchRuntime {
    pub(super) fn draw(
        &mut self,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        self.replace.draw(width, height, ops);
    }
}
impl Shell {
    pub(super) fn search_modal(&self) -> bool {
        self.search.replace.is_open()
    }
    pub(super) fn search_draw(
        &mut self,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        self.search.replace.draw(width, height, ops);
    }
    pub(super) fn search_pointer(&mut self, point: Point) -> bool {
        self.search_replace_pointer(point)
    }
    pub(super) fn search_key(&mut self, key: bareline_ui::controls::Key) -> bool {
        self.search_replace_key(key)
    }

    pub(super) fn search_command(&mut self, id: &str) -> bool {
        if self.search_replace_command(id) {
            return true;
        }
        if id == "search.folder" {
            let selected = self
                .platform
                .as_ref()
                .map(|platform| platform.pick_folder());
            match selected {
                Some(Ok(Some(root))) => {
                    if let Some(workspace) = &mut self.workspace {
                        let query = workspace.find.query();
                        workspace.find.blur();
                        workspace.search_focus = true;
                        workspace.search_panel.start_folder(
                            FolderScope::user(root),
                            query,
                            Arc::new(bareline_platform_windows::WindowsPathTrustProvider),
                            Arc::new(bareline_platform_windows::WindowsFileSystem),
                            self.notify.clone(),
                        );
                    }
                }
                Some(Err(error)) => {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message = Some(error);
                    }
                }
                _ => {}
            }
            return true;
        }
        if id == "search.scope.selection" || id == "search.scope.current" {
            if let Some(workspace) = &mut self.workspace {
                let scope = if id.ends_with("selection") {
                    workspace.editors.get(self.app.active).map(|editor| {
                        let origin = match editor {
                            WorkspaceEditor::Paged(paged) => paged.viewport_start().0,
                            _ => 0,
                        };
                        TextOffset(origin + editor.selection.anchor.min(editor.selection.caret))
                            ..TextOffset(
                                origin + editor.selection.anchor.max(editor.selection.caret),
                            )
                    })
                } else {
                    None
                };
                workspace.find.set_selection_scope(scope);
                workspace.find.show();
            }
            return true;
        }
        if id.starts_with("search.mark.") {
            if let Some(workspace) = &mut self.workspace {
                let clear = id
                    .strip_prefix("search.mark.clearStyle")
                    .and_then(|value| value.parse::<u8>().ok());
                let style = id
                    .strip_prefix("search.mark.style")
                    .and_then(|value| value.parse::<u8>().ok());
                if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                    if id == "search.mark.clearAll" || clear.is_some() {
                        editor.clear_search_marks(clear);
                    } else if let Some(style) = style {
                        let ranges = workspace
                            .find
                            .completed_results()
                            .filter(|results| results.is_current(editor.snapshot()))
                            .map(|results| {
                                results
                                    .matches()
                                    .iter()
                                    .map(|matched| matched.range.clone())
                                    .collect()
                            });
                        match ranges {
                            Some(ranges) => {
                                if let Err(error) = editor.set_search_marks(style, ranges) {
                                    workspace.message = Some(error);
                                }
                            }
                            None => {
                                workspace.message =
                                    Some("Complete the current search before marking".into())
                            }
                        }
                    }
                }
            }
            return true;
        }
        false
    }
    pub(super) fn search_pump(&mut self) -> bool {
        let mut replacement_changed = self.search_replace_pump();
        if let Some(index) = self
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.take_paged_search_activation())
        {
            self.app.active = index;
            replacement_changed = true;
        }
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if let Some(target) = workspace.search_panel.take_folder_activation() {
            let open = (0..workspace.editors.len())
                .any(|index| workspace.path(index) == Some(target.0.as_path()));
            if !open && !workspace.path_loading(&target.0) {
                workspace.open(target.0.clone());
            }
            self.search.navigation = Some(target);
        }
        let Some((path, fingerprint, range)) = &self.search.navigation else {
            return replacement_changed;
        };
        let index = (0..workspace.editors.len())
            .find(|index| workspace.path(*index) == Some(path.as_path()));
        let Some(index) = index else {
            if !workspace.path_loading(path) {
                self.search.navigation = None;
                return true;
            }
            return false;
        };
        if workspace.editors[index].busy() {
            return false;
        }
        if workspace.fingerprint(index) != Some(fingerprint) || workspace.editors[index].dirty() {
            workspace.message =
                Some("Search result changed; search the file again before navigating".into());
            self.search.navigation = None;
            return true;
        }
        let range = range.clone();
        match &mut workspace.editors[index] {
            WorkspaceEditor::Resident(editor) => {
                editor.enqueue(Input::SetCaret(range.start.0, false));
                editor.enqueue(Input::SetCaret(range.end.0, true));
                editor.search_selection = true;
            }
            WorkspaceEditor::Paged(editor) => {
                if let Err(error) = editor.restore_selection(range.start, range.end) {
                    workspace.message = Some(error);
                }
            }
        }
        self.app.active = index;
        self.search.navigation = None;
        true
    }
}
