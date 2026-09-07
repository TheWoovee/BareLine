// SPDX-License-Identifier: MPL-2.0
//! Bottom search results controller. Only visible rows read bounded resident excerpts.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_search::{
    Case, Completeness, SearchMode, SearchQuery,
    service::{OpenDocumentTicket, SearchWorker},
    sources::OpenDocumentResults,
};
use bareline_ui::{
    controls::{Button, ControlState, Key, visible_rows},
    text_field::TextField,
    widgets::{SemanticAction, SemanticRole, Semantics},
    *,
};
use std::{
    ops::Range,
    sync::{Arc, mpsc::TryRecvError},
};

pub const HEIGHT: f32 = 260.0;
const ROW_HEIGHT: f32 = 28.0;
type Activation = (DocumentSnapshot, Range<TextOffset>);
pub fn register_commands(
    registry: &mut bareline_commands::CommandRegistry,
) -> Result<(), bareline_commands::CommandId> {
    use bareline_commands::{Action, CommandId, CommandSpec};
    for (id, title, shortcut) in [
        (
            "search.open_documents",
            "Find in Open Documents",
            "Ctrl+Shift+F",
        ),
        ("search.cancel_panel", "Cancel Open Document Search", ""),
        ("search.close_panel", "Close Search Results", ""),
    ] {
        let id = CommandId(id);
        registry.register(CommandSpec {
            id,
            title,
            category: "Search",
            shortcut,
            action: Action::Contributed(id),
        })?;
    }
    Ok(())
}
#[derive(Default)]
pub struct SearchPanel {
    pub open: bool,
    pub focused: bool,
    pub field: TextField,
    worker: Option<SearchWorker>,
    pending: Option<OpenDocumentTicket>,
    results: Option<OpenDocumentResults>,
    query: Option<SearchQuery>,
    status: String,
    offsets: Vec<usize>,
    collapsed: Vec<bool>,
    selected: Option<usize>,
    scroll: f64,
    bounds: Rect,
    list_bounds: Rect,
    field_bounds: Rect,
    search_requested: bool,
    visible_names: Vec<(usize, String)>,
}
impl SearchPanel {
    pub fn owns_accessibility_id(&self, id: u64) -> bool { self.semantics().iter().any(|node| node.id.0 == id) }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn accessibility_activate(&mut self, id: u64) -> Option<Activation> {
        self.semantic_action(ViewId(id), SemanticAction::Invoke)
    }
    pub fn accessibility_focus(&mut self, id: u64) {
        self.semantic_action(ViewId(id), SemanticAction::Focus);
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        let mut field = Semantics::new(
            ViewId(7000),
            SemanticRole::TextField,
            "Find in open documents",
            "search.open_documents",
            self.field_bounds,
            ControlState {
                focused: self.focused,
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::SetValue);
        field.value = Some(self.field.value().into());
        let mut nodes = vec![field];
        for (id, name, command, bounds, disabled) in [
            (
                7001,
                "Search open documents",
                "search.open_documents",
                rect(
                    self.bounds.x + self.bounds.width - 108.0,
                    self.bounds.y + 40.0,
                    96.0,
                    28.0,
                ),
                self.field.value().is_empty() && self.query().mode != SearchMode::Regex,
            ),
            (
                7002,
                "Cancel search",
                "search.cancel_panel",
                rect(
                    self.bounds.x + self.bounds.width - 90.0,
                    self.bounds.y + self.bounds.height - 28.0,
                    82.0,
                    24.0,
                ),
                self.pending.is_none(),
            ),
            (
                7003,
                "Close search results",
                "search.close_panel",
                rect(
                    self.bounds.x + self.bounds.width - 32.0,
                    self.bounds.y + 3.0,
                    28.0,
                    28.0,
                ),
                false,
            ),
        ] {
            nodes.push(
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    name,
                    command,
                    bounds,
                    ControlState {
                        disabled,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Invoke),
            );
        }
        for (row, name) in &self.visible_names {
            let Some((group, matched)) = self.row(*row) else {
                continue;
            };
            let mut node = Semantics::new(
                ViewId(2000000 + *row as u64),
                if matched.is_some() {
                    SemanticRole::ListItem
                } else {
                    SemanticRole::TreeItem
                },
                name,
                "search.result.activate",
                rect(
                    self.list_bounds.x,
                    self.list_bounds.y + (*row as f64 * ROW_HEIGHT as f64 - self.scroll) as f32,
                    self.list_bounds.width,
                    ROW_HEIGHT,
                ),
                ControlState {
                    focused: !self.focused && self.selected == Some(*row),
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Invoke);
            node.selected = self.selected == Some(*row);
            if matched.is_none() {
                node.expanded = Some(!self.collapsed[group]);
            }
            nodes.push(node);
        }
        nodes
    }
    pub fn semantic_action(&mut self, id: ViewId, action: SemanticAction) -> Option<Activation> {
        if !self.open {
            return None;
        }
        match id.0 {
            7000 if action == SemanticAction::Focus => self.focused = true,
            7001 if action == SemanticAction::Invoke => self.search_requested = true,
            7002 if action == SemanticAction::Invoke => self.cancel(),
            7003 if action == SemanticAction::Invoke => self.hide(),
            number if number >= 2000000 => {
                let row = (number - 2000000) as usize;
                if row < self.row_count() {
                    self.selected = Some(row);
                    self.focused = false;
                    self.reveal();
                    if action == SemanticAction::Invoke {
                        return self.activate();
                    }
                }
            }
            _ => {}
        }
        None
    }
    pub fn show(&mut self) {
        self.open = true;
        self.focused = true;
        self.field.select_all();
    }
    pub fn hide(&mut self) {
        self.open = false;
        self.focused = false;
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        self.field.release(backend);
    }
    pub fn height(&self) -> f32 {
        if self.open { HEIGHT } else { 0.0 }
    }
    pub fn query(&self) -> SearchQuery {
        let mut query = self
            .query
            .clone()
            .unwrap_or_else(|| SearchQuery::literal(""));
        query.pattern = self.field.value().into();
        query.selection = None;
        query
    }
    pub fn take_search_requested(&mut self) -> bool {
        std::mem::take(&mut self.search_requested)
    }
    pub fn start(
        &mut self,
        snapshots: Vec<DocumentSnapshot>,
        mut query: SearchQuery,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.open = true;
        query.selection = None;
        if self.field.value() != query.pattern {
            self.field.select_all();
            self.field.insert(&query.pattern);
        }
        self.query = Some(query.clone());
        self.results = None;
        self.visible_names.clear();
        self.offsets.clear();
        self.collapsed.clear();
        self.selected = None;
        self.scroll = 0.0;
        self.pending = None;
        if query.pattern.is_empty() && query.mode != SearchMode::Regex {
            self.status = "Type to find".into();
            return;
        }
        if self.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(_) => {
                    self.status = "Search worker unavailable".into();
                    return;
                }
            }
        }
        self.status = "Searching open documents…".into();
        self.pending = Some(
            self.worker
                .as_ref()
                .unwrap()
                .submit_open_documents(snapshots, query, notify),
        );
    }
    pub fn cancel(&mut self) {
        if let Some(ticket) = &self.pending {
            ticket.job.cancel();
            self.status = "Stopping search…".into();
        }
    }
    pub fn pump(&mut self) -> bool {
        let Some(ticket) = &self.pending else {
            return false;
        };
        match ticket.try_recv() {
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) | Ok(Err(_)) => {
                self.pending = None;
                self.status = "Search stopped".into();
                true
            }
            Ok(Ok(results)) => {
                self.pending = None;
                self.status = match results.completeness() {
                    Completeness::Complete => format!(
                        "{} matches in {} open documents",
                        results.count(),
                        results.documents().iter().filter(|d| !d.is_empty()).count()
                    ),
                    Completeness::Cancelled => {
                        format!("Search cancelled · {} partial matches", results.count())
                    }
                    Completeness::ResultLimit => {
                        format!("{} matches · Result limit reached", results.count())
                    }
                    Completeness::InvalidQuery => {
                        "Invalid query · Check pattern and options".into()
                    }
                    Completeness::UnsupportedStreaming => {
                        "Results incomplete · Regex subject exceeds 16 MiB".into()
                    }
                    Completeness::RegexLimit => "Results incomplete · Regex resource limit".into(),
                    Completeness::Unsupported => "Results incomplete · Source unavailable".into(),
                };
                self.collapsed = vec![false; results.documents().len()];
                self.results = Some(results);
                self.rebuild_rows();
                true
            }
        }
    }
    fn rebuild_rows(&mut self) {
        self.offsets.clear();
        self.offsets.push(0);
        if let Some(results) = &self.results {
            for (index, document) in results.documents().iter().enumerate() {
                let rows = if document.is_empty() {
                    0
                } else {
                    1 + if self.collapsed[index] {
                        0
                    } else {
                        document.count()
                    }
                };
                self.offsets
                    .push(self.offsets.last().copied().unwrap() + rows);
            }
        }
        if self.selected.is_some_and(|row| row >= self.row_count()) {
            self.selected = None;
        }
    }
    fn row_count(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }
    fn row(&self, row: usize) -> Option<(usize, Option<usize>)> {
        if row >= self.row_count() {
            return None;
        }
        let group = self
            .offsets
            .partition_point(|offset| *offset <= row)
            .saturating_sub(1);
        let local = row - self.offsets[group];
        Some((group, local.checked_sub(1)))
    }
    fn activate(&mut self) -> Option<Activation> {
        let (group, matched) = self.row(self.selected?)?;
        if let Some(index) = matched {
            let document = &self.results.as_ref()?.documents()[group];
            Some((
                document.source().clone(),
                document.matches().get(index)?.range.clone(),
            ))
        } else {
            self.collapsed[group] = !self.collapsed[group];
            self.rebuild_rows();
            None
        }
    }
    fn reveal(&mut self) {
        if let Some(row) = self.selected {
            let y = row as f64 * ROW_HEIGHT as f64;
            if y < self.scroll {
                self.scroll = y;
            }
            if y + ROW_HEIGHT as f64 > self.scroll + self.list_bounds.height as f64 {
                self.scroll = (y + ROW_HEIGHT as f64 - self.list_bounds.height as f64).max(0.0);
            }
        }
    }
    pub fn scroll(&mut self, delta: f64) {
        let limit =
            (self.row_count() as f64 * ROW_HEIGHT as f64 - self.list_bounds.height as f64).max(0.0);
        if delta.is_finite() {
            self.scroll = (self.scroll + delta).clamp(0.0, limit);
        }
    }
    pub fn key(&mut self, key: Key) -> Option<Activation> {
        match key {
            Key::Escape => {
                self.focused = false;
                self.field.cancel();
            }
            Key::Enter if self.focused => self.search_requested = true,
            Key::Enter | Key::Space => return self.activate(),
            Key::Down | Key::Up | Key::Home | Key::End if self.row_count() != 0 => {
                self.focused = false;
                self.selected = Some(match key {
                    Key::Home => 0,
                    Key::End => self.row_count() - 1,
                    Key::Up => self.selected.unwrap_or(1).saturating_sub(1),
                    _ => (self.selected.map_or(0, |i| i + 1)).min(self.row_count() - 1),
                });
                self.reveal();
            }
            Key::Tab => {
                self.focused = !self.focused;
                if !self.focused && self.selected.is_none() && self.row_count() > 0 {
                    self.selected = Some(0);
                }
            }
            _ => {}
        }
        None
    }
    pub fn pointer(&mut self, point: Point) -> Option<Activation> {
        if !self.open || !self.bounds.contains(point) {
            return None;
        }
        if self.field_bounds.contains(point) {
            self.focused = true;
            return None;
        }
        self.focused = false;
        if rect(
            self.bounds.x + self.bounds.width - 32.0,
            self.bounds.y + 3.0,
            28.0,
            28.0,
        )
        .contains(point)
        {
            self.hide();
            return None;
        }
        if rect(
            self.bounds.x + self.bounds.width - 108.0,
            self.bounds.y + 40.0,
            96.0,
            28.0,
        )
        .contains(point)
        {
            self.search_requested = true;
            return None;
        }
        if rect(
            self.bounds.x + self.bounds.width - 90.0,
            self.bounds.y + self.bounds.height - 28.0,
            82.0,
            24.0,
        )
        .contains(point)
        {
            self.cancel();
            return None;
        }
        if self.list_bounds.contains(point) {
            let row = ((point.y - self.list_bounds.y) as f64 + self.scroll) / ROW_HEIGHT as f64;
            self.selected =
                (row >= 0.0 && (row as usize) < self.row_count()).then_some(row as usize);
            return self.activate();
        }
        None
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        labels: &[(DocumentSnapshot, String)],
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            return Ok(None);
        }
        let top = (height - STATUS_HEIGHT - HEIGHT).max(TAB_HEIGHT + 48.0);
        self.bounds = rect(
            6.0,
            top,
            width - 12.0,
            (height - STATUS_HEIGHT - top).max(0.0),
        );
        self.field_bounds = rect(
            24.0,
            top + 40.0,
            (width * 0.40).max(80.0).min((width - 160.0).max(80.0)),
            28.0,
        );
        self.list_bounds = rect(18.0, top + 78.0, width - 36.0, self.bounds.height - 110.0);
        self.scroll(0.0);
        ops.push(DrawOp::FillRounded(self.bounds, ELEVATED, 4.0));
        ops.push(DrawOp::StrokeRounded(self.bounds, BORDER, 4.0, 1.0));
        ops.push(DrawOp::PushClip(self.bounds));
        text(ops, 26.0, top + 10.0, "Find", 13.0, TEXT);
        text(ops, 94.0, top + 10.0, "Replace", 13.0, MUTED);
        text(ops, 184.0, top + 10.0, "Files", 13.0, MUTED);
        ops.push(DrawOp::Fill(rect(18.0, top + 31.0, 58.0, 2.0), ACCENT));
        text(ops, width - 32.0, top + 9.0, "×", 14.0, TEXT);
        let caret = self
            .field
            .draw(backend, self.field_bounds, self.focused, ops)?;
        let query = self.query();
        let mode = match query.mode {
            SearchMode::Literal => "Literal",
            SearchMode::Extended => "Extended",
            SearchMode::Regex => "Regex",
        };
        if width >= 650.0 {
            text(
                ops,
                self.field_bounds.x + self.field_bounds.width + 14.0,
                top + 47.0,
                format!(
                    "{mode} · Open documents{}{}",
                    if query.case == Case::Sensitive {
                        " · Match case"
                    } else {
                        ""
                    },
                    if query.whole_word {
                        " · Whole word"
                    } else {
                        ""
                    }
                ),
                13.0,
                MUTED,
            );
        }
        Button {
            id: ViewId(7001),
            label: "Search".into(),
            bounds: rect(width - 114.0, top + 40.0, 96.0, 28.0),
            toggle: false,
            state: ControlState {
                disabled: self.field.value().is_empty() && query.mode != SearchMode::Regex,
                ..Default::default()
            },
        }
        .paint(ops);
        ops.push(DrawOp::PushClip(self.list_bounds));
        self.visible_names.clear();
        for row in visible_rows(
            self.scroll,
            self.list_bounds.height as f64,
            ROW_HEIGHT as f64,
            Some(self.row_count()),
            1,
        ) {
            let Some((group, matched)) = self.row(row) else {
                continue;
            };
            let document = &self.results.as_ref().unwrap().documents()[group];
            let y = self.list_bounds.y + (row as f64 * ROW_HEIGHT as f64 - self.scroll) as f32;
            if self.selected == Some(row) {
                ops.push(DrawOp::Fill(
                    rect(18.0, y, width - 36.0, ROW_HEIGHT),
                    EDITOR,
                ));
            }
            if let Some(index) = matched {
                let range = &document.matches()[index].range;
                let snapshot = document.source();
                let mut start = range.start.0.saturating_sub(60);
                while !snapshot.is_boundary(TextOffset(start)) {
                    start += 1;
                }
                let mut end = (start + 160).min(snapshot.len());
                while !snapshot.is_boundary(TextOffset(end)) {
                    end -= 1;
                }
                let excerpt = snapshot
                    .read(TextOffset(start)..TextOffset(end), 160)
                    .unwrap_or_default()
                    .replace(['\r', '\n', '\t'], " ");
                let line = snapshot
                    .line_at(range.start)
                    .map(|line| (line + 1).to_string())
                    .unwrap_or_else(|_| "?".into());
                text(ops, 52.0, y + 6.0, &line, 13.0, MUTED);
                text(ops, 108.0, y + 6.0, &excerpt, 13.0, TEXT);
                self.visible_names
                    .push((row, format!("Line {line}: {excerpt}")));
            } else {
                let name = labels
                    .iter()
                    .find(|(snapshot, _)| snapshot.same_document(document.source()))
                    .map(|(_, name)| name.as_str())
                    .unwrap_or("Closed document");
                text(
                    ops,
                    24.0,
                    y + 6.0,
                    if self.collapsed[group] { "›" } else { "⌄" },
                    14.0,
                    TEXT,
                );
                self.visible_names
                    .push((row, format!("{name}, {} matches", document.count())));
                text(
                    ops,
                    48.0,
                    y + 6.0,
                    format!("{name}   {}", document.count()),
                    13.0,
                    TEXT,
                );
            }
        }
        ops.push(DrawOp::PopClip);
        text(
            ops,
            24.0,
            top + self.bounds.height - 22.0,
            if self.status.is_empty() {
                "Type to find · Open documents"
            } else {
                &self.status
            },
            13.0,
            MUTED,
        );
        if self.pending.is_some() {
            text(
                ops,
                width - 86.0,
                top + self.bounds.height - 22.0,
                "Cancel",
                13.0,
                ACCENT,
            );
        }
        ops.push(DrawOp::PopClip);
        Ok(self.focused.then_some(caret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    use bareline_renderer::balanced_clips;
    use bareline_renderer_recording::RecordingBackend;
    #[test]
    fn grouped_panel_draws_only_visible_rows_and_activates_exact_source() {
        let snapshot = Document::from_utf8(
            &"x\n".repeat(20_000),
            Budget::new(1024 * 1024),
            Budget::new(1024 * 1024),
        )
        .unwrap()
        .snapshot();
        let mut panel = SearchPanel::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        panel.start(
            vec![snapshot.clone()],
            SearchQuery::literal("x"),
            Arc::new(move || {
                let _ = sender.send(());
            }),
        );
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(panel.pump());
        assert_eq!(panel.row_count(), 20_001);
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        panel
            .draw(
                &mut backend,
                1200.0,
                800.0,
                &[(snapshot.clone(), "example.txt".into())],
                &mut ops,
            )
            .unwrap();
        assert!(balanced_clips(&ops));
        assert!(ops.len() < 100);
        panel.key(Key::Home);
        panel.key(Key::Down);
        let (source, range) = panel.key(Key::Enter).unwrap();
        assert!(source.same_document(&snapshot));
        assert_eq!(range, TextOffset(0)..TextOffset(1));
        panel.key(Key::Home);
        assert!(panel.key(Key::Enter).is_none());
        assert_eq!(panel.row_count(), 1);
        panel.key(Key::Escape);
        assert!(panel.open);
        panel.release(&mut backend);
    }
    #[test]
    fn panel_commands_and_search_request_keep_explicit_scope() {
        let mut registry = bareline_commands::CommandRegistry::default();
        register_commands(&mut registry).unwrap();
        assert!(
            registry
                .dispatch(bareline_commands::CommandId("search.open_documents"))
                .is_some()
        );
        let mut panel = SearchPanel::default();
        panel.show();
        panel.field.insert("needle");
        panel.key(Key::Enter);
        assert!(panel.take_search_requested());
        assert!(!panel.take_search_requested());
        assert_eq!(panel.query().pattern, "needle");
        assert!(panel.query().selection.is_none());
    }
}
