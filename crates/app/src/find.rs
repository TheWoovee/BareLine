// SPDX-License-Identifier: MPL-2.0
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_search::{
    Case, Completeness, ReplaceScope, SearchMode, SearchQuery, SearchResults,
    service::{ReplaceTicket, SearchTicket, SearchWorker},
};
use bareline_ui::{
    controls::{Button, ControlState},
    text_field::TextField,
    *,
};
use std::sync::Arc;
pub const HEIGHT: f32 = 52.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindAction {
    Case,
    WholeWord,
    Previous,
    Next,
    Close,
    Replace,
    ReplaceOne,
    ReplaceAll,
    Mode,
    Cancel,
}
pub struct FindController {
    pub open: bool,
    pub focused: bool,
    pub field: TextField,
    pub replacement: TextField,
    pub replacing: bool,
    replacement_focus: bool,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub status: String,
    selection_scope: Option<std::ops::Range<TextOffset>>,
    cancelled_query: Option<SearchQuery>,
    worker: Option<SearchWorker>,
    pending: Option<SearchTicket>,
    paged_pending: Option<bareline_search::service::PagedSearchTicket>,
    paged_results: Option<Arc<bareline_search::paged::PagedResults>>,
    paged_requested: Option<(bareline_document::paged::PagedSnapshot, SearchQuery)>,
    results: Option<Arc<SearchResults>>,
    requested: Option<(DocumentSnapshot, String, bool, bool, SearchMode)>,
    pressed: Option<FindAction>,
    keyboard_focus: Option<FindAction>,
}
impl Default for FindController {
    fn default() -> Self {
        Self {
            open: false,
            focused: false,
            field: TextField::default(),
            replacement: TextField::default(),
            replacing: false,
            replacement_focus: false,
            mode: SearchMode::Literal,
            case_sensitive: false,
            whole_word: false,
            status: "Type to find".into(),
            selection_scope: None,
            cancelled_query: None,
            worker: None,
            pending: None,
            paged_pending: None,
            paged_results: None,
            paged_requested: None,
            results: None,
            requested: None,
            pressed: None,
            keyboard_focus: None,
        }
    }
}
impl FindController {
    /// Captures the selection once; later result navigation does not move its bounds.
    pub fn set_selection_scope(&mut self, selection: Option<std::ops::Range<TextOffset>>) {
        if self.selection_scope != selection {
            self.cancel_search();
            self.requested = None;
            self.results = None;
            self.selection_scope = selection;
        }
    }
    pub fn set_query(&mut self, query: &SearchQuery) -> Result<(), &'static str> {
        let mut field = TextField::default();
        if !query.pattern.is_empty() && !field.insert(&query.pattern) {
            return Err("Query exceeds the find field limit or contains control characters");
        }
        self.cancel_search();
        self.field = field;
        self.mode = query.mode;
        self.case_sensitive = query.case == Case::Sensitive;
        self.whole_word = query.whole_word;
        self.set_selection_scope(query.selection.clone());
        self.requested = None;
        self.results = None;
        self.paged_results = None;
        self.cancelled_query = None;
        Ok(())
    }
    pub fn completed_paged_results(&self) -> Option<&bareline_search::paged::PagedResults> {
        self.paged_results
            .as_deref()
            .filter(|results| results.completeness == Completeness::Complete)
    }
    pub fn query(&self) -> SearchQuery {
        let mut query = SearchQuery::literal(self.field.value());
        query.mode = self.mode;
        query.case = if self.case_sensitive {
            Case::Sensitive
        } else {
            Case::Folded
        };
        query.whole_word = self.whole_word;
        query.selection = self.selection_scope.clone();
        query
    }
    pub fn completed_results(&self) -> Option<&SearchResults> {
        self.results
            .as_deref()
            .filter(|results| results.completeness() == Completeness::Complete)
    }
    pub fn searching(&self) -> bool {
        self.pending.is_some() || self.paged_pending.is_some()
    }
    pub fn refresh_paged(
        &mut self,
        handle: bareline_editor_surface::paged_view::PagedReadHandle,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        if !self.open || self.field.composing() || self.cancelled_query.as_ref() == Some(&self.query()) {
            return;
        }
        let query = self.query();
        let snapshot = handle.snapshot().clone();
        if self
            .paged_requested
            .as_ref()
            .is_some_and(|(source, previous)| {
                source.same_document(&snapshot)
                    && source.content_state == snapshot.content_state
                    && previous == &query
            })
        {
            return;
        }
        self.paged_pending = None;
        self.paged_results = None;
        self.paged_requested = Some((snapshot.clone(), query.clone()));
        if query.pattern.is_empty() && query.mode != SearchMode::Regex {
            self.status = "Type to find".into();
            return;
        }
        if self.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.status = error.to_string();
                    return;
                }
            }
        }
        self.paged_pending = Some(self.worker.as_ref().unwrap().submit_paged(
            snapshot,
            query,
            move |ticket| handle.resolve_page(ticket),
            notify,
        ));
        self.status = "Searching full document…".into();
    }
    pub fn next_paged(
        &self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        at: usize,
        backwards: bool,
    ) -> Option<std::ops::Range<TextOffset>> {
        if self.field.composing()
            || !self
                .paged_requested
                .as_ref()
                .is_some_and(|(_, query)| query == &self.query())
        {
            return None;
        }
        let results = self.paged_results.as_ref()?;
        if !results.source.same_document(snapshot)
            || results.source.content_state != snapshot.content_state
        {
            return None;
        }
        let index = results
            .matches
            .partition_point(|matched| matched.range.start.0 < at);
        let matched = if backwards {
            index
                .checked_sub(1)
                .and_then(|i| results.matches.get(i))
                .or_else(|| results.matches.last())
        } else {
            results
                .matches
                .get(index)
                .or_else(|| results.matches.first())
        };
        matched.map(|matched| matched.range.clone())
    }

    pub fn show(&mut self) {
        self.cancelled_query = None;
        if self.results.is_none() {
            self.requested = None;
        }
        self.open = true;
        self.focused = true;
        self.keyboard_focus = None;
        self.field.select_all();
        self.replacement_focus = false;
    }
    pub fn show_replace(&mut self) {
        self.show();
        self.replacing = true;
    }
    pub fn height(&self) -> f32 {
        if self.replacing {
            HEIGHT + 84.0
        } else {
            HEIGHT
        }
    }
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            SearchMode::Literal => SearchMode::Extended,
            SearchMode::Extended => SearchMode::Regex,
            SearchMode::Regex => SearchMode::Literal,
        };
    }
    pub fn active_field(&mut self) -> &mut TextField {
        if self.replacement_focus {
            &mut self.replacement
        } else {
            &mut self.field
        }
    }
    pub fn enter_action(&self) -> FindAction {
        self.keyboard_focus.unwrap_or(if self.replacement_focus {
            FindAction::ReplaceOne
        } else {
            FindAction::Next
        })
    }
    pub fn cancel_search_tracked(&mut self) -> Option<bareline_search::SearchJob> {
        let job = self.paged_pending.as_ref().map(|ticket| ticket.job.clone())
            .or_else(|| self.pending.as_ref().map(|ticket| ticket.job.clone()));
        self.cancel_search();
        job
    }
    pub fn cancel_search(&mut self) {
        self.cancelled_query = Some(self.query());
        self.paged_pending = None;
        self.paged_requested = None;
        self.pending = None;
        self.results = None;
        self.status = "Cancelled".into();
    }
    pub fn start_replace_paged(
        &self,
        handle: bareline_editor_surface::paged_view::PagedReadHandle,
        selection: std::ops::Range<TextOffset>,
        all: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<bareline_search::service::PagedReplaceTicket, &'static str> {
        if self.field.composing() || self.replacement.composing() {
            return Err("Finish composing before replacing");
        }
        let results = self
            .paged_results
            .as_ref()
            .ok_or("Search the document first")?;
        if results.query != self.query() || results.completeness != Completeness::Complete {
            return Err("Complete the current search before replacing");
        }
        let snapshot = handle.snapshot().clone();
        if !results.source.same_document(&snapshot)
            || results.source.content_state != snapshot.content_state
        {
            return Err("Search results are stale");
        }
        Ok(self
            .worker
            .as_ref()
            .ok_or("Search worker unavailable")?
            .replace_paged(
                results.clone(),
                snapshot,
                self.replacement.value().into(),
                if all {
                    ReplaceScope::All
                } else {
                    ReplaceScope::One(selection)
                },
                move |ticket| handle.resolve_page(ticket),
                notify,
            ))
    }
    pub fn start_replace(
        &mut self,
        snapshot: &DocumentSnapshot,
        selection: std::ops::Range<TextOffset>,
        all: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<ReplaceTicket, &'static str> {
        if self.field.composing() || self.replacement.composing() || !self.query_current() {
            return Err("Wait for the current search to finish.");
        }
        let results = self
            .results
            .as_ref()
            .ok_or("Run a search before replacing.")?;
        if !results.is_current(snapshot) || results.completeness() != Completeness::Complete {
            return Err("Search is stale or incomplete; run it again.");
        }
        let scope = if all {
            ReplaceScope::All
        } else {
            ReplaceScope::One(selection)
        };
        let worker = self
            .worker
            .as_ref()
            .ok_or("Search worker is unavailable.")?;
        let replacement = bareline_search::decode_replacement(self.replacement.value(), self.mode)
            .map_err(|_| "Replacement contains an invalid escape.")?;
        self.status = "Preparing replacement…".into();
        Ok(worker.replace(
            results.clone(),
            snapshot.clone(),
            replacement,
            scope,
            notify,
        ))
    }
    fn query_current(&self) -> bool {
        self.requested
            .as_ref()
            .is_some_and(|(_, pattern, case, word, mode)| {
                pattern == self.field.value()
                    && *case == self.case_sensitive
                    && *word == self.whole_word
                    && *mode == self.mode
            })
    }
    fn action_enabled(&self, action: FindAction) -> bool {
        if matches!(action, FindAction::ReplaceOne | FindAction::ReplaceAll) {
            return !self.field.composing()
                && !self.replacement.composing()
                && self.query_current()
                && self.results.as_ref().is_some_and(|results| {
                    !results.is_empty() && results.completeness() == Completeness::Complete
                });
        }
        true
    }
    pub fn hide(&mut self) {
        if self.paged_pending.take().is_some() {
            self.paged_requested = None;
        }
        self.open = false;
        self.focused = false;
        self.keyboard_focus = None;
        self.field.cancel();
        self.replacement.cancel();
        // Retain completed results for F3 after closing the bar. A cancelled
        // in-flight query must be eligible for resubmission when reopened.
        if self.pending.take().is_some() {
            self.requested = None;
        }
    }
    pub fn clear_source(&mut self) {
        self.paged_pending = None;
        self.paged_results = None;
        self.paged_requested = None;
        self.selection_scope = None;
        self.pending = None;
        self.results = None;
        self.requested = None;
        self.status = "Type to find".into();
    }
    pub fn has_focus(&self) -> bool {
        self.open && (self.focused || self.keyboard_focus.is_some())
    }
    pub fn blur(&mut self) {
        self.focused = false;
        self.keyboard_focus = None;
        self.field.cancel();
        self.replacement.cancel();
    }
    pub fn focus_next(&mut self, backwards: bool) {
        if self.replacing && self.focused && !backwards && !self.replacement_focus {
            self.field.cancel();
            self.replacement_focus = true;
            return;
        }
        if self.replacing && self.focused && backwards && self.replacement_focus {
            self.replacement.cancel();
            self.replacement_focus = false;
            return;
        }
        let mut order = vec![
            None,
            Some(FindAction::Case),
            Some(FindAction::WholeWord),
            Some(FindAction::Mode),
            Some(FindAction::Previous),
            Some(FindAction::Next),
            Some(FindAction::Replace),
            Some(FindAction::Close),
        ];
        if self.replacing {
            order.extend([
                Some(FindAction::ReplaceOne),
                Some(FindAction::ReplaceAll),
                Some(FindAction::Cancel),
            ]);
        }
        let current = order
            .iter()
            .position(|value| *value == self.keyboard_focus)
            .unwrap_or(0);
        let next = if backwards {
            (current + order.len() - 1) % order.len()
        } else {
            (current + 1) % order.len()
        };
        self.field.cancel();
        self.replacement.cancel();
        self.keyboard_focus = order[next];
        self.focused = next == 0;
        self.replacement_focus = self.replacing && backwards && next == 0;
    }
    pub fn focused_action(&self) -> Option<FindAction> {
        self.keyboard_focus
    }
    pub fn refresh(&mut self, snapshot: &DocumentSnapshot, notify: Arc<dyn Fn() + Send + Sync>) {
        if !self.open || self.field.composing() || self.cancelled_query.as_ref() == Some(&self.query()) {
            return;
        }
        if self
            .requested
            .as_ref()
            .is_some_and(|(source, pattern, case, word, mode)| {
                source.same_document(snapshot)
                    && source.revision == snapshot.revision
                    && pattern == self.field.value()
                    && *case == self.case_sensitive
                    && *word == self.whole_word
                    && *mode == self.mode
            })
        {
            return;
        }
        self.pending = None;
        self.results = None;
        self.requested = Some((
            snapshot.clone(),
            self.field.value().into(),
            self.case_sensitive,
            self.whole_word,
            self.mode,
        ));
        if self.field.value().is_empty() {
            self.status = "Type to find".into();
            return;
        }
        if self.worker.is_none() {
            match SearchWorker::new() {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.status = format!("Search unavailable: {error}");
                    return;
                }
            }
        }
        let query = self.query();
        self.pending = Some(
            self.worker
                .as_ref()
                .unwrap()
                .submit(snapshot.clone(), query, notify),
        );
        self.status = "Searching…".into();
    }
    pub fn pump(&mut self) -> bool {
        if let Some(ticket) = &self.paged_pending {
            match ticket.try_recv() {
                Ok(Ok(results)) => {
                    self.status = if results.count_complete {
                        format!("{} matches", results.count)
                    } else {
                        format!("{} matches; {:?}", results.count, results.completeness)
                    };
                    self.paged_results = Some(Arc::new(results));
                    self.paged_pending = None;
                    return true;
                }
                Ok(Err(error)) => {
                    self.status = format!("Search stopped: {error:?}");
                    self.paged_pending = None;
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.status = "Search worker stopped".into();
                    self.paged_pending = None;
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(pending) = &self.pending else {
            return false;
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.pending = None;
                self.status = "Search stopped".into();
                return true;
            }
        };
        self.pending = None;
        match result {
            Ok(results) => {
                self.status = match results.completeness() {
                    Completeness::Complete => {
                        if results.is_empty() {
                            "No matches".into()
                        } else {
                            format!("{} matches", results.count())
                        }
                    }
                    Completeness::Cancelled => "Cancelled".into(),
                    Completeness::ResultLimit => format!("{}+ matches", results.count()),
                    Completeness::UnsupportedStreaming => {
                        "Incomplete: source exceeds regex limit".into()
                    }
                    Completeness::RegexLimit => "Incomplete: regex resource limit".into(),
                    Completeness::InvalidQuery => "Invalid query".into(),
                    _ => "Unsupported query".into(),
                };
                self.results = Some(Arc::new(results));
            }
            Err(_) => self.status = "Search superseded".into(),
        }
        true
    }
    pub fn next(
        &mut self,
        snapshot: &DocumentSnapshot,
        caret: usize,
        backwards: bool,
    ) -> Option<std::ops::Range<TextOffset>> {
        if self.field.composing() || !self.query_current() {
            return None;
        }
        let results = self.results.as_ref()?;
        if !results.is_current(snapshot) {
            self.status = "Results are stale".into();
            return None;
        }
        let found = results.next(TextOffset(caret), backwards, true)?;
        let index = results
            .matches()
            .partition_point(|m| m.range.start < found.range.start);
        self.status = format!(
            "{} of {}{}",
            index + 1,
            results.count(),
            if results.completeness() == Completeness::Complete {
                ""
            } else {
                "+"
            }
        );
        Some(found.range.clone())
    }
    pub fn field_bounds(width: f32) -> Rect {
        rect(16.0, TAB_HEIGHT + 12.0, width - 490.0, 28.0)
    }
    fn buttons(&self, width: f32) -> Vec<(FindAction, &'static str, Rect)> {
        let mut buttons = vec![
            (
                FindAction::Mode,
                match self.mode {
                    SearchMode::Literal => "Lit",
                    SearchMode::Extended => "Ext",
                    SearchMode::Regex => ".*",
                },
                rect(width - 362.0, TAB_HEIGHT + 12.0, 40.0, 28.0),
            ),
            (
                FindAction::Replace,
                "› Replace",
                rect(width - 150.0, TAB_HEIGHT + 12.0, 104.0, 28.0),
            ),
            (
                FindAction::WholeWord,
                "ab",
                rect(width - 410.0, TAB_HEIGHT + 12.0, 40.0, 28.0),
            ),
            (
                FindAction::Case,
                "Aa",
                rect(width - 458.0, TAB_HEIGHT + 12.0, 40.0, 28.0),
            ),
            (
                FindAction::Previous,
                "⌃",
                rect(width - 236.0, TAB_HEIGHT + 12.0, 32.0, 28.0),
            ),
            (
                FindAction::Next,
                "⌄",
                rect(width - 196.0, TAB_HEIGHT + 12.0, 32.0, 28.0),
            ),
            (
                FindAction::Close,
                "×",
                rect(width - 38.0, TAB_HEIGHT + 12.0, 28.0, 28.0),
            ),
        ];
        if self.replacing {
            buttons.extend([
                (
                    FindAction::ReplaceOne,
                    "Replace",
                    rect(width - 238.0, TAB_HEIGHT + 54.0, 104.0, 28.0),
                ),
                (
                    FindAction::ReplaceAll,
                    "Replace All",
                    rect(width - 126.0, TAB_HEIGHT + 54.0, 110.0, 28.0),
                ),
                (
                    FindAction::Mode,
                    match self.mode {
                        SearchMode::Literal => "Literal",
                        SearchMode::Extended => "Extended",
                        SearchMode::Regex => "Regex",
                    },
                    rect(16.0, TAB_HEIGHT + 94.0, 100.0, 28.0),
                ),
                (
                    FindAction::Cancel,
                    "Cancel",
                    rect(width - 126.0, TAB_HEIGHT + 94.0, 110.0, 28.0),
                ),
            ]);
        }
        buttons
    }
    pub fn pointer(
        &mut self,
        width: f32,
        point: Point,
        down: bool,
        backend: &impl TextBackend,
        extend: bool,
    ) -> Result<Option<FindAction>, LayoutError> {
        let hit = self
            .buttons(width)
            .iter()
            .find(|(action, _, bounds)| self.action_enabled(*action) && bounds.contains(point))
            .map(|(action, _, _)| *action);
        if down {
            self.pressed = hit;
            self.keyboard_focus = hit;
            self.replacement_focus =
                self.replacing && Self::replacement_bounds(width).contains(point);
            self.focused = self.replacement_focus || Self::field_bounds(width).contains(point);
            if self.focused {
                self.active_field().click(backend, point, extend)?;
            }
            Ok(None)
        } else {
            Ok(self.pressed.take().filter(|pressed| Some(*pressed) == hit))
        }
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        self.draw_with_theme(backend, width, bareline_ui::theme::UiTheme::default(), ops)
    }
    pub fn draw_with_theme(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        theme: bareline_ui::theme::UiTheme,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            self.field.release(backend);
            self.replacement.release(backend);
            return Ok(None);
        }
        ops.push(DrawOp::Fill(
            rect(0.0, TAB_HEIGHT, width, self.height()),
            theme.elevated,
        ));
        ops.push(DrawOp::Stroke(
            rect(0.0, TAB_HEIGHT, width, self.height()),
            theme.border,
            1.0,
        ));
        let mut caret = self.field.draw_with_theme(
            backend,
            Self::field_bounds(width),
            self.focused && !self.replacement_focus,
            theme,
            ops,
        )?;
        if self.replacing {
            let replace_caret = self.replacement.draw_with_theme(
                backend,
                Self::replacement_bounds(width),
                self.focused && self.replacement_focus,
                theme,
                ops,
            )?;
            if self.replacement_focus {
                caret = replace_caret;
            }
            text(
                ops,
                132.0,
                TAB_HEIGHT + 101.0,
                "Scope: Current document",
                13.0,
                theme.muted,
            );
        }
        for (i, (action, label, bounds)) in self.buttons(width).into_iter().enumerate() {
            if action == FindAction::Close {
                let center = Point {
                    x: bounds.x + bounds.width / 2.0,
                    y: bounds.y + bounds.height / 2.0,
                };
                for sign in [-1.0, 1.0] {
                    ops.push(DrawOp::Line {
                        from: Point {
                            x: center.x - 4.0,
                            y: center.y - sign * 4.0,
                        },
                        to: Point {
                            x: center.x + 4.0,
                            y: center.y + sign * 4.0,
                        },
                        color: theme.text,
                        width: 1.0,
                    });
                }
                if self.keyboard_focus == Some(action) {
                    ops.push(DrawOp::StrokeRounded(bounds, theme.focus, 4.0, 2.0));
                }
                continue;
            }
            Button {
                id: ViewId(500 + i as u64),
                label: if matches!(action, FindAction::Previous | FindAction::Next) {
                    String::new()
                } else {
                    label.into()
                },
                bounds,
                toggle: matches!(
                    action,
                    FindAction::Case | FindAction::WholeWord | FindAction::Mode
                ),
                state: ControlState {
                    disabled: !self.action_enabled(action),
                    checked: (action == FindAction::Case && self.case_sensitive)
                        || (action == FindAction::Mode && self.mode == SearchMode::Regex)
                        || (action == FindAction::WholeWord && self.whole_word),
                    pressed: self.pressed == Some(action),
                    focused: self.keyboard_focus == Some(action),
                    ..Default::default()
                },
            }
            .paint_with_theme(theme, ops);
            if matches!(action, FindAction::Previous | FindAction::Next) {
                let x = bounds.x + bounds.width / 2.0;
                let y = bounds.y + bounds.height / 2.0;
                let sign = if action == FindAction::Previous {
                    -1.0
                } else {
                    1.0
                };
                for side in [-1.0, 1.0] {
                    ops.push(DrawOp::Line {
                        from: Point {
                            x: x + side * 5.0,
                            y: y - sign * 3.0,
                        },
                        to: Point {
                            x,
                            y: y + sign * 3.0,
                        },
                        color: theme.text,
                        width: 1.0,
                    });
                }
            }
        }
        ops.push(DrawOp::PushClip(rect(
            width - 314.0,
            TAB_HEIGHT,
            74.0,
            HEIGHT,
        )));
        text(
            ops,
            width - 314.0,
            TAB_HEIGHT + 19.0,
            &self.status,
            13.0,
            theme.muted,
        );
        ops.push(DrawOp::PopClip);
        Ok(self.focused.then_some(caret))
    }
    fn replacement_bounds(width: f32) -> Rect {
        rect(16.0, TAB_HEIGHT + 54.0, width - 270.0, 28.0)
    }
}

impl FindController {
    /// IDs are shared with UIA actions; names are full command labels, not glyphs.
    pub fn semantics(&self, width: f32) -> Vec<bareline_ui::widgets::Semantics> {
        use bareline_ui::widgets::{SemanticAction, SemanticRole, Semantics};
        if !self.open {
            return Vec::new();
        }
        let mut field = Semantics::new(
            ViewId(6000),
            SemanticRole::TextField,
            "Find",
            "search.find",
            Self::field_bounds(width),
            ControlState {
                focused: self.focused && !self.replacement_focus,
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::SetValue);
        field.value = Some(self.field.semantic_value());
        let mut nodes = vec![field];
        if self.replacing {
            let mut field = Semantics::new(
                ViewId(6001),
                SemanticRole::TextField,
                "Replace with",
                "search.replace",
                Self::replacement_bounds(width),
                ControlState {
                    focused: self.focused && self.replacement_focus,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::SetValue);
            field.value = Some(self.replacement.semantic_value());
            nodes.push(field);
        }
        for (index, (action, _, bounds)) in self.buttons(width).into_iter().enumerate() {
            let (name, command) = match action {
                FindAction::Case => ("Match Case", "search.match_case"),
                FindAction::WholeWord => ("Match Whole Word", "search.whole_word"),
                FindAction::Previous => ("Find Previous", "search.find_previous"),
                FindAction::Next => ("Find Next", "search.find_next"),
                FindAction::Close => ("Close Find", "search.close_find"),
                FindAction::Replace => ("Replace", "search.replace"),
                FindAction::ReplaceOne => ("Replace One", "search.replace_one"),
                FindAction::ReplaceAll => ("Replace All", "search.replace_all"),
                FindAction::Mode => ("Search Mode", "search.mode"),
                FindAction::Cancel => ("Cancel Search", "search.cancel"),
            };
            let toggle = matches!(action, FindAction::Case | FindAction::WholeWord);
            let mut node = Semantics::new(
                ViewId(if index < 7 { 6100 } else { 6200 } + action as u64),
                if toggle {
                    SemanticRole::Checkbox
                } else {
                    SemanticRole::Button
                },
                name,
                command,
                bounds,
                ControlState {
                    disabled: !self.action_enabled(action),
                    focused: self.keyboard_focus == Some(action),
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Invoke);
            node.selected = match action {
                FindAction::Case => self.case_sensitive,
                FindAction::WholeWord => self.whole_word,
                _ => false,
            };
            if action == FindAction::Mode {
                node.value = Some(format!("{:?}", self.mode));
            }
            nodes.push(node);
        }
        nodes
    }
    pub fn accessibility_action(&mut self, id: u64, focus: bool) -> Option<FindAction> {
        if !self.open {
            return None;
        }
        if id == 6000 || (id == 6001 && self.replacing) {
            self.focused = true;
            self.keyboard_focus = None;
            self.replacement_focus = id == 6001;
            return None;
        }
        let action = self
            .buttons(1200.0)
            .into_iter()
            .map(|(action, _, _)| action)
            .find(|action| {
                (6100 + *action as u64 == id || 6200 + *action as u64 == id)
                    && self.action_enabled(*action)
            })?;
        if focus {
            self.focused = false;
            self.keyboard_focus = Some(action);
            None
        } else {
            Some(action)
        }
    }
}
