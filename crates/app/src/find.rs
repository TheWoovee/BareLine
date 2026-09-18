// SPDX-License-Identifier: MPL-2.0
use bareline_document::{ContentStateId, DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_search::{
    Case, Completeness, ReplaceScope, SearchMode, SearchQuery, SearchResults,
    service::{ReplaceTicket, SearchTicket, SearchWorker},
};
use bareline_ui::{
    controls::{Button, ControlState},
    overlays::Tooltip,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindSourceKey {
    Resident {
        identity: (u64, u64),
        content: ContentStateId,
    },
    Paged {
        identity: (u64, u64),
        content: ContentStateId,
    },
}
impl FindSourceKey {
    fn resident(snapshot: &DocumentSnapshot) -> Self {
        Self::Resident {
            identity: snapshot.identity_token(),
            content: snapshot.content_state,
        }
    }
    fn paged(snapshot: &bareline_document::paged::PagedSnapshot) -> Self {
        Self::Paged {
            identity: snapshot.identity_token(),
            content: snapshot.content_state,
        }
    }
    fn same_document(self, other: Self) -> bool {
        matches!((self, other),
            (Self::Resident { identity: a, .. }, Self::Resident { identity: b, .. }) |
            (Self::Paged { identity: a, .. }, Self::Paged { identity: b, .. }) if a.0 == b.0)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct FindSessionKey {
    source: FindSourceKey,
    query: SearchQuery,
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
    cancelled_query: Option<FindSessionKey>,
    worker: Option<SearchWorker>,
    pending: Option<(u64, FindSessionKey, SearchTicket)>,
    paged_pending: Option<(u64, FindSessionKey, bareline_search::service::PagedSearchTicket)>,
    paged_results: Option<(FindSessionKey, Arc<bareline_search::paged::PagedResults>)>,
    results: Option<(FindSessionKey, Arc<SearchResults>)>,
    requested: Option<FindSessionKey>,
    pressed: Option<FindAction>,
    keyboard_focus: Option<FindAction>,
    tooltip: Tooltip,
    tooltip_target: Option<FindAction>,
    tooltip_clock_ms: u64,
    tooltip_wake_sent: bool,
    active_source: Option<FindSourceKey>,
    request_generation: u64,
    accessibility_revision: std::cell::Cell<u64>,
    accessibility_fingerprint: std::cell::Cell<u64>,
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
            results: None,
            requested: None,
            pressed: None,
            keyboard_focus: None,
            tooltip: Tooltip::default(),
            tooltip_target: None,
            tooltip_clock_ms: 0,
            tooltip_wake_sent: false,
            active_source: None,
            request_generation: 0,
            accessibility_revision: std::cell::Cell::new(0),
            accessibility_fingerprint: std::cell::Cell::new(0),
        }
    }
}
impl FindController {
    fn next_request_generation(&mut self) -> u64 {
        self.request_generation = self
            .request_generation
            .checked_add(1)
            .expect("find request generation exhausted");
        self.request_generation
    }
    fn retire_pending(&mut self) {
        if let Some((_, _, ticket)) = self.pending.take() {
            ticket.job.cancel();
        }
        if let Some((_, _, ticket)) = self.paged_pending.take() {
            ticket.job.cancel();
        }
    }

    fn bind_source(&mut self, source: FindSourceKey) -> bool {
        if self.active_source == Some(source) {
            return false;
        }
        let revision_drift = self.active_source.is_some_and(|active| active.same_document(source));
        let document_changed = self.active_source.is_some_and(|active| !active.same_document(source));
        if document_changed {
            // A selection scope belongs to the document that supplied its byte
            // offsets. A cancelled query likewise must not suppress a fresh
            // activation after visiting another document.
            self.selection_scope = None;
            self.cancelled_query = None;
        }
        self.next_request_generation();
        self.active_source = Some(source);
        self.retire_pending();
        self.results = None;
        self.paged_results = None;
        self.requested = None;
        self.status = if self.field.value().is_empty() {
            "Type to find".into()
        } else if revision_drift {
            "Results changed; search again".into()
        } else {
            "Searching…".into()
        };
        true
    }

    fn set_terminal_status_if_current(&mut self, generation: u64, key: &FindSessionKey, status: &str) -> bool {
        if generation != self.request_generation || !self.session_current(key) {
            return false;
        }
        self.status = status.into();
        true
    }

    pub fn bind_resident(&mut self, snapshot: &DocumentSnapshot) -> bool {
        self.bind_source(FindSourceKey::resident(snapshot))
    }

    pub fn bind_paged(&mut self, snapshot: &bareline_document::paged::PagedSnapshot) -> bool {
        self.bind_source(FindSourceKey::paged(snapshot))
    }

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
            .as_ref()
            .filter(|(key, results)| self.session_current(key) && results.completeness == Completeness::Complete)
            .map(|(_, results)| results.as_ref())
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
            .as_ref()
            .filter(|(key, results)| self.session_current(key) && results.completeness() == Completeness::Complete)
            .map(|(_, results)| results.as_ref())
    }
    pub fn searching(&self) -> bool {
        self.pending.is_some() || self.paged_pending.is_some()
    }
    pub fn refresh_paged(
        &mut self,
        handle: bareline_editor_surface::paged_view::PagedReadHandle,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        if !self.open || self.field.composing() {
            return;
        }
        let query = self.query();
        let snapshot = handle.snapshot().clone();
        self.bind_paged(&snapshot);
        let key = FindSessionKey {
            source: FindSourceKey::paged(&snapshot),
            query: query.clone(),
        };
        if self.cancelled_query.as_ref() == Some(&key) {
            return;
        }
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        let generation = self.next_request_generation();
        self.retire_pending();
        self.paged_results = None;
        self.results = None;
        self.requested = Some(key.clone());
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
        self.paged_pending = Some((
            generation,
            key,
            self.worker.as_ref().unwrap().submit_paged(
                snapshot,
                query,
                move |ticket| handle.resolve_page(ticket).map_err(|error| error.to_string()),
                notify,
            ),
        ));
        self.status = "Searching full document…".into();
    }
    pub fn next_paged(
        &self,
        snapshot: &bareline_document::paged::PagedSnapshot,
        at: usize,
        backwards: bool,
    ) -> Option<std::ops::Range<TextOffset>> {
        if self.field.composing() {
            return None;
        }
        let (key, results) = self.paged_results.as_ref()?;
        if !self.session_current(key) {
            return None;
        }
        if !results.source.same_document(snapshot)
            || results.source.revision != snapshot.revision
            || results.source.content_state != snapshot.content_state
        {
            return None;
        }
        let index = results.matches.partition_point(|matched| matched.range.start.0 < at);
        let matched = if backwards {
            index
                .checked_sub(1)
                .and_then(|i| results.matches.get(i))
                .or_else(|| results.matches.last())
        } else {
            results.matches.get(index).or_else(|| results.matches.first())
        };
        matched.map(|matched| matched.range.clone())
    }

    pub fn show(&mut self) {
        self.dismiss_tooltip();
        self.accessibility_revision.set(
            self.accessibility_revision
                .get()
                .checked_add(1)
                .expect("find accessibility revision exhausted"),
        );
        self.cancelled_query = None;
        if self.results.is_none() {
            self.requested = None;
        }
        self.open = true;
        self.replacing = false;
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
        if self.replacing { HEIGHT + 84.0 } else { HEIGHT }
    }
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            SearchMode::Literal => SearchMode::Extended,
            SearchMode::Extended => SearchMode::Regex,
            SearchMode::Regex => SearchMode::Literal,
        };
    }
    /// Regex is an independent icon toggle: turning it on selects Regex matching,
    /// turning it off returns to plain (Literal) matching.
    pub fn toggle_regex(&mut self) {
        self.mode = if self.mode == SearchMode::Regex {
            SearchMode::Literal
        } else {
            SearchMode::Regex
        };
    }
    /// Extended (escape-sequence) matching as its own toggle, mirroring regex.
    pub fn toggle_extended(&mut self) {
        self.mode = if self.mode == SearchMode::Extended {
            SearchMode::Literal
        } else {
            SearchMode::Extended
        };
    }
    /// F3 finds the next match, Shift+F3 the previous one, from the field.
    pub fn find_direction(&self, backwards: bool) -> FindAction {
        if backwards {
            FindAction::Previous
        } else {
            FindAction::Next
        }
    }
    pub fn active_field(&mut self) -> &mut TextField {
        if self.replacement_focus {
            &mut self.replacement
        } else {
            &mut self.field
        }
    }
    pub fn accessibility_text_field(&self) -> Option<(u64, u64, &TextField)> {
        if !self.has_focus() || !self.focused {
            return None;
        }
        let (owner, field) = if self.replacing && self.replacement_focus {
            (6001, &self.replacement)
        } else {
            (6000, &self.field)
        };
        use std::hash::{Hash, Hasher};
        let mut state = std::collections::hash_map::DefaultHasher::new();
        owner.hash(&mut state);
        field.value().hash(&mut state);
        field.selection().hash(&mut state);
        field.composition_text().hash(&mut state);
        let fingerprint = state.finish();
        if fingerprint != self.accessibility_fingerprint.get() {
            self.accessibility_fingerprint.set(fingerprint);
            self.accessibility_revision.set(
                self.accessibility_revision
                    .get()
                    .checked_add(1)
                    .expect("find accessibility revision exhausted"),
            );
        }
        Some((owner, self.accessibility_revision.get(), field))
    }
    pub fn enter_action(&self) -> FindAction {
        self.keyboard_focus.unwrap_or(if self.replacement_focus {
            FindAction::ReplaceOne
        } else {
            FindAction::Next
        })
    }
    pub fn cancel_search_tracked(&mut self) -> Option<bareline_search::SearchJob> {
        let job = self
            .paged_pending
            .as_ref()
            .map(|(_, _, ticket)| ticket.job.clone())
            .or_else(|| self.pending.as_ref().map(|(_, _, ticket)| ticket.job.clone()));
        self.cancel_search();
        job
    }
    pub fn cancel_search(&mut self) {
        self.next_request_generation();
        let query = self.query();
        self.cancelled_query = self.active_source.map(|source| FindSessionKey { source, query });
        self.retire_pending();
        self.results = None;
        self.paged_results = None;
        self.requested = None;
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
        let (key, results) = self.paged_results.as_ref().ok_or("Search the document first")?;
        if !self.session_current(key) || results.query != self.query() || results.completeness != Completeness::Complete
        {
            return Err("Complete the current search before replacing");
        }
        let snapshot = handle.snapshot().clone();
        if !results.source.same_document(&snapshot)
            || results.source.revision != snapshot.revision
            || results.source.content_state != snapshot.content_state
        {
            return Err("Search results are stale");
        }
        Ok(self.worker.as_ref().ok_or("Search worker unavailable")?.replace_paged(
            results.clone(),
            snapshot,
            self.replacement.value().into(),
            if all {
                ReplaceScope::All
            } else {
                ReplaceScope::One(selection)
            },
            move |ticket| handle.resolve_page(ticket).map_err(|error| error.to_string()),
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
        let (key, results) = self.results.as_ref().ok_or("Run a search before replacing.")?;
        if !self.session_current(key) {
            return Err("Search results are stale; run it again.");
        }
        if !results.is_current(snapshot) || results.completeness() != Completeness::Complete {
            return Err("Search is stale or incomplete; run it again.");
        }
        let scope = if all {
            ReplaceScope::All
        } else {
            ReplaceScope::One(selection)
        };
        let worker = self.worker.as_ref().ok_or("Search worker is unavailable.")?;
        let replacement = bareline_search::decode_replacement(self.replacement.value(), self.mode)
            .map_err(|_| "Replacement contains an invalid escape.")?;
        self.status = "Preparing replacement…".into();
        Ok(worker.replace(results.clone(), snapshot.clone(), replacement, scope, notify))
    }
    fn query_current(&self) -> bool {
        self.requested.as_ref().is_some_and(|key| self.session_current(key))
    }
    fn session_current(&self, key: &FindSessionKey) -> bool {
        self.active_source == Some(key.source) && key.query == self.query()
    }
    fn action_enabled(&self, action: FindAction) -> bool {
        if matches!(action, FindAction::ReplaceOne | FindAction::ReplaceAll) {
            let resident = self.results.as_ref().is_some_and(|(key, results)| {
                self.session_current(key) && !results.is_empty() && results.completeness() == Completeness::Complete
            });
            let paged = self.paged_results.as_ref().is_some_and(|(key, results)| {
                self.session_current(key)
                    && !results.matches.is_empty()
                    && results.completeness == Completeness::Complete
            });
            return !self.field.composing()
                && !self.replacement.composing()
                && self.query_current()
                && (resident || paged);
        }
        true
    }
    pub fn hide(&mut self) {
        let had_pending = self.pending.is_some() || self.paged_pending.is_some();
        self.retire_pending();
        self.open = false;
        self.focused = false;
        self.keyboard_focus = None;
        self.dismiss_tooltip();
        self.field.cancel();
        self.replacement.cancel();
        // Retain completed results for F3 after closing the bar. A cancelled
        // in-flight query must be eligible for resubmission when reopened.
        if had_pending {
            self.requested = None;
        }
    }
    pub fn clear_source(&mut self) {
        self.cancelled_query = None;
        if self.active_source.is_none()
            && self.pending.is_none()
            && self.paged_pending.is_none()
            && self.results.is_none()
            && self.paged_results.is_none()
            && self.requested.is_none()
        {
            self.status = "Type to find".into();
            return;
        }
        self.retire_pending();
        self.paged_results = None;
        self.selection_scope = None;
        self.results = None;
        self.requested = None;
        self.active_source = None;
        self.next_request_generation();
        self.status = "Type to find".into();
    }
    pub fn has_focus(&self) -> bool {
        self.open && (self.focused || self.keyboard_focus.is_some())
    }
    pub fn blur(&mut self) {
        self.focused = false;
        self.keyboard_focus = None;
        self.dismiss_tooltip();
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
        if !self.open || self.field.composing() {
            return;
        }
        self.bind_resident(snapshot);
        let query = self.query();
        let key = FindSessionKey {
            source: FindSourceKey::resident(snapshot),
            query: query.clone(),
        };
        if self.cancelled_query.as_ref() == Some(&key) {
            return;
        }
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        let generation = self.next_request_generation();
        self.retire_pending();
        self.results = None;
        self.paged_results = None;
        self.requested = Some(key.clone());
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
        self.pending = Some((
            generation,
            key,
            self.worker.as_ref().unwrap().submit(snapshot.clone(), query, notify),
        ));
        self.status = "Searching…".into();
    }
    pub fn pump(&mut self) -> bool {
        if let Some((generation, key, ticket)) = &self.paged_pending {
            let generation = *generation;
            let key = key.clone();
            let received = ticket.try_recv();
            match received {
                Ok(Ok(results)) => {
                    self.paged_pending = None;
                    if generation != self.request_generation || !self.session_current(&key) {
                        return false;
                    }
                    self.status = if results.count_complete {
                        format!("{} matches", results.count)
                    } else {
                        format!("{} matches; {:?}", results.count, results.completeness)
                    };
                    self.paged_results = Some((key, Arc::new(results)));
                    return true;
                }
                Ok(Err(error)) => {
                    self.paged_pending = None;
                    if generation != self.request_generation || !self.session_current(&key) {
                        return false;
                    }
                    self.status = format!("Search stopped: {error:?}");
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.paged_pending = None;
                    if generation != self.request_generation || !self.session_current(&key) {
                        return false;
                    }
                    self.status = "Search worker stopped".into();
                    return true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some((generation, key, pending)) = &self.pending else {
            return false;
        };
        let generation = *generation;
        let key = key.clone();
        let received = pending.try_recv();
        let result = match received {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.pending = None;
                return self.set_terminal_status_if_current(generation, &key, "Search stopped");
            }
        };
        self.pending = None;
        if generation != self.request_generation || !self.session_current(&key) {
            return false;
        }
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
                    Completeness::UnsupportedStreaming => "Incomplete: source exceeds regex limit".into(),
                    Completeness::RegexLimit => "Incomplete: regex resource limit".into(),
                    Completeness::InvalidQuery => "Invalid query".into(),
                    _ => "Unsupported query".into(),
                };
                self.results = Some((key, Arc::new(results)));
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
        let (key, results) = self.results.as_ref()?;
        if !self.session_current(key) {
            return None;
        }
        if !results.is_current(snapshot) {
            self.status = "Results are stale".into();
            return None;
        }
        let found = results.next(TextOffset(caret), backwards, true)?;
        let index = results.matches().partition_point(|m| m.range.start < found.range.start);
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
        // The find field never grows past 640 px so it stays readable and the
        // option buttons keep their fixed positions on wide windows.
        rect(16.0, TAB_HEIGHT + 12.0, (width - 490.0).min(640.0), 28.0)
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
    fn toggle_contract(&self, action: FindAction) -> Option<(&'static str, bool)> {
        match action {
            FindAction::Case => Some(("Match case", self.case_sensitive)),
            FindAction::WholeWord => Some(("Whole word", self.whole_word)),
            FindAction::Mode if self.mode == SearchMode::Literal => Some(("Literal search", false)),
            FindAction::Mode if self.mode == SearchMode::Regex => Some(("Regular expression", true)),
            FindAction::Mode => Some((
                "Extended escape sequences: \\\\, \\0, \\n, \\r, \\t, \\xNN, \\uNNNN, \\UNNNNNNNN",
                self.mode == SearchMode::Extended,
            )),
            _ => None,
        }
    }
    pub fn dismiss_tooltip(&mut self) -> bool {
        let changed = self.tooltip_target.is_some();
        self.tooltip.dismiss();
        self.tooltip_target = None;
        self.tooltip_wake_sent = false;
        changed
    }
    pub fn hover_toggles(&mut self, width: f32, point: Point, now_ms: u64) -> bool {
        self.tooltip_clock_ms = now_ms;
        let target = self
            .open
            .then(|| {
                self.buttons(width)
                    .into_iter()
                    .take(7)
                    .find(|(action, _, bounds)| self.toggle_contract(*action).is_some() && bounds.contains(point))
                    .map(|(action, _, _)| action)
            })
            .flatten();
        let changed = target != self.tooltip_target;
        if changed {
            self.tooltip.dismiss();
            self.tooltip_target = target;
            self.tooltip_wake_sent = false;
        }
        self.tooltip.hover(target.is_some(), now_ms);
        changed
    }
    pub fn tooltip_deadline_ms(&self) -> Option<u64> {
        (!self.tooltip_wake_sent && self.tooltip_target.is_some())
            .then(|| self.tooltip.deadline())
            .flatten()
    }
    pub fn tooltip_tick(&mut self, now_ms: u64) -> bool {
        self.tooltip_clock_ms = now_ms;
        if self.tooltip_wake_sent || self.tooltip_target.is_none() || !self.tooltip.visible(now_ms) {
            return false;
        }
        self.tooltip_wake_sent = true;
        true
    }
    fn wrap_tooltip(backend: &mut impl TextBackend, label: &str, max_width: f32) -> Result<String, LayoutError> {
        let mut lines = Vec::<String>::new();
        for word in label.split_whitespace() {
            let candidate = lines.last().map_or_else(
                || word.to_owned(),
                |line| {
                    if line.is_empty() {
                        word.to_owned()
                    } else {
                        format!("{line} {word}")
                    }
                },
            );
            let needs_new = !lines.is_empty() && backend.measure_text(&candidate, 13.0)?.0 > max_width;
            if lines.is_empty() || needs_new {
                lines.push(word.to_owned());
                continue;
            }
            let line = lines.last_mut().unwrap();
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        Ok(lines.join("\n"))
    }
    fn tooltip_layout(
        &self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        action: FindAction,
        label: &str,
    ) -> Result<Option<(Rect, String)>, LayoutError> {
        let available = (width - 16.0).max(0.0);
        if available <= 16.0 {
            return Ok(None);
        }
        let mut wrapped = Self::wrap_tooltip(backend, label, available - 16.0)?;
        let mut line_width = 0.0f32;
        for line in wrapped.lines() {
            line_width = line_width.max(backend.measure_text(line, 13.0)?.0);
        }
        let mut tooltip_width = (line_width + 16.0).clamp(0.0, available);
        let mut tooltip_height = wrapped.lines().count().max(1) as f32 * 18.0 + 12.0;
        let anchor = self
            .buttons(width)
            .into_iter()
            .take(7)
            .find(|(candidate, _, _)| *candidate == action)
            .map(|(_, _, bounds)| bounds)
            .unwrap_or_else(|| rect(8.0, TAB_HEIGHT + 12.0, 0.0, 28.0));
        let preferred_y = TAB_HEIGHT + self.height() + 4.0;
        let viewport_bottom = height - STATUS_HEIGHT;
        let mut y = preferred_y;
        if y + tooltip_height > viewport_bottom {
            let summary = match action {
                FindAction::Mode if self.mode == SearchMode::Literal => "Literal search",
                FindAction::Mode if self.mode == SearchMode::Regex => "Regular expression",
                FindAction::Mode => "Extended escape sequences",
                _ => label,
            };
            wrapped = Self::wrap_tooltip(backend, summary, available - 16.0)?;
            line_width = 0.0;
            for line in wrapped.lines() {
                line_width = line_width.max(backend.measure_text(line, 13.0)?.0);
            }
            tooltip_width = (line_width + 16.0).clamp(0.0, available);
            tooltip_height = wrapped.lines().count().max(1) as f32 * 18.0 + 12.0;
            y = viewport_bottom - tooltip_height;
            let field = if self.replacing {
                Self::replacement_bounds(width)
            } else {
                Self::field_bounds(width)
            };
            if y < field.y + field.height {
                return Ok(None);
            }
        }
        let x = (anchor.x + anchor.width / 2.0 - tooltip_width / 2.0)
            .max(8.0)
            .min((width - tooltip_width - 8.0).max(8.0));
        Ok(Some((rect(x, y, tooltip_width, tooltip_height), wrapped)))
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
            self.dismiss_tooltip();
            self.pressed = hit;
            self.keyboard_focus = hit;
            self.replacement_focus = self.replacing && Self::replacement_bounds(width).contains(point);
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
        self.draw_with_theme_in(backend, width, f32::INFINITY, theme, ops)
    }
    pub fn draw_with_theme_in(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
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
        let field_bounds = Self::field_bounds(width);
        let mut caret = self.field.draw_with_theme(
            backend,
            field_bounds,
            self.focused && !self.replacement_focus,
            theme,
            ops,
        )?;
        // Placeholder prompt while the field is empty.
        if self.field.value().is_empty() {
            text(
                ops,
                field_bounds.x + 8.0,
                field_bounds.y + 7.0,
                "Find",
                13.0,
                theme.muted,
            );
        }
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
            if self.replacement.value().is_empty() {
                let bounds = Self::replacement_bounds(width);
                text(ops, bounds.x + 8.0, bounds.y + 7.0, "Replace with", 13.0, theme.muted);
            }
            // Scope shown as a dropdown-style control rather than plain prose.
            // Sits to the right of the mode button (which ends near x=116).
            text(ops, 132.0, TAB_HEIGHT + 101.0, "Scope", 12.0, theme.muted);
            let scope = rect(180.0, TAB_HEIGHT + 94.0, 176.0, 26.0);
            ops.push(DrawOp::StrokeRounded(scope, theme.border, 4.0, 1.0));
            let label = if self.selection_scope.is_some() {
                "Selection"
            } else {
                "Current document"
            };
            text(ops, scope.x + 8.0, scope.y + 6.0, label, 13.0, theme.text);
            let cx = scope.x + scope.width - 14.0;
            let cy = scope.y + scope.height / 2.0;
            for side in [-1.0, 1.0] {
                ops.push(DrawOp::Line {
                    from: Point {
                        x: cx + side * 4.0,
                        y: cy - 2.0,
                    },
                    to: Point { x: cx, y: cy + 3.0 },
                    color: theme.muted,
                    width: 1.0,
                });
            }
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
                toggle: matches!(action, FindAction::Case | FindAction::WholeWord | FindAction::Mode),
                state: ControlState {
                    disabled: !self.action_enabled(action),
                    checked: (action == FindAction::Case && self.case_sensitive)
                        || (action == FindAction::Mode && self.mode != SearchMode::Literal)
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
                let sign = if action == FindAction::Previous { -1.0 } else { 1.0 };
                for side in [-1.0, 1.0] {
                    ops.push(DrawOp::Line {
                        from: Point {
                            x: x + side * 5.0,
                            y: y - sign * 3.0,
                        },
                        to: Point { x, y: y + sign * 3.0 },
                        color: theme.text,
                        width: 1.0,
                    });
                }
            }
        }
        ops.push(DrawOp::PushClip(rect(width - 314.0, TAB_HEIGHT, 74.0, HEIGHT)));
        text(ops, width - 314.0, TAB_HEIGHT + 19.0, &self.status, 13.0, theme.muted);
        ops.push(DrawOp::PopClip);
        if let Some(action) = self.tooltip_target
            && let Some((label, _)) = self.toggle_contract(action)
        {
            if let Some((bounds, wrapped)) = self.tooltip_layout(backend, width, height, action, label)? {
                self.tooltip
                    .paint(self.tooltip_clock_ms, bounds, &wrapped, theme.widgets(), ops);
            }
        }
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
                FindAction::Case => ("Match case", "search.match_case"),
                FindAction::WholeWord => ("Whole word", "search.whole_word"),
                FindAction::Previous => ("Find Previous", "search.find_previous"),
                FindAction::Next => ("Find Next", "search.find_next"),
                FindAction::Close => ("Close Find", "search.close_find"),
                FindAction::Replace => ("Replace", "search.replace"),
                FindAction::ReplaceOne => ("Replace One", "search.replace_one"),
                FindAction::ReplaceAll => ("Replace All", "search.replace_all"),
                FindAction::Mode => (self.toggle_contract(action).unwrap().0, "search.mode"),
                FindAction::Cancel => ("Cancel Search", "search.cancel"),
            };
            // Mode cycles through three choices; it is not an on/off checkbox.
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
                (6100 + *action as u64 == id || 6200 + *action as u64 == id) && self.action_enabled(*action)
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

#[cfg(test)]
mod find_bar_tests {
    use super::*;
    fn snapshot(text: &str) -> DocumentSnapshot {
        bareline_document::Document::from_utf8(
            text,
            bareline_document::Budget::new(4096),
            bareline_document::Budget::new(4096),
        )
        .unwrap()
        .snapshot()
    }

    #[test]
    fn document_switch_clears_byte_scope_and_restarts_a_cancelled_query() {
        let a = snapshot("a needle");
        let b = snapshot("b needle");
        let mut find = FindController::default();
        find.show();
        assert!(find.field.insert("needle"));
        find.bind_resident(&a);
        find.set_selection_scope(Some(TextOffset(2)..TextOffset(8)));
        find.cancel_search();
        assert!(find.cancelled_query.is_some());

        find.bind_resident(&b);
        assert!(find.query().selection.is_none());
        assert!(find.cancelled_query.is_none());
        find.bind_resident(&a);
        find.refresh(&a, Arc::new(|| {}));
        assert!(find.searching());
        assert_eq!(find.status, "Searching…");
        find.cancel_search();

        find.clear_source();
        assert!(find.cancelled_query.is_none());
        find.bind_resident(&a);
        find.refresh(&a, Arc::new(|| {}));
        assert!(find.searching());
        assert_eq!(find.status, "Searching…");
        find.cancel_search();
    }

    #[test]
    fn stale_worker_disconnect_cannot_replace_the_current_document_status() {
        let a = snapshot("a needle");
        let b = snapshot("b needle");
        let mut find = FindController::default();
        find.show();
        assert!(find.field.insert("needle"));
        find.bind_resident(&a);
        let generation = find.request_generation;
        let stale = FindSessionKey {
            source: FindSourceKey::resident(&a),
            query: find.query(),
        };
        find.bind_resident(&b);
        let status = find.status.clone();

        assert!(!find.set_terminal_status_if_current(generation, &stale, "Search stopped"));
        assert_eq!(find.status, status);
    }

    #[test]
    fn option_toggles_map_to_query_flags() {
        let mut find = FindController::default();
        assert!(find.field.insert("needle"));
        let query = find.query();
        assert_eq!(query.case, Case::Folded);
        assert!(!query.whole_word);
        assert_eq!(query.mode, SearchMode::Literal);
        find.case_sensitive = true;
        find.whole_word = true;
        let query = find.query();
        assert_eq!(query.case, Case::Sensitive);
        assert!(query.whole_word);
        // Regex and extended are independent icon toggles that flip back off.
        find.toggle_regex();
        assert_eq!(find.query().mode, SearchMode::Regex);
        find.toggle_regex();
        assert_eq!(find.query().mode, SearchMode::Literal);
        find.toggle_extended();
        assert_eq!(find.query().mode, SearchMode::Extended);
        find.toggle_extended();
        assert_eq!(find.query().mode, SearchMode::Literal);
    }
    #[test]
    fn mode_controls_and_tooltips_describe_each_current_search_mode() {
        use bareline_ui::widgets::SemanticRole;

        let mut find = FindController::default();
        find.show_replace();
        for (mode, name, value) in [
            (SearchMode::Literal, "Literal search", "Literal"),
            (SearchMode::Extended, "Extended escape sequences:", "Extended"),
            (SearchMode::Regex, "Regular expression", "Regex"),
        ] {
            find.mode = mode;
            let label = find.toggle_contract(FindAction::Mode).unwrap().0;
            assert!(label.starts_with(name), "{mode:?}: {label}");
            let nodes = find.semantics(1000.0);
            for id in [6108, 6208] {
                let node = nodes.iter().find(|node| node.id.0 == id).unwrap();
                assert_eq!(node.name, label);
                assert_eq!(node.value.as_deref(), Some(value));
                assert_eq!(node.role, SemanticRole::Button);
                assert!(!node.selected);
            }
            let mut backend = bareline_renderer_recording::RecordingBackend::default();
            let (_, tooltip) = find
                .tooltip_layout(&mut backend, 480.0, 180.0, FindAction::Mode, label)
                .unwrap()
                .unwrap();
            assert!(tooltip.starts_with(name.trim_end_matches(':')));
        }
    }

    #[test]
    fn toggle_tooltips_share_names_and_pressed_state_with_semantics() {
        let mut find = FindController::default();
        find.show();
        find.case_sensitive = true;
        find.whole_word = true;
        find.mode = SearchMode::Extended;
        let semantics = find.semantics(1000.0);
        for (command, name) in [
            ("search.match_case", "Match case"),
            ("search.whole_word", "Whole word"),
            (
                "search.mode",
                "Extended escape sequences: \\\\, \\0, \\n, \\r, \\t, \\xNN, \\uNNNN, \\UNNNNNNNN",
            ),
        ] {
            let node = semantics.iter().find(|node| node.command_id == command).unwrap();
            assert_eq!(node.name, name);
            if command == "search.mode" {
                assert_eq!(node.role, bareline_ui::widgets::SemanticRole::Button);
                assert!(!node.selected);
            } else {
                assert_eq!(node.role, bareline_ui::widgets::SemanticRole::Checkbox);
                assert!(node.selected);
            }
        }
    }

    #[test]
    fn toggle_tooltip_deadline_wakes_once_and_dismisses_on_leave_or_close() {
        let mut find = FindController::default();
        find.show();
        let width = 1000.0;
        for (index, action) in [FindAction::Case, FindAction::WholeWord, FindAction::Mode]
            .into_iter()
            .enumerate()
        {
            let bounds = find
                .buttons(width)
                .into_iter()
                .find(|(candidate, _, _)| *candidate == action)
                .unwrap()
                .2;
            let entered = find.hover_toggles(
                width,
                Point {
                    x: bounds.x + 2.0,
                    y: bounds.y + 2.0,
                },
                index as u64 * 1_000,
            );
            assert!(entered);
            let deadline = index as u64 * 1_000 + 500;
            assert_eq!(find.tooltip_deadline_ms(), Some(deadline));
            assert!(!find.tooltip_tick(deadline - 1));
            assert!(find.tooltip_tick(deadline));
            assert!(!find.tooltip_tick(deadline + 1));
            assert_eq!(find.tooltip_deadline_ms(), None);
            assert!(find.tooltip.visible(deadline));

            assert!(find.hover_toggles(width, Point { x: 4.0, y: 4.0 }, deadline + 2));
            assert!(!find.tooltip.visible(deadline + 2));
        }
        let case = find
            .buttons(width)
            .into_iter()
            .find(|(action, _, _)| *action == FindAction::Case)
            .unwrap()
            .2;
        find.hover_toggles(
            width,
            Point {
                x: case.x + 2.0,
                y: case.y + 2.0,
            },
            4_000,
        );
        find.hide();
        assert_eq!(find.tooltip_deadline_ms(), None);
        assert!(find.tooltip_target.is_none());
    }

    #[test]
    fn tooltip_content_wraps_and_clamps_without_covering_the_query() {
        let mut find = FindController::default();
        find.show();
        find.mode = SearchMode::Extended;
        let label = find.toggle_contract(FindAction::Mode).unwrap().0;
        for (physical_width, scale) in [(480.0, 1.0), (720.0, 1.5), (960.0, 2.0), (240.0, 1.0)] {
            let logical_width = physical_width / scale;
            let mut backend = bareline_renderer_recording::RecordingBackend::default();
            let (bounds, wrapped) = find
                .tooltip_layout(&mut backend, logical_width, 300.0, FindAction::Mode, label)
                .unwrap()
                .unwrap();
            assert!(bounds.x >= 8.0);
            assert!(bounds.x + bounds.width <= logical_width - 8.0 + f32::EPSILON);
            assert!(bounds.y >= FindController::field_bounds(logical_width).y + 28.0);
            assert!(bounds.y + bounds.height <= 300.0 - STATUS_HEIGHT + f32::EPSILON);
            assert_eq!(wrapped.lines().collect::<Vec<_>>().join(" "), label);

            let mut operations = Vec::new();
            find.tooltip.hover(true, 0);
            find.tooltip.paint(
                500,
                bounds,
                &wrapped,
                bareline_ui::widgets::Theme::default(),
                &mut operations,
            );
            let lines: Vec<_> = operations
                .iter()
                .filter_map(|operation| match operation {
                    DrawOp::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(lines, wrapped.lines().collect::<Vec<_>>());
            assert!(
                lines
                    .iter()
                    .all(|line| backend.measure(line, 13.0).0 <= bounds.width - 16.0 + f32::EPSILON)
            );
            find.tooltip.dismiss();
        }
    }
    #[test]
    fn short_replace_view_uses_summary_or_hides_without_covering_a_field() {
        let mut find = FindController::default();
        find.show_replace();
        find.mode = SearchMode::Extended;
        let label = find.toggle_contract(FindAction::Mode).unwrap().0;
        let mut backend = bareline_renderer_recording::RecordingBackend::default();
        let (bounds, wrapped) = find
            .tooltip_layout(&mut backend, 480.0, 180.0, FindAction::Mode, label)
            .unwrap()
            .unwrap();
        let replacement = FindController::replacement_bounds(480.0);
        assert_eq!(wrapped, "Extended escape sequences");
        assert!(bounds.y >= replacement.y + replacement.height);
        assert!(bounds.y + bounds.height <= 180.0 - STATUS_HEIGHT);

        assert!(
            find.tooltip_layout(&mut backend, 480.0, 120.0, FindAction::Mode, label)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn enter_and_f3_cycle_direction() {
        let mut find = FindController::default();
        find.show();
        // Enter in the find field advances to the next match.
        assert_eq!(find.enter_action(), FindAction::Next);
        // F3 forward, Shift+F3 backward.
        assert_eq!(find.find_direction(false), FindAction::Next);
        assert_eq!(find.find_direction(true), FindAction::Previous);
        // Enter with the replacement field focused replaces the current match.
        find.show_replace();
        find.focus_next(false);
        assert_eq!(find.enter_action(), FindAction::ReplaceOne);
    }
}
