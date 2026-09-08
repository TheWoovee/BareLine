// SPDX-License-Identifier: MPL-2.0
//! Explicit, bounded folder search preferences; worker contracts own scanning.
use super::*;
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::{
    ViewId,
    controls::ControlState,
    rect, text,
    text_field::TextField,
    widgets::{SemanticAction, SemanticRole, Semantics},
};
const BASE: u64 = 77_000;

pub(super) struct FolderControls {
    pub open: bool,
    root: Option<PathBuf>,
    query: bareline_search::SearchQuery,
    fields: [TextField; 3],
    bounds: [Rect; 7],
    focus: usize,
    include_binary: bool,
    count_beyond_limit: bool,
    error: Option<String>,
    pub ime_caret: Option<Rect>,
}
impl Default for FolderControls {
    fn default() -> Self {
        let mut excluded = TextField::default();
        excluded.insert(".git, .svn, .hg");
        Self {
            open: false,
            root: None,
            query: bareline_search::SearchQuery::literal(""),
            fields: [TextField::default(), TextField::default(), excluded],
            bounds: [Rect::default(); 7],
            focus: 0,
            include_binary: false,
            count_beyond_limit: false,
            error: None,
            ime_caret: None,
        }
    }
}
fn filters(value: &str, extensions: bool) -> Result<Vec<String>, String> {
    if value.len() > 4096 {
        return Err("Filters must fit within 4096 bytes".into());
    }
    let mut result = Vec::new();
    for entry in value
        .split([',', ';'])
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let entry = if extensions {
            entry
                .strip_prefix("*.")
                .or_else(|| entry.strip_prefix('.'))
                .unwrap_or(entry)
        } else {
            entry
        };
        if extensions && entry == "*" {
            if value.trim() == "*" {
                return Ok(Vec::new());
            }
            return Err("Use * alone for all extensions".into());
        }
        if entry.is_empty()
            || entry.len() > 128
            || entry
                .chars()
                .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
            || (!extensions && matches!(entry, "." | ".."))
        {
            return Err(
                "Use comma-separated exact names; extension filters also accept *.rs".into(),
            );
        }
        if !result
            .iter()
            .any(|v: &String| v.eq_ignore_ascii_case(entry))
        {
            result.push(entry.to_owned());
        }
        if result.len() > 64 {
            return Err("Use at most 64 filters".into());
        }
    }
    Ok(result)
}
impl FolderControls {
    fn focus_control(&mut self, index: usize) {
        if index != self.focus {
            for field in &mut self.fields {
                field.cancel();
            }
        }
        self.focus = index;
    }
    pub fn show(&mut self, root: PathBuf, query: bareline_search::SearchQuery) {
        self.root = Some(root);
        self.fields[0].select_all();
        self.fields[0].insert(&query.pattern);
        self.query = query;
        self.open = true;
        self.focus = 0;
        self.error = None;
    }
    fn close(&mut self) {
        self.open = false;
        for field in &mut self.fields {
            field.cancel();
        }
    }
    fn request(&self) -> Result<(FolderScope, bareline_search::SearchQuery), String> {
        if self.fields.iter().any(TextField::composing) {
            return Err("Finish composing before starting search".into());
        }
        let mut scope = FolderScope::user(self.root.clone().ok_or("Choose a folder")?);
        scope.include_binary = self.include_binary;
        scope.extensions = filters(self.fields[1].value(), true)?;
        scope.excluded_directory_names = filters(self.fields[2].value(), false)?;
        let mut query = self.query.clone();
        query.pattern = self.fields[0].value().into();
        query.selection = None;
        query.count_beyond_limit = self.count_beyond_limit;
        Ok((scope, query))
    }
    pub fn draw(
        &mut self,
        renderer: &mut WindowsRenderer,
        theme: bareline_ui::theme::UiTheme,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) {
        self.ime_caret = None;
        if !self.open {
            for field in &mut self.fields {
                field.release(renderer);
            }
            return;
        }
        let panel = rect(0.0, (height - 330.0).max(0.0), width, 330.0_f32.min(height));
        ops.push(DrawOp::Fill(panel, theme.elevated));
        ops.push(DrawOp::Stroke(panel, theme.interactive, 1.0));
        ops.push(DrawOp::PushClip(panel));
        text(
            ops,
            16.0,
            panel.y + 10.0,
            "Find in Folder",
            16.0,
            theme.text,
        );
        text(
            ops,
            16.0,
            panel.y + 34.0,
            self.root
                .as_ref()
                .map_or_else(String::new, |p| p.display().to_string()),
            12.0,
            theme.muted,
        );
        for (index, label) in [
            "Find (uses current case / word / regex options)",
            "Extensions — comma-separated; empty means all",
            "Excluded directory names — comma-separated",
        ]
        .into_iter()
        .enumerate()
        {
            let y = panel.y + 58.0 + index as f32 * 52.0;
            text(ops, 16.0, y, label, 12.0, theme.muted);
            self.bounds[index] = rect(16.0, y + 17.0, (width - 32.0).max(1.0), 30.0);
            match self.fields[index].draw_with_theme(
                renderer,
                self.bounds[index],
                self.focus == index,
                theme,
                ops,
            ) {
                Ok(caret) => {
                    if self.focus == index {
                        self.ime_caret = Some(caret);
                    }
                }
                Err(error) => self.error = Some(format!("Search field layout failed: {error:?}")),
            }
        }
        for (index, label, checked) in [
            (3, "Include binary / NUL-heavy files", self.include_binary),
            (
                4,
                "Continue counting beyond retained result limit",
                self.count_beyond_limit,
            ),
        ] {
            self.bounds[index] = rect(
                16.0,
                panel.y + 216.0 + (index - 3) as f32 * 28.0,
                (width - 32.0).max(1.0),
                26.0,
            );
            let bounds = self.bounds[index];
            text(
                ops,
                bounds.x + 4.0,
                bounds.y + 4.0,
                format!("{} {label}", if checked { "☑" } else { "☐" }),
                13.0,
                theme.text,
            );
        }
        for (index, label, x) in [(5, "Search", 16.0), (6, "Cancel", 112.0)] {
            self.bounds[index] = rect(x, panel.y + 276.0, 88.0, 30.0);
            ops.push(DrawOp::Stroke(self.bounds[index], theme.interactive, 1.0));
            text(ops, x + 10.0, panel.y + 283.0, label, 13.0, theme.text);
        }
        ops.push(DrawOp::Stroke(self.bounds[self.focus], theme.focus, 2.0));
        if let Some(error) = &self.error {
            text(ops, 212.0, panel.y + 283.0, error, 12.0, theme.text);
        }
        ops.push(DrawOp::PopClip);
    }
    fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        let mut nodes = Vec::new();
        for (index, name) in [
            "Find in folder",
            "File extensions",
            "Excluded directory names",
            "Include binary files",
            "Count beyond result limit",
            "Search folder",
            "Cancel folder search",
        ]
        .into_iter()
        .enumerate()
        {
            let state = ControlState {
                focused: self.focus == index,
                checked: if index == 3 {
                    self.include_binary
                } else {
                    index == 4 && self.count_beyond_limit
                },
                ..Default::default()
            };
            let node = if index < 3 {
                self.fields[index].semantics(
                    ViewId(BASE + index as u64),
                    name,
                    "search.folder",
                    self.bounds[index],
                    state,
                )
            } else {
                Semantics::new(
                    ViewId(BASE + index as u64),
                    if index < 5 {
                        SemanticRole::Checkbox
                    } else {
                        SemanticRole::Button
                    },
                    name,
                    "search.folder",
                    self.bounds[index],
                    state,
                )
                .action(SemanticAction::Focus)
                .action(if index < 5 {
                    SemanticAction::Toggle
                } else {
                    SemanticAction::Invoke
                })
            };
            nodes.push(node);
        }
        if let Some(error) = &self.error {
            nodes.push(Semantics::new(
                ViewId(BASE + 7),
                SemanticRole::Alert,
                error,
                "search.folder",
                self.bounds[5],
                ControlState::default(),
            ));
        }
        nodes
    }
}
impl Shell {
    pub(in crate::windows_app) fn search_folder_open(&self) -> bool {
        self.search.folder.open
    }
    fn search_folder_activate(&mut self, index: usize) {
        match index {
            3 => self.search.folder.include_binary = !self.search.folder.include_binary,
            4 => self.search.folder.count_beyond_limit = !self.search.folder.count_beyond_limit,
            5 => match self.search.folder.request() {
                Ok((scope, query)) => {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.search_panel.start_folder(
                            scope,
                            query,
                            Arc::new(bareline_platform_windows::WindowsPathTrustProvider),
                            Arc::new(bareline_platform_windows::WindowsFileSystem),
                            self.notify.clone(),
                        );
                        workspace.search_focus = true;
                    }
                    self.search.folder.close();
                }
                Err(error) => self.search.folder.error = Some(error),
            },
            6 => self.search.folder.close(),
            _ => {}
        }
    }
    pub(in crate::windows_app) fn search_folder_event(&mut self, event: &WindowEvent) -> bool {
        if !self.search.folder.open || self.palette.open {
            return false;
        }
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().map_or(1.0, Window::scale_factor);
                self.pointer = Point {
                    x: (position.x / scale) as f32,
                    y: (position.y / scale) as f32,
                };
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(index) = self
                    .search
                    .folder
                    .bounds
                    .iter()
                    .position(|r| r.contains(self.pointer))
                {
                    self.search.folder.focus_control(index);
                    if index < 3 {
                        if let Some(renderer) = &self.renderer {
                            let _ = self.search.folder.fields[index].click(
                                renderer,
                                self.pointer,
                                self.modifiers.shift_key(),
                            );
                        }
                    } else {
                        self.search_folder_activate(index);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let focus = self.search.folder.focus;
                if event.logical_key == Key::Named(NamedKey::Tab) {
                    self.search.folder.focus_control(
                        (focus + if self.modifiers.shift_key() { 6 } else { 1 }) % 7,
                    );
                } else if event.logical_key == Key::Named(NamedKey::Escape) {
                    if focus < 3 && self.search.folder.fields[focus].composing() {
                        self.search.folder.fields[focus].cancel();
                    } else {
                        self.search.folder.close();
                    }
                } else if event.logical_key == Key::Named(NamedKey::Enter) {
                    self.search_folder_activate(if focus < 3 { 5 } else { focus });
                } else if focus < 3 {
                    let field = &mut self.search.folder.fields[focus];
                    if !field.composing() {
                        let shift = self.modifiers.shift_key();
                        if self.modifiers.control_key() && !self.modifiers.alt_key() {
                            if let Key::Character(key) = &event.logical_key {
                                match key.to_lowercase().as_str() {
                                    "a" => field.select_all(),
                                    "z" => field.undo(false),
                                    "y" => field.undo(true),
                                    "c" | "x" => {
                                        if !field.selected().is_empty() {
                                            if let Some(platform) = &self.platform {
                                                match platform
                                                    .set_clipboard_text(field.selected())
                                                {
                                                    Ok(()) => {
                                                        if key.eq_ignore_ascii_case("x") {
                                                            field.insert("");
                                                        }
                                                    }
                                                    Err(error) => {
                                                        self.search.folder.error =
                                                            Some(error.to_string())
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    "v" => {
                                        if let Some(platform) = &self.platform {
                                            match platform.clipboard_text() {
                                                Ok(value) => {
                                                    field.insert(&value);
                                                }
                                                Err(error) => {
                                                    self.search.folder.error =
                                                        Some(error.to_string())
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        } else {
                            match &event.logical_key {
                                Key::Named(NamedKey::ArrowLeft) => field.horizontal(false, shift),
                                Key::Named(NamedKey::ArrowRight) => field.horizontal(true, shift),
                                Key::Named(NamedKey::Home) => field.edge(false, shift),
                                Key::Named(NamedKey::End) => field.edge(true, shift),
                                Key::Named(NamedKey::Backspace) => {
                                    field.delete(false);
                                }
                                Key::Named(NamedKey::Delete) => {
                                    field.delete(true);
                                }
                                Key::Dead(_) => {}
                                _ => {
                                    if let Some(value) = &event.text {
                                        field.insert(value);
                                    }
                                }
                            }
                        }
                    }
                } else if matches!(&event.logical_key, Key::Named(NamedKey::Space))
                    || matches!(&event.logical_key,Key::Character(v)if v==" ")
                {
                    self.search_folder_activate(focus);
                }
            }
            WindowEvent::Ime(ime) => {
                let focus = self.search.folder.focus;
                if focus < 3 {
                    let field = &mut self.search.folder.fields[focus];
                    match ime {
                        winit::event::Ime::Preedit(value, cursor) => {
                            field.preedit(value.clone(), *cursor)
                        }
                        winit::event::Ime::Commit(value) => {
                            field.commit(value);
                        }
                        winit::event::Ime::Disabled => field.cancel(),
                        _ => {}
                    }
                }
            }
            WindowEvent::Focused(false) => {
                for field in &mut self.search.folder.fields {
                    field.cancel();
                }
                return false;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return false;
            }
            WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. } => {}
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(in crate::windows_app) fn search_folder_semantics(&self) -> Vec<Semantics> {
        self.search.folder.semantics()
    }
    pub(in crate::windows_app) fn search_folder_accessibility(
        &mut self,
        id: u64,
        invoke: bool,
        value: Option<&str>,
    ) -> bool {
        if !self.search.folder.open || !(BASE..BASE + 7).contains(&id) {
            return false;
        }
        let index = (id - BASE) as usize;
        self.search.folder.focus_control(index);
        if let Some(value) = value {
            if index >= 3 || value.len() > 4096 || value.chars().any(char::is_control) {
                return false;
            }
            let field = &mut self.search.folder.fields[index];
            field.cancel();
            field.select_all();
            field.insert(value);
        }
        if invoke {
            self.search_folder_activate(index);
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_filter_composition_never_changes_the_search_scope() {
        let mut controls = FolderControls::default();
        controls.show(
            PathBuf::from("fixture"),
            bareline_search::SearchQuery::literal("needle"),
        );
        controls.focus_control(1);
        controls.fields[1].preedit("*.rs".into(), Some((4, 4)));
        assert!(controls.request().is_err());
        controls.focus_control(3);
        assert!(controls.request().unwrap().0.extensions.is_empty());
        assert_eq!(controls.fields[1].value(), "");
        controls.close();
        assert!(!controls.open);
    }
    #[test]
    fn folder_preferences_are_explicit_and_preserve_safe_defaults() {
        let mut controls = FolderControls::default();
        controls.show(
            PathBuf::from("fixture"),
            bareline_search::SearchQuery::literal("needle"),
        );
        let (scope, query) = controls.request().unwrap();
        assert!(!scope.include_binary);
        assert!(!query.count_beyond_limit);
        assert!(scope.extensions.is_empty());
        assert_eq!(scope.excluded_directory_names, [".git", ".svn", ".hg"]);
        controls.fields[1].insert("*.rs, .toml");
        controls.include_binary = true;
        controls.count_beyond_limit = true;
        let (scope, query) = controls.request().unwrap();
        assert_eq!(scope.extensions, ["rs", "toml"]);
        assert!(scope.include_binary && query.count_beyond_limit);
        assert_eq!(query.pattern, "needle");
        controls.fields[1].select_all();
        controls.fields[1].insert("../*.rs");
        assert!(controls.request().is_err());
    }
}
