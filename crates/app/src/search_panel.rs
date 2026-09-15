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
pub const FIND_TAB_ID: u64 = 7_100;
pub const REPLACE_TAB_ID: u64 = 7_101;
pub const FILES_TAB_ID: u64 = 7_102;
const ROW_HEIGHT: f32 = 28.0;
type Activation = (DocumentSnapshot, Range<TextOffset>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchTab {
    Find,
    Replace,
    Files,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchScope {
    #[default]
    CurrentDocument,
    Selection,
    OpenDocuments,
    Folder,
}
pub fn register_commands(
    registry: &mut bareline_commands::CommandRegistry,
) -> Result<(), bareline_commands::CommandId> {
    use bareline_commands::{Action, CommandId, CommandSpec};
    for (id, title, shortcut) in [
        ("search.open_documents", "Find in Open Documents", "Ctrl+Shift+F"),
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
    scope: SearchScope,
    worker: Option<SearchWorker>,
    pending: Option<OpenDocumentTicket>,
    folder_pending: Option<bareline_search::service::FolderSearchTicket>,
    folder_options: Option<(
        bareline_search::folders::FolderScope,
        Arc<dyn bareline_platform::PathTrustProvider + Send + Sync>,
        Arc<dyn bareline_platform::LocalFileSystem>,
        Arc<dyn Fn() + Send + Sync>,
    )>,
    folder_results: Option<bareline_search::folders::FolderResults>,
    folder_activation: Option<(
        std::path::PathBuf,
        bareline_file_io::lifecycle::Fingerprint,
        Range<TextOffset>,
    )>,
    results: Option<OpenDocumentResults>,
    paged_activation: Option<(bareline_document::paged::PagedSnapshot, Range<TextOffset>)>,
    query: Option<SearchQuery>,
    status: String,
    offsets: Vec<usize>,
    collapsed: Vec<bool>,
    selected: Option<usize>,
    scroll: f64,
    bounds: Rect,
    list_bounds: Rect,
    field_bounds: Rect,
    draw_offset: Point,
    search_requested: bool,
    tab_requested: Option<SearchTab>,
    semantic_focus: Option<u64>,
    accessibility_revision: std::cell::Cell<u64>,
    accessibility_fingerprint: std::cell::Cell<u64>,
    visible_names: Vec<(usize, String)>,
}
impl SearchPanel {
    pub fn scope(&self) -> SearchScope {
        self.scope
    }
    pub fn set_scope(&mut self, scope: SearchScope) {
        self.scope = scope;
    }
    pub fn owns_accessibility_id(&self, id: u64) -> bool {
        self.semantics().iter().any(|node| node.id.0 == id)
    }
    pub fn folder_receipt(&self) -> Option<&bareline_search::folders::FolderResults> {
        self.folder_results.as_ref()
    }
    pub fn folder_searching(&self) -> bool {
        self.folder_pending.is_some()
    }
    pub fn searching(&self) -> bool {
        self.pending.is_some() || self.folder_pending.is_some()
    }
    pub fn folder_job(&self) -> Option<bareline_search::SearchJob> {
        self.folder_pending.as_ref().map(|ticket| ticket.job.clone())
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn dock_state(&self) -> (bool, bool, u64) {
        (
            self.open,
            self.pending.is_some() || self.folder_pending.is_some(),
            self.accessibility_revision.get(),
        )
    }
    pub fn accessibility_text_field(&self) -> Option<(u64, u64, &TextField)> {
        if !self.open || !self.focused || self.semantic_focus.is_some_and(|id| id != 7000) {
            return None;
        }
        use std::hash::{Hash, Hasher};
        let mut state = std::collections::hash_map::DefaultHasher::new();
        self.field.value().hash(&mut state);
        self.field.selection().hash(&mut state);
        self.field.composition_text().hash(&mut state);
        let fingerprint = state.finish();
        if fingerprint != self.accessibility_fingerprint.get() {
            self.accessibility_fingerprint.set(fingerprint);
            self.accessibility_revision.set(
                self.accessibility_revision
                    .get()
                    .checked_add(1)
                    .expect("search accessibility revision exhausted"),
            );
        }
        Some((7000, self.accessibility_revision.get(), &self.field))
    }
    pub fn accessibility_activate(&mut self, id: u64) -> Option<Activation> {
        self.semantic_action(ViewId(id), SemanticAction::Invoke)
    }
    pub fn accessibility_focus(&mut self, id: u64) {
        self.semantic_action(ViewId(id), SemanticAction::Focus);
    }
    pub fn accessibility_focus_id(&self) -> Option<u64> {
        self.semantics()
            .into_iter()
            .find(|node| node.focused)
            .map(|node| node.id.0)
    }
    pub fn take_tab_requested(&mut self) -> Option<SearchTab> {
        self.tab_requested.take()
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
                focused: self.focused && self.semantic_focus.is_none_or(|id| id == 7000),
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::SetValue);
        field.value = Some(self.field.value().into());
        let mut nodes = vec![field];
        for (id, name, tab, bounds) in [
            (
                FIND_TAB_ID,
                "Find",
                SearchTab::Find,
                rect(self.bounds.x + 12.0, self.bounds.y, 58.0, 34.0),
            ),
            (
                REPLACE_TAB_ID,
                "Replace",
                SearchTab::Replace,
                rect(self.bounds.x + 70.0, self.bounds.y, 88.0, 34.0),
            ),
            (
                FILES_TAB_ID,
                "Files",
                SearchTab::Files,
                rect(self.bounds.x + 158.0, self.bounds.y, 70.0, 34.0),
            ),
        ] {
            let mut node = Semantics::new(
                ViewId(id),
                SemanticRole::Tab,
                name,
                match tab {
                    SearchTab::Find => "search.find",
                    SearchTab::Replace => "search.replace",
                    SearchTab::Files => "search.open_documents",
                },
                bounds,
                ControlState {
                    focused: self.semantic_focus == Some(id),
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Invoke)
            .action(SemanticAction::Select);
            node.selected = tab == SearchTab::Files;
            nodes.push(node);
        }
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
                self.pending.is_none() && self.folder_pending.is_none(),
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
                        focused: self.semantic_focus == Some(id),
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
                    focused: self.semantic_focus == Some(2_000_000 + *row as u64)
                        || (self.semantic_focus.is_none() && !self.focused && self.selected == Some(*row)),
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
            7000 if action == SemanticAction::Focus => {
                self.focused = true;
                self.semantic_focus = Some(7000);
            }
            7001..=7003 if action == SemanticAction::Focus => {
                self.focused = false;
                self.semantic_focus = Some(id.0);
            }
            7001 if action == SemanticAction::Invoke => self.search_requested = true,
            7002 if action == SemanticAction::Invoke => self.cancel(),
            7003 if action == SemanticAction::Invoke => self.hide(),
            FIND_TAB_ID if matches!(action, SemanticAction::Invoke | SemanticAction::Select) => {
                self.tab_requested = Some(SearchTab::Find)
            }
            REPLACE_TAB_ID if matches!(action, SemanticAction::Invoke | SemanticAction::Select) => {
                self.tab_requested = Some(SearchTab::Replace)
            }
            FILES_TAB_ID if matches!(action, SemanticAction::Invoke | SemanticAction::Select) => {
                self.tab_requested = Some(SearchTab::Files)
            }
            FIND_TAB_ID..=FILES_TAB_ID if action == SemanticAction::Focus => {
                self.focused = false;
                self.semantic_focus = Some(id.0);
            }
            number if number >= 2000000 => {
                let row = (number - 2000000) as usize;
                if row < self.row_count() {
                    self.selected = Some(row);
                    self.focused = false;
                    self.semantic_focus = Some(number);
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
        self.accessibility_revision.set(
            self.accessibility_revision
                .get()
                .checked_add(1)
                .expect("search accessibility revision exhausted"),
        );
        self.open = true;
        self.focused = true;
        self.semantic_focus = Some(7000);
        self.field.select_all();
    }
    pub fn hide(&mut self) {
        self.open = false;
        self.focused = false;
        self.semantic_focus = None;
    }
    pub fn release(&mut self, backend: &mut impl TextBackend) {
        self.field.release(backend);
    }
    pub fn height(&self) -> f32 {
        if self.open { HEIGHT } else { 0.0 }
    }
    pub fn query(&self) -> SearchQuery {
        let mut query = self.query.clone().unwrap_or_else(|| SearchQuery::literal(""));
        query.pattern = self.field.value().into();
        query.selection = None;
        query
    }
    pub fn take_search_requested(&mut self) -> bool {
        let requested = std::mem::take(&mut self.search_requested);
        if requested && let Some((scope, trust, platform, notify)) = self.folder_options.clone() {
            self.start_folder(scope, self.query(), trust, platform, notify);
            return false;
        }
        requested
    }
    pub fn start(&mut self, snapshots: Vec<DocumentSnapshot>, query: SearchQuery, notify: Arc<dyn Fn() + Send + Sync>) {
        self.start_mixed(snapshots, Vec::new(), query, notify);
    }
    pub fn take_paged_activation(&mut self) -> Option<(bareline_document::paged::PagedSnapshot, Range<TextOffset>)> {
        self.paged_activation.take()
    }
    pub fn start_mixed(
        &mut self,
        snapshots: Vec<DocumentSnapshot>,
        paged: Vec<bareline_search::sources::PagedOpenDocument>,
        mut query: SearchQuery,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.scope = SearchScope::OpenDocuments;
        self.accessibility_revision.set(
            self.accessibility_revision
                .get()
                .checked_add(1)
                .expect("search accessibility revision exhausted"),
        );
        self.paged_activation = None;
        self.open = true;
        self.folder_options = None;
        self.folder_pending = None;
        self.folder_results = None;
        self.folder_activation = None;
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
                .submit_mixed_open_documents(snapshots, paged, query, notify),
        );
    }
    pub fn start_folder(
        &mut self,
        scope: bareline_search::folders::FolderScope,
        mut query: SearchQuery,
        trust: Arc<dyn bareline_platform::PathTrustProvider + Send + Sync>,
        platform: Arc<dyn bareline_platform::LocalFileSystem>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.scope = SearchScope::Folder;
        self.accessibility_revision.set(
            self.accessibility_revision
                .get()
                .checked_add(1)
                .expect("search accessibility revision exhausted"),
        );
        self.folder_options = Some((scope.clone(), trust.clone(), platform.clone(), notify.clone()));
        self.open = true;
        self.pending = None;
        self.folder_pending = None;
        self.results = None;
        self.folder_results = None;
        self.offsets.clear();
        self.selected = None;
        self.scroll = 0.0;
        self.visible_names.clear();
        query.selection = None;
        self.field.select_all();
        self.field.insert(&query.pattern);
        self.query = Some(query.clone());
        if self.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            }
        }
        self.folder_pending = Some(
            self.worker
                .as_ref()
                .unwrap()
                .submit_folder(scope, query, trust, platform, notify),
        );
        self.status = "Searching files…".into();
    }
    pub fn take_folder_activation(
        &mut self,
    ) -> Option<(
        std::path::PathBuf,
        bareline_file_io::lifecycle::Fingerprint,
        Range<TextOffset>,
    )> {
        self.folder_activation.take()
    }
    pub fn cancel(&mut self) {
        if let Some(ticket) = &self.folder_pending {
            ticket.job.cancel();
            self.status = "Stopping search…".into();
        }
        if let Some(ticket) = &self.pending {
            ticket.job.cancel();
            self.status = "Stopping search…".into();
        }
    }
    pub fn pump(&mut self) -> bool {
        if let Some(ticket) = &self.folder_pending {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                Ok(Ok(results)) => {
                    self.status = format!(
                        "{} matches; {} files searched; {} skipped; {:?}",
                        results.summary.count,
                        results.summary.searched_files,
                        results.summary.skipped_files,
                        results.summary.completeness
                    );
                    self.collapsed = vec![false; results.groups.len()];
                    self.folder_results = Some(results);
                    self.folder_pending = None;
                    self.rebuild_rows();
                    return true;
                }
                _ => {
                    self.folder_pending = None;
                    self.status = "File search stopped".into();
                    return true;
                }
            }
        }
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
                            + results.paged.iter().filter(|d| !d.results.matches.is_empty()).count()
                    ),
                    Completeness::Cancelled => {
                        format!("Search cancelled · {} partial matches", results.count())
                    }
                    Completeness::ResultLimit => {
                        format!("{} matches · Result limit reached", results.count())
                    }
                    Completeness::InvalidQuery => "Invalid query · Check pattern and options".into(),
                    Completeness::UnsupportedStreaming => "Results incomplete · Regex context exceeds 64 MiB".into(),
                    Completeness::RegexLimit => "Results incomplete · Regex resource limit".into(),
                    Completeness::Unsupported => "Results incomplete · Source unavailable".into(),
                };
                self.collapsed = vec![false; results.documents().len() + results.paged.len()];
                self.results = Some(results);
                self.rebuild_rows();
                true
            }
        }
    }
    fn rebuild_rows(&mut self) {
        self.offsets.clear();
        self.offsets.push(0);
        if let Some(results) = &self.folder_results {
            for (index, group) in results.groups.iter().enumerate() {
                let rows = 1 + if self.collapsed[index] { 0 } else { group.matches.len() };
                self.offsets.push(self.offsets.last().copied().unwrap() + rows);
            }
        }
        if let Some(results) = &self.results {
            for (index, document) in results.documents().iter().enumerate() {
                let rows = if document.is_empty() {
                    0
                } else {
                    1 + if self.collapsed[index] {
                        0
                    } else {
                        document.matches().len()
                    }
                };
                self.offsets.push(self.offsets.last().copied().unwrap() + rows);
            }
        }
        if let Some(results) = &self.results {
            for (index, document) in results.paged.iter().enumerate() {
                let rows = if document.results.matches.is_empty() {
                    0
                } else {
                    1 + if self.collapsed[results.documents().len() + index] {
                        0
                    } else {
                        document.results.matches.len()
                    }
                };
                self.offsets.push(self.offsets.last().copied().unwrap() + rows);
            }
        }
        let row_count = self.row_count();
        if let Some(selected) = self.selected {
            self.selected = if row_count == 0 {
                None
            } else {
                Some(selected.min(row_count - 1))
            };
            if self.semantic_focus.is_some_and(|id| id >= 2_000_000) {
                self.semantic_focus = self.selected.map(|row| 2_000_000 + row as u64);
            }
        }
    }
    fn row_count(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }
    fn row(&self, row: usize) -> Option<(usize, Option<usize>)> {
        if row >= self.row_count() {
            return None;
        }
        let group = self.offsets.partition_point(|offset| *offset <= row).saturating_sub(1);
        let local = row - self.offsets[group];
        Some((group, local.checked_sub(1)))
    }
    fn activate(&mut self) -> Option<Activation> {
        let (group, matched) = self.row(self.selected?)?;
        if let Some(index) = matched {
            if let Some(results) = &self.folder_results {
                let file = &results.groups[group];
                self.folder_activation = Some((
                    file.path.clone(),
                    file.fingerprint.clone(),
                    file.matches.get(index)?.range.clone(),
                ));
                return None;
            }
            let results = self.results.as_ref()?;
            if group >= results.documents().len() {
                let document = &results.paged[group - results.documents().len()];
                self.paged_activation = Some((
                    document.results.source.clone(),
                    document.results.matches.get(index)?.range.clone(),
                ));
                return None;
            }
            let document = &self.results.as_ref()?.documents()[group];
            Some((document.source().clone(), document.matches().get(index)?.range.clone()))
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
        let limit = (self.row_count() as f64 * ROW_HEIGHT as f64 - self.list_bounds.height as f64).max(0.0);
        if delta.is_finite() {
            self.scroll = (self.scroll + delta).clamp(0.0, limit);
        }
    }
    pub fn key(&mut self, key: Key) -> Option<Activation> {
        match key {
            Key::Escape => {
                self.focused = false;
                self.semantic_focus = None;
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
                self.semantic_focus = self.selected.map(|row| 2_000_000 + row as u64);
                self.reveal();
            }
            Key::Tab => {
                self.focused = !self.focused;
                if !self.focused && self.selected.is_none() && self.row_count() > 0 {
                    self.selected = Some(0);
                }
                self.semantic_focus = if self.focused {
                    Some(7000)
                } else {
                    self.selected.map(|row| 2_000_000 + row as u64)
                };
            }
            _ => {}
        }
        None
    }
    pub fn pointer(&mut self, point: Point) -> Option<Activation> {
        if !self.open || !self.bounds.contains(point) {
            return None;
        }
        for (bounds, tab) in [
            (rect(self.bounds.x + 12.0, self.bounds.y, 58.0, 34.0), SearchTab::Find),
            (
                rect(self.bounds.x + 70.0, self.bounds.y, 88.0, 34.0),
                SearchTab::Replace,
            ),
            (rect(self.bounds.x + 158.0, self.bounds.y, 70.0, 34.0), SearchTab::Files),
        ] {
            if bounds.contains(point) {
                self.tab_requested = Some(tab);
                self.focused = false;
                self.semantic_focus = Some(match tab {
                    SearchTab::Find => FIND_TAB_ID,
                    SearchTab::Replace => REPLACE_TAB_ID,
                    SearchTab::Files => FILES_TAB_ID,
                });
                return None;
            }
        }
        if self.field_bounds.contains(point) {
            self.focused = true;
            self.semantic_focus = Some(7000);
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
            self.semantic_focus = Some(7001);
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
            self.semantic_focus = Some(7002);
            return None;
        }
        if self.list_bounds.contains(point) {
            let row = ((point.y - self.list_bounds.y) as f64 + self.scroll) / ROW_HEIGHT as f64;
            self.selected = (row >= 0.0 && (row as usize) < self.row_count()).then_some(row as usize);
            self.semantic_focus = self.selected.map(|row| 2_000_000 + row as u64);
            return self.activate();
        }
        None
    }
    pub fn pointer_with_backend(
        &mut self,
        backend: &impl TextBackend,
        point: Point,
        extend: bool,
    ) -> Result<Option<Activation>, LayoutError> {
        let activation = self.pointer(point);
        if self.open && self.field_bounds.contains(point) {
            self.field.click(
                backend,
                Point {
                    x: point.x - self.draw_offset.x,
                    y: point.y - self.draw_offset.y,
                },
                extend,
            )?;
            self.focused = true;
            self.semantic_focus = Some(7000);
        }
        Ok(activation)
    }
    fn draw_canvas(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        panel_height: f32,
        labels: &[(DocumentSnapshot, String)],
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            return Ok(None);
        }
        self.draw_offset = Point::default();
        let top = (height - STATUS_HEIGHT - panel_height).max(TAB_HEIGHT + 48.0);
        self.bounds = rect(
            6.0,
            top,
            (width - 12.0).max(0.0),
            (height - STATUS_HEIGHT - top).max(0.0),
        );
        self.field_bounds = rect(
            24.0,
            top + 40.0,
            (width * 0.40).max(80.0).min((width - 160.0).max(80.0)),
            28.0,
        );
        self.list_bounds = rect(
            18.0,
            top + 78.0,
            (width - 36.0).max(0.0),
            (self.bounds.height - 110.0).max(0.0),
        );
        self.scroll(0.0);
        ops.push(DrawOp::FillRounded(self.bounds, ELEVATED, 4.0));
        ops.push(DrawOp::StrokeRounded(self.bounds, BORDER, 4.0, 1.0));
        ops.push(DrawOp::PushClip(self.bounds));
        text(ops, 26.0, top + 10.0, "Find", 13.0, MUTED);
        text(ops, 94.0, top + 10.0, "Replace", 13.0, MUTED);
        text(ops, 184.0, top + 10.0, "Files", 13.0, TEXT);
        ops.push(DrawOp::Fill(rect(164.0, top + 31.0, 70.0, 2.0), ACCENT));
        if let Some(bounds) = match self.semantic_focus {
            Some(FIND_TAB_ID) => Some(rect(18.0, top, 58.0, 34.0)),
            Some(REPLACE_TAB_ID) => Some(rect(76.0, top, 88.0, 34.0)),
            Some(FILES_TAB_ID) => Some(rect(164.0, top, 70.0, 34.0)),
            _ => None,
        } {
            ops.push(DrawOp::Stroke(bounds, ACCENT, 2.0));
        }
        text(ops, width - 32.0, top + 9.0, "×", 14.0, TEXT);
        let caret = self.field.draw(backend, self.field_bounds, self.focused, ops)?;
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
                    "{mode} · {}{}{}",
                    if self.folder_results.is_some() || self.folder_pending.is_some() {
                        "Files"
                    } else {
                        "Open documents"
                    },
                    if query.case == Case::Sensitive {
                        " · Match case"
                    } else {
                        ""
                    },
                    if query.whole_word { " · Whole word" } else { "" }
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
            if let Some(results) = &self.folder_results {
                let file = &results.groups[group];
                let y = self.list_bounds.y + (row as f64 * ROW_HEIGHT as f64 - self.scroll) as f32;
                if self.selected == Some(row) {
                    ops.push(DrawOp::Fill(rect(18.0, y, width - 36.0, ROW_HEIGHT), EDITOR));
                }
                let label = if let Some(index) = matched {
                    let found = &file.matches[index];
                    format!(
                        "{}; line unavailable; byte {}; {}",
                        file.path.display(),
                        found.range.start.0,
                        found.excerpt.replace(['\r', '\n', '\t'], " ")
                    )
                } else {
                    format!(
                        "{} {}   {} matches",
                        if self.collapsed[group] { "›" } else { "⌄" },
                        file.path.display(),
                        file.matches.len()
                    )
                };
                text(
                    ops,
                    if matched.is_some() { 52.0 } else { 24.0 },
                    y + 6.0,
                    &label,
                    13.0,
                    TEXT,
                );
                self.visible_names.push((row, label));
                continue;
            }
            let results = self.results.as_ref().unwrap();
            if group >= results.documents().len() {
                let document = &results.paged[group - results.documents().len()];
                let y = self.list_bounds.y + (row as f64 * ROW_HEIGHT as f64 - self.scroll) as f32;
                if self.selected == Some(row) {
                    ops.push(DrawOp::Fill(rect(18.0, y, width - 36.0, ROW_HEIGHT), EDITOR));
                }
                let label = if let Some(index) = matched {
                    format!(
                        "{}; line unavailable; byte {}; {}",
                        document.label,
                        document.results.matches[index].range.start.0,
                        document.excerpts[index].replace(['\r', '\n', '\t'], " ")
                    )
                } else {
                    format!(
                        "{} {}   {}",
                        if self.collapsed[group] { "›" } else { "⌄" },
                        document.label,
                        document.results.count
                    )
                };
                text(
                    ops,
                    if matched.is_some() { 52.0 } else { 24.0 },
                    y + 6.0,
                    &label,
                    13.0,
                    TEXT,
                );
                self.visible_names.push((row, label));
                continue;
            }
            let document = &self.results.as_ref().unwrap().documents()[group];
            let y = self.list_bounds.y + (row as f64 * ROW_HEIGHT as f64 - self.scroll) as f32;
            if self.selected == Some(row) {
                ops.push(DrawOp::Fill(rect(18.0, y, width - 36.0, ROW_HEIGHT), EDITOR));
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
                let name = labels
                    .iter()
                    .find(|(candidate, _)| candidate.same_document(snapshot))
                    .map(|(_, name)| name.as_str())
                    .unwrap_or("Closed document");
                self.visible_names
                    .push((row, format!("{name}; line {line}; {excerpt}")));
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
                text(ops, 48.0, y + 6.0, format!("{name}   {}", document.count()), 13.0, TEXT);
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
        if self.pending.is_some() || self.folder_pending.is_some() {
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
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        labels: &[(DocumentSnapshot, String)],
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        self.draw_canvas(backend, width, height, HEIGHT, labels, ops)
    }
    /// Draw Search inside a shell-owned dock body while keeping hit testing and
    /// accessibility bounds in the shell's editor coordinate space.
    pub fn draw_in(
        &mut self,
        backend: &mut impl TextBackend,
        bounds: Rect,
        labels: &[(DocumentSnapshot, String)],
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Ok(None);
        }
        let mut local = Vec::new();
        let canvas_width = bounds.width + 12.0;
        let canvas_height = TAB_HEIGHT + 48.0 + bounds.height + STATUS_HEIGHT;
        let caret = self.draw_canvas(backend, canvas_width, canvas_height, bounds.height, labels, &mut local)?;
        let dx = bounds.x - self.bounds.x;
        let dy = bounds.y - self.bounds.y;
        self.draw_offset = Point { x: dx, y: dy };
        self.field_bounds = translate_rect(self.field_bounds, dx, dy);
        self.list_bounds = translate_rect(self.list_bounds, dx, dy);
        for op in &mut local {
            translate_draw_op(op, dx, dy);
        }
        self.bounds = bounds;
        self.list_bounds.width = self.list_bounds.width.min(bounds.width).max(0.0);
        self.list_bounds.height = self
            .list_bounds
            .height
            .min((bounds.y + bounds.height - self.list_bounds.y).max(0.0))
            .max(0.0);
        ops.push(DrawOp::PushClip(bounds));
        ops.extend(local);
        ops.push(DrawOp::PopClip);
        Ok(caret.map(|caret| translate_rect(caret, dx, dy)))
    }
}

fn translate_rect(bounds: Rect, x: f32, y: f32) -> Rect {
    rect(bounds.x + x, bounds.y + y, bounds.width, bounds.height)
}
fn translate_draw_op(op: &mut DrawOp, x: f32, y: f32) {
    let point = |point: Point| Point {
        x: point.x + x,
        y: point.y + y,
    };
    match op {
        DrawOp::Fill(bounds, _)
        | DrawOp::Stroke(bounds, _, _)
        | DrawOp::FillRounded(bounds, _, _)
        | DrawOp::StrokeRounded(bounds, _, _, _)
        | DrawOp::PushClip(bounds)
        | DrawOp::PushLayer { bounds, .. } => *bounds = translate_rect(*bounds, x, y),
        DrawOp::Text { origin, .. } | DrawOp::Layout { origin, .. } => *origin = point(*origin),
        DrawOp::Image { destination, .. } => *destination = translate_rect(*destination, x, y),
        DrawOp::Line { from, to, .. } => {
            *from = point(*from);
            *to = point(*to);
        }
        DrawOp::PopClip | DrawOp::PopLayer => {}
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
        receiver.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
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
        let first_result = panel
            .semantics()
            .into_iter()
            .find(|node| node.id.0 == 2_000_001)
            .unwrap();
        assert!(first_result.name.contains("example.txt; line 1;"));
        assert!(first_result.actions.contains(&SemanticAction::Invoke));
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
    #[test]
    fn external_dock_bounds_own_search_hit_and_semantic_geometry() {
        let mut panel = SearchPanel::default();
        panel.show();
        let dock = rect(12.0, 410.0, 476.0, 150.0);
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        panel.draw_in(&mut backend, dock, &[], &mut ops).unwrap();
        assert_eq!(panel.bounds, dock);
        assert!(dock.contains(Point {
            x: panel.field_bounds.x,
            y: panel.field_bounds.y,
        }));
        assert!(panel.semantics().iter().all(|node| {
            node.bounds.x >= dock.x
                && node.bounds.y >= dock.y
                && node.bounds.x + node.bounds.width <= dock.x + dock.width + f32::EPSILON
                && node.bounds.y + node.bounds.height <= dock.y + dock.height + f32::EPSILON
        }));
        assert!(balanced_clips(&ops));
        panel.field.insert("abcdef");
        ops.clear();
        panel.draw_in(&mut backend, dock, &[], &mut ops).unwrap();
        let search = rect(dock.x + dock.width - 108.0, dock.y + 40.0, 96.0, 28.0);
        assert!(
            ops.iter()
                .any(|op| matches!(op, DrawOp::StrokeRounded(bounds, _, _, _) if *bounds == search))
        );
        assert!(ops.iter().any(|op| matches!(op, DrawOp::Text { origin, text, .. }
            if text == "×" && *origin == Point { x: dock.x + dock.width - 26.0, y: dock.y + 9.0 })));
        let glyph = backend.measure("a", 13.0).0;
        panel
            .pointer_with_backend(
                &backend,
                Point {
                    x: panel.field_bounds.x + 8.0,
                    y: panel.field_bounds.y + panel.field_bounds.height / 2.0,
                },
                false,
            )
            .unwrap();
        assert_eq!(panel.field.selection(), (0, 0));
        panel
            .pointer_with_backend(
                &backend,
                Point {
                    x: panel.field_bounds.x + 8.0 + glyph * 3.0,
                    y: panel.field_bounds.y + panel.field_bounds.height / 2.0,
                },
                false,
            )
            .unwrap();
        assert_eq!(panel.field.selection(), (3, 3));
        panel
            .pointer_with_backend(
                &backend,
                Point {
                    x: panel.field_bounds.x + 8.0 + glyph * 5.0,
                    y: panel.field_bounds.y + panel.field_bounds.height / 2.0,
                },
                true,
            )
            .unwrap();
        assert_eq!(panel.field.selection(), (3, 5));
        assert_eq!(
            panel
                .semantics()
                .into_iter()
                .find(|node| node.id.0 == 7001)
                .unwrap()
                .bounds,
            search
        );
        panel.pointer(Point {
            x: search.x + search.width - 1.0,
            y: search.y + search.height / 2.0,
        });
        assert!(panel.take_search_requested());
        panel.pointer(Point {
            x: dock.x + dock.width - 5.0,
            y: dock.y + 15.0,
        });
        assert!(!panel.open);
    }

    #[test]
    fn accessibility_tabs_select_modes_and_provider_revisions_do_not_recycle() {
        let mut panel = SearchPanel::default();
        panel.show();
        let first = panel.accessibility_text_field().unwrap().1;
        panel.field.insert("é");
        let changed = panel.accessibility_text_field().unwrap().1;
        assert!(changed > first);
        panel.field.set_selection(0, "é".len());
        let selected = panel.accessibility_text_field().unwrap().1;
        assert!(selected > changed);

        let semantics = panel.semantics();
        for id in [FIND_TAB_ID, REPLACE_TAB_ID, FILES_TAB_ID] {
            let tab = semantics.iter().find(|node| node.id.0 == id).unwrap();
            assert_eq!(tab.role, SemanticRole::Tab);
            assert!(tab.actions.contains(&SemanticAction::Invoke));
            assert!(tab.actions.contains(&SemanticAction::Select));
        }
        assert!(
            semantics
                .iter()
                .find(|node| node.id.0 == FILES_TAB_ID)
                .unwrap()
                .selected
        );
        panel.semantic_action(ViewId(REPLACE_TAB_ID), SemanticAction::Select);
        assert_eq!(panel.take_tab_requested(), Some(SearchTab::Replace));

        panel.hide();
        panel.show();
        let reopened = panel.accessibility_text_field().unwrap().1;
        assert!(reopened > selected);

        panel.selected = Some(4);
        panel.semantic_focus = Some(2_000_004);
        panel.offsets = vec![0, 5];
        panel.rebuild_rows();
        assert_eq!(panel.row_count(), 0);
        assert_eq!(panel.selected, None);
        assert_eq!(panel.semantic_focus, None);
    }
}
