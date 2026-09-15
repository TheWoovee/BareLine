// SPDX-License-Identifier: MPL-2.0
//! Native search scope commands and fingerprint-bound file-result navigation.
use super::*;
mod folder;
mod replace;
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_document::TextOffset;
use bareline_file_io::lifecycle::Fingerprint;
use bareline_search::folders::FolderScope;
use std::{ops::Range, sync::Arc};

pub(super) fn select_panel_tab(
    workspace: &mut bareline_app::workspace::Workspace,
    tab: bareline_app::search_panel::SearchTab,
) {
    match tab {
        bareline_app::search_panel::SearchTab::Find | bareline_app::search_panel::SearchTab::Replace => {
            let query = workspace.search_panel.query();
            workspace
                .search_panel
                .set_scope(bareline_app::search_panel::SearchScope::CurrentDocument);
            workspace.search_panel.hide();
            workspace.search_focus = false;
            let _ = workspace.find.set_query(&query);
            if tab == bareline_app::search_panel::SearchTab::Replace {
                workspace.find.show_replace();
            } else {
                workspace.find.show();
            }
        }
        bareline_app::search_panel::SearchTab::Files => {
            let query = workspace.find.query();
            workspace.find.blur();
            workspace.search_panel.show();
            workspace
                .search_panel
                .set_scope(bareline_app::search_panel::SearchScope::OpenDocuments);
            workspace.search_panel.field.select_all();
            workspace.search_panel.field.insert(&query.pattern);
            workspace.search_focus = true;
        }
    }
}

pub(super) fn accessibility_invoke_panel(
    workspace: &mut bareline_app::workspace::Workspace,
    id: u64,
) -> (bool, Option<usize>) {
    if !workspace.search_panel.owns_accessibility_id(id) {
        return (false, None);
    }
    let activation = workspace.search_panel.accessibility_activate(id);
    if let Some(tab) = workspace.search_panel.take_tab_requested() {
        select_panel_tab(workspace, tab);
        return (true, None);
    }
    let active = activation.and_then(|(source, range)| workspace.activate_search(source, range));
    (true, active)
}
#[derive(Default)]
pub(super) struct SearchRuntime {
    folder: folder::FolderControls,
    replace: replace::ReplaceRuntime,
    navigation: Option<(PathBuf, Fingerprint, Range<TextOffset>)>,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    replace::register(registry);
    let commands = [
        ("search.mode.literal", "Literal Search Mode"),
        ("search.mode.extended", "Extended Search Mode"),
        ("search.mode.regex", "Regular Expression Search Mode"),
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
                menu_path: "Search".into(),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}
impl SearchRuntime {
    pub(super) fn close_folder(&mut self) {
        self.folder.close();
    }
    pub(super) fn folder_ime_caret(&self) -> Option<bareline_renderer::Rect> {
        self.folder.ime_caret
    }
    pub(super) fn draw(
        &mut self,
        workspace: Option<&bareline_app::workspace::Workspace>,
        renderer: &mut WindowsRenderer,
        theme: bareline_ui::theme::UiTheme,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) {
        self.replace.draw(workspace, width, height, ops);
        self.folder.draw(renderer, theme, width, height, ops);
    }
}
impl Shell {
    pub(super) fn search_modal(&self) -> bool {
        self.search.replace.is_open() || self.search.folder.open
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
        if let Some(mode) = match id {
            "search.mode.literal" => Some(bareline_search::SearchMode::Literal),
            "search.mode.extended" => Some(bareline_search::SearchMode::Extended),
            "search.mode.regex" => Some(bareline_search::SearchMode::Regex),
            _ => None,
        } {
            if let Some(workspace) = &mut self.workspace {
                let mut query = workspace.find.query();
                query.mode = mode;
                let _ = workspace.find.set_query(&query);
                workspace.search_panel.set_scope(if query.selection.is_some() {
                    bareline_app::search_panel::SearchScope::Selection
                } else {
                    bareline_app::search_panel::SearchScope::CurrentDocument
                });
                workspace.find.show();
            }
            return true;
        }
        if id == "search.folder" {
            let selected = self.platform.as_ref().map(|platform| platform.pick_folder());
            match selected {
                Some(Ok(Some(root))) => {
                    if let Some(workspace) = &mut self.workspace {
                        let query = workspace.find.query();
                        workspace.find.blur();
                        workspace.search_focus = false;
                        workspace
                            .search_panel
                            .set_scope(bareline_app::search_panel::SearchScope::Folder);
                        self.search.folder.show(root, query);
                    }
                }
                Some(Err(error)) => {
                    self.toasts.push_typed(
                        "search-folder-picker",
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "Folder search could not choose a folder.",
                        Some(error),
                        None,
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
                _ => {}
            }
            return true;
        }
        if id == "search.scope.selection" || id == "search.scope.current" {
            if let Some(workspace) = &mut self.workspace {
                let scope = if id.ends_with("selection") {
                    let Some(selection) = self.views.active_selection(workspace, self.app.active) else {
                        workspace.message = Some("Wait for the current selection before searching it.".into());
                        return true;
                    };
                    Some(selection)
                } else {
                    None
                };
                workspace.find.set_selection_scope(scope);
                workspace.search_panel.set_scope(if id.ends_with("selection") {
                    bareline_app::search_panel::SearchScope::Selection
                } else {
                    bareline_app::search_panel::SearchScope::CurrentDocument
                });
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
                            .map(|results| results.matches().iter().map(|matched| matched.range.clone()).collect());
                        match ranges {
                            Some(ranges) => {
                                if let Err(error) = editor.set_search_marks(style, ranges) {
                                    let document = editor.snapshot().identity_token();
                                    self.toasts.push_typed(
                                        format!("search-mark-{}", document.0),
                                        toast::next_revision(),
                                        bareline_ui::theme::ToastLevel::Error,
                                        toast::NotificationKind::Outcome,
                                        "Search marks could not be updated.",
                                        Some(error),
                                        Some(document),
                                        toast::NotificationLifetime::Persistent,
                                        Instant::now(),
                                    );
                                }
                            }
                            None => workspace.message = Some("Complete the current search before marking".into()),
                        }
                    }
                }
            }
            return true;
        }
        false
    }

    pub(super) fn search_annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        use bareline_commands::{CommandId, CommandState};
        let Some(workspace) = self.workspace.as_ref() else {
            for id in [
                "search.mode.literal",
                "search.mode.extended",
                "search.mode.regex",
                "search.mode",
                "search.match_case",
                "search.whole_word",
                "search.scope.current",
                "search.scope.selection",
                "search.open_documents",
                "search.folder",
                "search.cancel",
                "search.close_find",
                "search.cancel_panel",
                "search.close_panel",
            ] {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Open a document first"));
            }
            context.states.insert(
                CommandId("search.folder"),
                CommandState {
                    enabled: false,
                    radio: true,
                    disabled_reason: Some("Open a document first".into()),
                    ..Default::default()
                },
            );
            self.search.replace.annotate_context(context, None);
            return;
        };

        let query = workspace.find.query();
        for (id, mode) in [
            ("search.mode.literal", bareline_search::SearchMode::Literal),
            ("search.mode.extended", bareline_search::SearchMode::Extended),
            ("search.mode.regex", bareline_search::SearchMode::Regex),
        ] {
            context.states.insert(
                CommandId(id),
                CommandState {
                    checked: query.mode == mode,
                    radio: true,
                    ..Default::default()
                },
            );
        }
        context.states.insert(
            CommandId("search.mode"),
            CommandState {
                label: Some(format!("Next Search Mode (current: {:?})", query.mode)),
                ..Default::default()
            },
        );

        let scope = match workspace.search_panel.scope() {
            bareline_app::search_panel::SearchScope::Folder => bareline_app::search_panel::SearchScope::Folder,
            bareline_app::search_panel::SearchScope::OpenDocuments => {
                bareline_app::search_panel::SearchScope::OpenDocuments
            }
            bareline_app::search_panel::SearchScope::CurrentDocument
            | bareline_app::search_panel::SearchScope::Selection => {
                if query.selection.is_some() {
                    bareline_app::search_panel::SearchScope::Selection
                } else {
                    bareline_app::search_panel::SearchScope::CurrentDocument
                }
            }
        };
        for (id, checked) in [
            (
                "search.scope.current",
                scope == bareline_app::search_panel::SearchScope::CurrentDocument,
            ),
            (
                "search.scope.selection",
                scope == bareline_app::search_panel::SearchScope::Selection,
            ),
            (
                "search.open_documents",
                scope == bareline_app::search_panel::SearchScope::OpenDocuments,
            ),
            (
                "search.folder",
                scope == bareline_app::search_panel::SearchScope::Folder,
            ),
        ] {
            context.states.insert(
                CommandId(id),
                CommandState {
                    checked,
                    radio: true,
                    ..Default::default()
                },
            );
        }
        if self
            .views
            .active_selection(workspace, self.app.active)
            .is_none_or(|selection| selection.is_empty())
        {
            context.states.insert(
                CommandId("search.scope.selection"),
                CommandState {
                    enabled: false,
                    checked: scope == bareline_app::search_panel::SearchScope::Selection,
                    radio: true,
                    disabled_reason: Some("Select text in the active document first".into()),
                    ..Default::default()
                },
            );
        }
        context.states.insert(
            CommandId("search.cancel"),
            if workspace.find.searching() {
                CommandState::default()
            } else {
                CommandState::disabled("No current-document search is running")
            },
        );
        context.states.insert(
            CommandId("search.close_find"),
            if workspace.find.open {
                CommandState::default()
            } else {
                CommandState::disabled("The Find bar is closed")
            },
        );
        context.states.insert(
            CommandId("search.cancel_panel"),
            if workspace.search_panel.searching() {
                CommandState::default()
            } else {
                CommandState::disabled("No open-document or folder search is running")
            },
        );
        context.states.insert(
            CommandId("search.close_panel"),
            if workspace.search_panel.open {
                CommandState::default()
            } else {
                CommandState::disabled("The Search Results panel is closed")
            },
        );
        self.search.replace.annotate_context(context, Some(workspace));
    }
    pub(super) fn search_pump(&mut self) -> bool {
        let mut replacement_changed = self.search_replace_pump();
        if let Some(index) = self
            .workspace
            .as_mut()
            .and_then(|workspace| workspace.take_paged_search_activation())
        {
            self.app.active = index;
            if let Some(workspace) = &mut self.workspace {
                workspace.bind_find_to(index);
            }
            replacement_changed = true;
        }
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if let Some(target) = workspace.search_panel.take_folder_activation() {
            let open = (0..workspace.editors.len()).any(|index| workspace.path(index) == Some(target.0.as_path()));
            if !open && !workspace.path_loading(&target.0) {
                workspace.open(target.0.clone());
            }
            self.search.navigation = Some(target);
        }
        let Some((path, fingerprint, range)) = &self.search.navigation else {
            return replacement_changed;
        };
        let index = (0..workspace.editors.len()).find(|index| workspace.path(*index) == Some(path.as_path()));
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
            let document = workspace.editors[index].snapshot().identity_token();
            self.toasts.push_typed(
                format!("search-stale-{}", document.0),
                toast::next_revision(),
                bareline_ui::theme::ToastLevel::Warning,
                toast::NotificationKind::Outcome,
                "Search result changed; search the file again before navigating.",
                None,
                Some(document),
                toast::NotificationLifetime::Persistent,
                Instant::now(),
            );
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
                    let document = editor.snapshot().identity_token();
                    self.toasts.push_typed(
                        format!("search-navigation-{}", document.0),
                        toast::next_revision(),
                        bareline_ui::theme::ToastLevel::Error,
                        toast::NotificationKind::Outcome,
                        "Search result could not be selected.",
                        Some(error),
                        Some(document),
                        toast::NotificationLifetime::Persistent,
                        Instant::now(),
                    );
                }
            }
        }
        self.app.active = index;
        workspace.bind_find_to(index);
        self.search.navigation = None;
        true
    }
}

#[cfg(test)]
mod menu_projection_tests {
    use super::*;
    use bareline_app::workspace::Input;
    use bareline_commands::{CommandContext, MenuItem};

    fn checked(context: &CommandContext, id: &'static str) -> bool {
        context.states.get(&CommandId(id)).is_some_and(|state| state.checked)
    }

    fn settle(workspace: &mut bareline_app::workspace::Workspace) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors.iter().any(WorkspaceEditor::busy) {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn scope_mode_and_options_follow_current_intent_after_panel_cancellation() {
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace = bareline_app::workspace::Workspace::new(
            Arc::new(|| {}),
            Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        for _ in 0..2 {
            workspace.new_document().unwrap();
        }
        workspace.editors[0].enqueue(Input::Insert("needle".into()));
        settle(&mut workspace);
        workspace.bind_find_to(0);
        let mut query = workspace.find.query();
        query.mode = bareline_search::SearchMode::Regex;
        query.case = bareline_search::Case::Sensitive;
        query.whole_word = true;
        workspace.find.set_query(&query).unwrap();
        workspace
            .search_panel
            .set_scope(bareline_app::search_panel::SearchScope::Folder);
        workspace.search_panel.show();
        workspace.search_panel.cancel();
        shell.workspace = Some(workspace);

        let context = shell.command_context();
        assert!(checked(&context, "search.folder"));
        assert!(checked(&context, "search.mode.regex"));
        assert!(checked(&context, "search.match_case"));
        assert!(checked(&context, "search.whole_word"));

        assert!(shell.search_command("search.mode.extended"));
        let context = shell.command_context();
        assert!(checked(&context, "search.scope.current"));
        assert!(!checked(&context, "search.folder"));
        assert!(checked(&context, "search.mode.extended"));

        shell.workspace.as_mut().unwrap().editors[0].enqueue(Input::SetCaret(0, false));
        shell.workspace.as_mut().unwrap().editors[0].enqueue(Input::SetCaret(3, true));
        settle(shell.workspace.as_mut().unwrap());
        assert!(shell.search_command("search.scope.selection"));
        let context = shell.command_context();
        assert!(checked(&context, "search.scope.selection"));
        assert!(shell.workspace.as_ref().unwrap().find.query().selection.is_some());

        shell.app.active = 1;
        shell.workspace.as_mut().unwrap().bind_find_to(1);
        assert!(shell.workspace.as_ref().unwrap().find.query().selection.is_none());
        let context = shell.command_context();
        assert!(checked(&context, "search.scope.current"));
        assert!(!checked(&context, "search.scope.selection"));

        assert!(shell.search_command("search.scope.current"));
        let context = shell.command_context();
        assert!(checked(&context, "search.scope.current"));

        select_panel_tab(
            shell.workspace.as_mut().unwrap(),
            bareline_app::search_panel::SearchTab::Files,
        );
        let context = shell.command_context();
        assert!(checked(&context, "search.open_documents"));
    }

    fn top_level<'a>(model: &'a bareline_commands::MenuModel, title: &str) -> &'a [MenuItem] {
        model
            .items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu {
                    title: candidate,
                    items,
                } if candidate == title => Some(items.as_slice()),
                _ => None,
            })
            .expect("composed top-level menu")
    }

    #[test]
    fn composed_shell_file_and_search_popups_fit_supported_heights() {
        let mut app = bareline_app::App::default();
        super::super::register_all_commands(&mut app.commands);
        let model = bareline_app::menus::curated_model(&app.commands);
        for title in ["File", "Search"] {
            let items = top_level(&model, title);
            let estimated_height = items.len() * 28 + 8;
            for viewport in [600, 768, 800] {
                assert!(
                    estimated_height <= viewport,
                    "composed {title} popup {estimated_height}px exceeds {viewport}px"
                );
            }
            assert!(
                items.len() <= 14,
                "composed {title} has {} first-level rows",
                items.len()
            );
        }
    }
}
