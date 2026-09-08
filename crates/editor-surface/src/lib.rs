// SPDX-License-Identifier: MPL-2.0
pub mod completion;
pub mod group_view;
pub mod search_marks;
pub mod paged_view;
pub mod paged_navigation;
pub mod power;
mod view_geometry;
mod grapheme_navigation;
mod virtual_layout;
use bareline_document::{
    DocumentSnapshot, EditTransaction, TextOffset,
    service::{Completion, DocumentService, Mutation, SubmitError},
};
use bareline_renderer::{
    DrawOp, LayoutError, LayoutId, MAX_LAYOUT_BYTES, MAX_LAYOUTS, Point, Rect, TextBackend,
};
use bareline_ui::{
    STATUS_HEIGHT, TAB_HEIGHT,
    controls::{Scrollbar, visible_rows},
    rect, text,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError},
    },
};
use unicode_segmentation::UnicodeSegmentation;

const LEFT: f32 = 64.0;
const MAX_QUEUED_INPUTS: usize = 256;
pub struct SyntaxView<'a> {
    pub result: Option<&'a bareline_syntax::SyntaxResult>,
    pub language: &'a str,
    pub unavailable: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub caret: usize,
}
impl Selection {
    fn range(self) -> std::ops::Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
}
#[derive(Clone, Debug)]
pub enum Input {
    Insert(String),
    Backspace,
    Delete,
    Undo,
    Redo,
    SelectAll,
    Left(bool),
    Right(bool),
    Up(bool),
    Down(bool),
    Home(bool),
    End(bool),
    SetCaret(usize, bool),
}
#[derive(Clone, Copy)]
enum HistoryMove {
    Edit,
    Undo,
    Redo,
}
struct Pending {
    folds_before: Vec<std::ops::Range<usize>>,
    folds_after: Vec<std::ops::Range<usize>>,
    input: Option<Input>,
    receiver: Receiver<Completion>,
    after: power::SelectionSet,
    before: power::SelectionSet,
    bookmarks_before: power::Bookmarks,
    marks_before: search_marks::SearchMarks,
    bookmarks_after: power::Bookmarks,
    marks_after: search_marks::SearchMarks,
    history: HistoryMove,
}
#[derive(Clone)]
struct SelectionHistory {
    folds_before: Vec<std::ops::Range<usize>>,
    folds_after: Vec<std::ops::Range<usize>>,
    before: power::SelectionSet,
    after: power::SelectionSet,
    bookmarks_before: power::Bookmarks,
    marks_before: search_marks::SearchMarks,
    bookmarks_after: power::Bookmarks,
    marks_after: search_marks::SearchMarks,
    group: Option<bareline_document::group::UndoGroup>,
}
struct LineLayout {
    id: LayoutId,
    start: usize,
    end: usize,
    x_origin: f64,
    row_origin: usize,
}
pub struct EditorSurface {
    blink: view_geometry::CaretBlink,
    wrap: bool,
    wrap_rows: BTreeMap<usize, usize>,
    preferred_x: Option<f32>,
    visual_navigation: VecDeque<Input>,
    grapheme_navigation: Option<grapheme_navigation::Navigation>,
    virtual_lines: BTreeMap<usize,virtual_layout::VirtualLine>,
    recovery: Option<bareline_file_io::resident_recovery::ResidentRecovery>,
    pub theme: bareline_ui::theme::EditorTheme,
    service: Option<DocumentService>,
    pub user_read_only: bool,
    snapshot: DocumentSnapshot,
    pub selection: Selection,
    pub scroll_y: f64,
    pub top_inset: f32,
    pub bottom_inset: f32,
    pub search_selection: bool,
    pub error: Option<String>,
    pending: Option<Pending>,
    queue: VecDeque<Input>,
    queue_origins: VecDeque<bareline_document::history::EditOrigin>,
    history_boundary: u64,
    acknowledged: VecDeque<Input>,
    ordered_receipts: VecDeque<power::consumer::OrderedReceipt>,
    acknowledged_commands: VecDeque<(String, BTreeMap<String, String>)>,
    pending_command: Option<(String, BTreeMap<String, String>)>,
    manual_hidden: Vec<std::ops::RangeInclusive<usize>>,
    scroll_x: f64,
    power_rectangle: Option<power::Rectangle>,
    column_maps: BTreeMap<usize,power::DisplayColumnMap>,
    column_maps_revision: Option<bareline_document::Revision>,
    column_map_bytes: usize,
    column_measurement_pending: bool,
    notify: Arc<dyn Fn() + Send + Sync>,
    layouts: BTreeMap<usize, LineLayout>,
    layout_revision: Option<u64>,
    layout_width: u32,
    undo_selection: Vec<SelectionHistory>,
    redo_selection: Vec<SelectionHistory>,
    selections: power::SelectionSet,
    pub bookmarks: power::Bookmarks,
    search_marks: search_marks::SearchMarks,
    pub language: bareline_syntax::Language,
    pub language_override: Option<bareline_syntax::Language>,
    pub detected_language: Option<bareline_syntax::Language>,
    pub syntax_preference: bareline_syntax::LexerPreference,
    pub udl: Option<Arc<bareline_syntax::udl::Definition>>,
    pub smart_typing: bool,
    pub smart_pairs: bool,
    pub smart_indent: bool,
    typing_syntax: Option<bareline_syntax::SyntaxResult>,
    known_folds: Vec<bareline_syntax::folding::Fold>,
    fold_state: bareline_syntax::folding::FoldState,
    hidden_lines: Vec<std::ops::RangeInclusive<usize>>,
    fold_revision: Option<u64>,
    pub folds_incomplete: bool,
    pending_folds: Vec<std::ops::Range<u64>>,
    pub encoding_label: String,
    eol_status_override: Option<String>,
    occurrence_history: power::OccurrenceHistory,
    group_pending: bool,
    font_pixels: f32,
    font_family: String,
    view_spacers: Vec<(usize,usize)>,
    tab_width: usize,
    line_numbers: bool,
    highlight_current_line: bool,
    whitespace: String,
    composition: Option<(String, Option<(usize, usize)>)>,
    composition_layout: Option<LayoutId>,
    reveal_caret: bool,
    initial_state: bareline_document::ContentStateId,
    pub visible_text: std::ops::Range<TextOffset>,
}
impl EditorSurface {
    pub fn new(
        service: DocumentService,
        snapshot: DocumentSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::from_snapshot(Some(service), snapshot, notify)
    }
    /// Loading snapshots are navigable/copyable, but have no mutation service.
    pub fn loading(snapshot: DocumentSnapshot, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::from_snapshot(None, snapshot, notify)
    }
    fn from_snapshot(
        service: Option<DocumentService>,
        snapshot: DocumentSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let initial_state = snapshot.content_state;
        Self {
            blink: Default::default(),
            wrap: false,
            wrap_rows: BTreeMap::new(),
            preferred_x: None,
            visual_navigation: VecDeque::new(),
            grapheme_navigation: None,
            virtual_lines: BTreeMap::new(),
            recovery: None,
            service,
            theme: bareline_ui::theme::EditorTheme::default(),
            user_read_only: false,
            snapshot,
            selection: Selection::default(),
            scroll_y: 0.0,
            top_inset: 0.0,
            bottom_inset: 0.0,
            search_selection: false,
            error: None,
            pending: None,
            queue: VecDeque::new(),
            queue_origins: VecDeque::new(),
            history_boundary: power::consumer::next_receipt_sequence(),
            acknowledged: VecDeque::new(),
            ordered_receipts: VecDeque::new(),
            acknowledged_commands: VecDeque::new(),
            pending_command: None,
            manual_hidden: Vec::new(),
            scroll_x: 0.0,
            power_rectangle: None,
            column_maps: BTreeMap::new(),
            column_maps_revision: None,
            column_map_bytes: 0,
            column_measurement_pending: false,
            notify,
            layouts: BTreeMap::new(),
            layout_revision: None,
            layout_width: 0,
            undo_selection: Vec::new(),
            redo_selection: Vec::new(),
            selections: Selection::default().into(),
            bookmarks: power::Bookmarks::default(),
            search_marks: search_marks::SearchMarks::default(),
            language: bareline_syntax::Language::PlainText,
            language_override: None,
            detected_language: None,
            syntax_preference: bareline_syntax::LexerPreference::Lexilla,
            udl: None,
            smart_typing: true,
            smart_pairs: true,
            smart_indent: true,
            typing_syntax: None,
            known_folds: Vec::new(),
            fold_state: Default::default(),
            hidden_lines: Vec::new(),
            fold_revision: None,
            folds_incomplete: false,
            pending_folds: Vec::new(),
            encoding_label: "UTF-8".into(),
            eol_status_override: None,
            occurrence_history: power::OccurrenceHistory::default(),
            group_pending: false,
            font_pixels: 16.0,
            font_family: "Cascadia Mono".into(),
            view_spacers: Vec::new(),
            tab_width: 4,
            line_numbers: true,
            highlight_current_line: true,
            whitespace: "none".into(),
            composition: None,
            composition_layout: None,
            reveal_caret: true,
            initial_state,
            visible_text: TextOffset(0)..TextOffset(0),
        }
    }
    pub fn snapshot(&self) -> &DocumentSnapshot {
        &self.snapshot
    }
    pub fn layout_range(&self, id: LayoutId) -> Option<std::ops::Range<TextOffset>> {
        self.layouts.values().find(|layout| layout.id == id).map(|layout| TextOffset(layout.start)..TextOffset(layout.end))
    }
    pub fn set_known_folds(&mut self, mut folds: Vec<bareline_syntax::folding::Fold>, level: usize, incomplete: bool) {
        folds.truncate(8192);
        folds.retain(|fold| fold.header < fold.end && fold.end < self.snapshot.line_count());
        folds.sort_by_key(|fold| (fold.header, std::cmp::Reverse(fold.end)));
        self.known_folds = folds;
        self.fold_revision = Some(self.snapshot.revision.0);
        self.folds_incomplete = incomplete;
        self.fold_state.apply_level(&self.known_folds, level);
        if !self.pending_folds.is_empty() {
            self.fold_state.unfold_all();
            for range in &self.pending_folds {
                if let Some(fold) = self.known_folds.iter().find(|fold| fold.header as u64 == range.start && fold.end as u64 + 1 == range.end) {
                    self.fold_state.collapsed.insert(fold.header);
                }
            }
            if !incomplete { self.pending_folds.clear(); }
        }
        self.refresh_hidden_lines();
    }
    /// Half-open logical line ranges, kept pending until verified fold metadata arrives.
    pub fn restore_folds(&mut self, ranges: &[std::ops::Range<u64>]) {
        self.pending_folds = ranges.iter().filter(|range| range.start < range.end && range.end <= self.snapshot.line_count() as u64).take(8192).cloned().collect();
    }
    pub fn persisted_folds(&self) -> Vec<std::ops::Range<u64>> {
        if !self.pending_folds.is_empty() { return self.pending_folds.clone(); }
        self.known_folds.iter().filter(|fold| self.fold_state.collapsed.contains(&fold.header)).map(|fold| fold.header as u64..fold.end as u64 + 1).collect()
    }
    pub fn has_pending_folds(&self) -> bool { !self.pending_folds.is_empty() }
    fn fold_anchors(&self) -> Vec<std::ops::Range<usize>> {
        self.persisted_folds().into_iter().filter_map(|range| {
            let start = self.snapshot.line_range(range.start as usize).ok()?.start.0;
            let end = self.snapshot.line_range(range.end.saturating_sub(1) as usize).ok()?.end.0;
            Some(start..end)
        }).collect()
    }
    fn mapped_folds(&self, transaction: &EditTransaction) -> Vec<std::ops::Range<usize>> {
        let mut edits: Vec<_> = transaction.edits.iter().collect();
        edits.sort_by_key(|edit| edit.range.start);
        let map = |offset: usize, right: bool| -> Option<usize> {
            let mut shifted = offset as i128;
            for edit in &edits {
                if edit.range.start.0 < offset && offset < edit.range.end.0 { return None; }
                if edit.range.end.0 < offset || (edit.range.end.0 == offset && (!edit.range.is_empty() || right)) {
                    shifted += edit.insert.len() as i128 - (edit.range.end.0 - edit.range.start.0) as i128;
                }
            }
            usize::try_from(shifted).ok()
        };
        self.fold_anchors().into_iter().filter_map(|range| {
            let start = map(range.start, true)?;
            let end = map(range.end, false)?;
            (start < end).then_some(start..end)
        }).collect()
    }
    fn restore_fold_anchors(&mut self, ranges: &[std::ops::Range<usize>]) {
        let lines: Vec<_> = ranges.iter().filter_map(|range| {
            if range.end > self.snapshot.len() { return None; }
            let first = self.snapshot.line_at(TextOffset(range.start)).ok()?;
            let mut end = range.end.saturating_sub(1);
            while !self.snapshot.is_boundary(TextOffset(end)) { end = end.checked_sub(1)?; }
            let last = self.snapshot.line_at(TextOffset(end)).ok()?;
            Some(first as u64..last as u64 + 1)
        }).collect();
        self.restore_folds(&lines);
        self.known_folds.clear(); self.refresh_hidden_lines(); self.fold_revision = None;
    }
    pub fn sync_fold_metadata_from(&mut self, other: &Self) {
        if !self.snapshot.same_document(&other.snapshot) || self.snapshot.revision != other.snapshot.revision || other.fold_revision.is_none() { return; }
        let collapsed = self.fold_state.collapsed.clone();
        let collapse_level = self.fold_state.collapse_level;
        let restore = !self.pending_folds.is_empty();
        self.set_known_folds(other.known_folds.clone(), self.fold_state.collapse_level.unwrap_or(8), other.folds_incomplete);
        if !restore {
            self.fold_state.collapsed = collapsed;
            self.fold_state.collapse_level = collapse_level;
            self.refresh_hidden_lines();
        }
    }
    pub fn unfold_all(&mut self) {
        self.fold_state.unfold_all();
        self.refresh_hidden_lines();
    }
    pub fn toggle_current_fold(&mut self) {
        let line = self.snapshot.line_at(TextOffset(self.selection.caret)).unwrap_or(0);
        if let Some(fold) = self.known_folds.iter().rev().find(|fold| fold.header <= line && line <= fold.end) {
            self.fold_state.toggle(fold.header);
            self.refresh_hidden_lines();
        }
    }
    fn refresh_hidden_lines(&mut self) {
        self.hidden_lines.clear();
        for fold in &self.known_folds {
            if self.fold_state.collapsed.contains(&fold.header) && fold.end > fold.header {
                if let Some(last) = self.hidden_lines.last_mut() {
                    if fold.header < *last.end() { continue; }
                }
                self.hidden_lines.push(fold.header + 1..=fold.end);
            }
        }
        self.hidden_lines.extend(self.manual_hidden.iter().cloned());
        self.hidden_lines.sort_by_key(|range| *range.start());
        let mut merged: Vec<std::ops::RangeInclusive<usize>> = Vec::new();
        for range in self.hidden_lines.drain(..) {
            if let Some(last) = merged.last_mut() {
                if *range.start() <= last.end().saturating_add(1) {
                    *last = *last.start()..=(*last.end()).max(*range.end());
                    continue;
                }
            }
            merged.push(range);
        }
        self.hidden_lines = merged;
        self.reveal_caret = false;
    }
    fn visual_line(&self, line: usize) -> usize {
        let hidden: usize = self.hidden_lines.iter().map(|r| {
            if line < *r.start() { 0 } else { line.min(*r.end()) - r.start() + 1 }
        }).sum();
        let spacers = self.view_spacers.iter().filter(|(before,_)| *before <= line && !self.hidden_lines.iter().any(|range| range.contains(before))).fold(0usize,|total,(_,rows)| total.saturating_add(*rows));
        let wrapped = if self.wrap { self.wrap_rows.range(..line).filter(|(line,_)| !self.hidden_lines.iter().any(|range| range.contains(line))).fold(0usize, |sum,(_,rows)| sum.saturating_add(rows.saturating_sub(1))) } else { 0 };
        line.saturating_sub(hidden).saturating_add(spacers).saturating_add(wrapped)
    }
    fn logical_line(&self, row: usize) -> usize {
        // Lower bound also maps a spacer hit to the next real logical line.
        let mut first = 0;
        let mut last = self.snapshot.line_count();
        while first < last {
            let middle = first + (last-first)/2;
            if self.visual_line(middle) < row { first=middle+1; } else { last=middle; }
        }
        if self.wrap && first > 0 {
            let previous = first-1;
            if row < self.visual_line(previous).saturating_add(self.wrap_rows.get(&previous).copied().unwrap_or(1)) { return previous; }
        }
        first
    }
    pub fn apply_visual_preferences(
        &mut self,
        font_size_pt: f64,
        tab_width: u8,
        line_numbers: bool,
        highlight_current_line: bool,
        whitespace: &str,
    ) {
        let next_font=(font_size_pt.clamp(6.0,72.0)*96.0/72.0)as f32;
        let changed=self.font_pixels!=next_font||self.tab_width!=usize::from(tab_width.clamp(1,16))||self.line_numbers!=line_numbers||self.highlight_current_line!=highlight_current_line||self.whitespace!=whitespace;
        if self.font_pixels!=next_font||self.tab_width!=usize::from(tab_width.clamp(1,16)){self.clear_column_metrics();}
        self.font_pixels = next_font;
        self.tab_width = usize::from(tab_width.clamp(1, 16));
        self.line_numbers = line_numbers;
        self.highlight_current_line = highlight_current_line;
        self.whitespace = whitespace.to_owned();
        if changed {self.layout_revision = None;self.reveal_caret = true;}
    }
    fn line_height(&self) -> f32 {
        self.font_pixels * 1.2
    }
    fn power_limits(&self) -> power::Limits {
        power::Limits {
            tab_width: self.tab_width,
            ..power::Limits::default()
        }
    }
    pub fn clone_view(&self) -> Self {
        let mut view = Self::from_snapshot(
            self.service.clone(),
            self.snapshot.clone(),
            self.notify.clone(),
        );
        view.initial_state = self.initial_state;
        view.theme = self.theme;
        view.language = self.language;
        view.language_override = self.language_override;
        view.detected_language = self.detected_language;
        view.syntax_preference = self.syntax_preference;
        view.udl = self.udl.clone();
        view.smart_typing = self.smart_typing;
        view.smart_pairs = self.smart_pairs;
        view.smart_indent = self.smart_indent;
        view.manual_hidden = self.manual_hidden.clone();
        view.scroll_x = self.scroll_x;
        view.wrap = self.wrap;
        view.wrap_rows = self.wrap_rows.clone();
        view.known_folds = self.known_folds.clone();
        view.fold_state = self.fold_state.clone();
        view.hidden_lines = self.hidden_lines.clone();
        view.fold_revision = self.fold_revision;
        view.folds_incomplete = self.folds_incomplete;
        view.pending_folds = self.pending_folds.clone();
        view.encoding_label = self.encoding_label.clone();
        view.eol_status_override = self.eol_status_override.clone();
        view.font_pixels = self.font_pixels;
        view.font_family = self.font_family.clone();
        view.tab_width = self.tab_width;
        view.line_numbers = self.line_numbers;
        view.highlight_current_line = self.highlight_current_line;
        view.whitespace = self.whitespace.clone();
        view
    }
    pub fn sync_saved_from(&mut self, peer: &EditorSurface) {
        if self.snapshot.same_document(&peer.snapshot) {
            self.initial_state = peer.initial_state;
            self.encoding_label = peer.encoding_label.clone();
            self.eol_status_override = peer.eol_status_override.clone();
        }
    }
    /// Supplies an authoritative whole-document label for a viewport-backed surface.
    /// The owner refreshes this after document changes and uses "Computing" while
    /// full counts are unavailable. None restores the resident snapshot default.
    pub fn set_eol_status_override(&mut self, label: Option<String>) {
        self.eol_status_override = label;
    }
    pub fn eol_status_label(&self) -> &str {
        self.eol_status_override
            .as_deref()
            .unwrap_or_else(|| self.snapshot.eol_label())
    }
    pub fn refresh_peer(&mut self, snapshot: &DocumentSnapshot) -> bool {
        if self.busy()
            || !self.snapshot.same_document(snapshot)
            || self.snapshot.revision.0 >= snapshot.revision.0
        {
            return false;
        }
        self.snapshot = snapshot.clone();
        for offset in [&mut self.selection.anchor, &mut self.selection.caret] {
            *offset = (*offset).min(snapshot.len());
            while !snapshot.is_boundary(TextOffset(*offset)) {
                *offset -= 1;
            }
        }
        self.selections = self.selection.into();
        self.undo_selection.clear();
        self.redo_selection.clear();
        self.reveal_caret = true;
        true
    }
    pub fn selection_set(&self) -> power::SelectionSet {
        if self.selections.primary() == self.selection {
            self.selections.clone()
        } else {
            self.selection.into()
        }
    }
    pub fn set_selections(&mut self, selections: power::SelectionSet) -> Result<(), String> {
        self.history_boundary=power::consumer::next_receipt_sequence();
        let selections = power::normalize(&self.snapshot, &selections, self.power_limits())
            .map_err(|error| format!("Selection unavailable: {error:?}"))?;
        self.selection = selections.primary();
        self.selections = selections;
        self.power_rectangle = None;
        self.reveal_caret = true;
        Ok(())
    }
    pub fn execute_power(&mut self, command: &str) -> Result<(), String> {
        if self.read_only() || self.busy() || self.composition.is_some() {
            return Err("Wait for loading, composition or the pending edit to finish.".into());
        }
        let limits = self.power_limits();
        let set = self.selection_set();
        let error = |error| format!("Command was not applied: {error:?}");
        if matches!(
            command,
            "editor.comment.toggleLine" | "editor.comment.toggleBlock"
        ) {
            let prepared = if let Some(definition)=self.udl.as_deref() {
                completion::toggle_comment_with_provider(&self.snapshot,&set,&completion::DefinitionComments(definition),command.ends_with("toggleBlock"),limits)
            } else {
                completion::toggle_comment(&self.snapshot,&set,self.language,command.ends_with("toggleBlock"),limits)
            }.map_err(error)?;
            return self.submit_power(prepared).map_err(str::to_owned);
        }
        if let Some(transform) = power::transform_for_command(command) {
            let prepared =
                power::transform(&self.snapshot, &set, transform, limits).map_err(error)?;
            return self.submit_power(prepared).map_err(str::to_owned);
        }
        let next = match command {
            "editor.caret.above" | "editor.caret.below" => Some(
                power::add_caret(&self.snapshot, &set, command.ends_with("below"), limits)
                    .map_err(error)?,
            ),
            "editor.selection.nextOccurrence"
            | "editor.selection.allOccurrences"
            | "editor.selection.skipOccurrence" => {
                let next = if command.ends_with("skipOccurrence") {
                    power::skip_occurrence(&self.snapshot, &set, limits)
                } else {
                    power::select_occurrences(
                        &self.snapshot,
                        &set,
                        command.ends_with("allOccurrences"),
                        limits,
                    )
                }
                .map_err(error)?;
                self.occurrence_history
                    .remember(&set, limits)
                    .map_err(error)?;
                Some(next)
            }
            "editor.selection.undoOccurrence" => self.occurrence_history.undo(),
            "editor.selection.rotatePrimary" => {
                let mut next = set;
                next.rotate_primary();
                Some(next)
            }
            "editor.selection.escape" => {
                let mut next = set;
                next.escape();
                Some(next)
            }
            "editor.selection.expandLines" => {
                Some(power::expand_lines(&self.snapshot, &set, limits).map_err(error)?)
            }
            "editor.bookmark.toggle" => {
                self.bookmarks
                    .toggle(&self.snapshot, self.selection.caret)
                    .map_err(error)?;
                None
            }
            "editor.bookmark.clear" => {
                self.bookmarks.clear();
                None
            }
            "editor.bookmark.next" | "editor.bookmark.previous" => self
                .bookmarks
                .next(self.selection.caret, command.ends_with("previous"))
                .map(|caret| {
                    Selection {
                        anchor: caret,
                        caret,
                    }
                    .into()
                }),
            "editor.bookmark.selectLines" => Some(
                self.bookmarks
                    .selections(&self.snapshot, limits)
                    .map_err(error)?,
            ),
            _ => {
                return Err(
                    "This command requires an additional editor dialog or language definition."
                        .into(),
                );
            }
        };
        if let Some(next) = next {
            self.set_selections(next)?;
        }
        Ok(())
    }
    fn submit_power(&mut self, prepared: power::PowerEdit) -> Result<(), &'static str> {
        self.history_boundary=power::consumer::next_receipt_sequence();
        if prepared.transaction.edits.is_empty() {
            return Ok(());
        }
        let mut bookmarks_after = self.bookmarks.clone();
        bookmarks_after.map_edits(&prepared.transaction);
        let folds_before = self.fold_anchors();
        let folds_after = self.mapped_folds(&prepared.transaction);
        let marks_after = self.search_marks.mapped(&prepared.transaction);
        let receiver = self
            .service
            .as_ref()
            .ok_or("File is still loading.")?
            .submit_with_notify(
                Mutation::Apply(prepared.transaction),
                Some(self.notify.clone()),
            )
            .map_err(|_| "Document worker is busy.")?;
        self.pending = Some(Pending {
            folds_before,
            folds_after,
            input: None,
            receiver,
            before: self.selection_set(),
            after: prepared.selections,
            bookmarks_before: self.bookmarks.clone(),
            marks_before: self.search_marks.clone(),
            bookmarks_after,
            marks_after,
            history: HistoryMove::Edit,
        });
        Ok(())
    }
    pub fn apply_document_metadata(&mut self, metadata: bareline_document::DocumentMetadata) -> Result<(), String> {
        if self.read_only() || self.busy() { return Err("Document is busy or read only".into()); }
        let receiver=self.service.as_ref().ok_or("Document unavailable")?.submit_metadata(self.snapshot.revision,metadata,Some(self.notify.clone())).map_err(|_|"Document worker is busy")?;
        self.pending=Some(Pending {folds_before:self.fold_anchors(),folds_after:self.fold_anchors(),input:None,receiver,before:self.selection_set(),after:self.selection_set(),bookmarks_before:self.bookmarks.clone(),bookmarks_after:self.bookmarks.clone(),marks_before:self.search_marks.clone(),marks_after:self.search_marks.clone(),history:HistoryMove::Edit});
        Ok(())
    }
    pub fn apply_power(&mut self, prepared: power::PowerEdit) -> Result<(), String> {
        if self.read_only() || self.busy() || self.composition.is_some() {
            return Err("Document is busy or read only.".into());
        }
        if prepared.transaction.base_revision != self.snapshot.revision {
            return Err("Document changed; prepare the edit again.".into());
        }
        self.submit_power(prepared).map_err(str::to_owned)
    }
    pub fn composition_text(&self) -> Option<&str> {
        self.composition.as_ref().map(|(text, _)| text.as_str())
    }
    pub fn finish_loading(&mut self, service: DocumentService, snapshot: DocumentSnapshot) {
        self.initial_state = snapshot.content_state;
        self.snapshot = snapshot;
        self.service = Some(service);
        self.error = None;
    }
    pub fn enable_recovery(&mut self, root: std::path::PathBuf, platform: Arc<dyn bareline_platform::LocalFileSystem>, encoding: Option<bareline_file_io::codecs::resident::ResidentEncoding>, original_path: Option<std::path::PathBuf>, bytes: bareline_document::Budget) {
        if let Some(recovery) = &mut self.recovery { recovery.set_original_path(original_path); } else if self.snapshot.is_complete() { self.recovery = Some(bareline_file_io::resident_recovery::ResidentRecovery::new(root, platform, encoding, original_path, bytes, self.notify.clone())); }
    }
    pub fn recovery_status(&self) -> bareline_file_io::paged_recovery::PagedRecoveryStatus { self.recovery.as_ref().map(|recovery|recovery.status()).unwrap_or_default() }
    pub fn retry_recovery(&mut self) { if let Some(recovery)=&mut self.recovery { recovery.retry(); } }
    pub fn read_only(&self) -> bool {
        self.user_read_only || self.service.is_none() || !self.snapshot.is_complete()
    }
    pub fn can_undo(&self) -> bool { !self.undo_selection.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo_selection.is_empty() }
    pub fn dirty(&self) -> bool {
        self.snapshot.content_state != self.initial_state
    }
    pub fn selected_text(&self) -> Result<String, &'static str> {
        if self.busy() {
            return Err("Wait for the pending edit before copying.");
        }
        let limit=4*1024*1024;
        if let Some(rectangle)=self.power_rectangle {
            return self.copy_rectangle(rectangle,limit).map_err(|_|"Rectangle exceeds the clipboard limit.");
        }
        let mut text=String::new();
        for (index,selection) in self.selection_set().selections.iter().enumerate() {
            if index>0 {if text.len()>=limit{return Err("Selection exceeds the clipboard limit.");}text.push('\n');}
            let range=selection.range();
            text.push_str(&self.snapshot.read(TextOffset(range.start)..TextOffset(range.end),limit-text.len()).map_err(|_|"Selection exceeds the clipboard limit.")?);
        }
        Ok(text)
    }
    pub fn mark_saved(&mut self, captured: &DocumentSnapshot) {
        if self.snapshot.same_document(captured) {
            self.initial_state = captured.content_state;
        }
    }
    pub fn busy(&self) -> bool {
        self.column_measurement_pending || self.group_pending || self.pending.is_some() || !self.queue.is_empty()
    }
    /// Accept worker-prepared edits only against their original document and revision.
    pub fn apply_prepared(
        &mut self,
        source: &DocumentSnapshot,
        transaction: EditTransaction,
    ) -> Result<(), &'static str> {
        if self.read_only() {
            return Err("File is still loading; replacement is unavailable.");
        }
        if self.busy() || self.composition.is_some() {
            return Err("Document is busy; replacement was not applied.");
        }
        if !self.snapshot.same_document(source)
            || self.snapshot.revision != source.revision
            || transaction.base_revision != source.revision
        {
            return Err("Document changed; run replacement again.");
        }
        let Some(first) = transaction.edits.first() else {
            return Err("No matches to replace.");
        };
        let caret = first
            .range
            .start
            .0
            .checked_add(first.insert.len())
            .ok_or("Replacement is too large.")?;
        let mut bookmarks_after = self.bookmarks.clone();
        bookmarks_after.map_edits(&transaction);
        let folds_before = self.fold_anchors();
        let folds_after = self.mapped_folds(&transaction);
        let marks_after = self.search_marks.mapped(&transaction);
        let receiver = self
            .service
            .as_ref()
            .ok_or("File is still loading.")?
            .submit_with_notify(Mutation::Apply(transaction), Some(self.notify.clone()))
            .map_err(|_| "Document worker is busy; replacement was not applied.")?;
        self.pending = Some(Pending {
            folds_before,
            folds_after,
            input: None,
            receiver,
            after: Selection {
                anchor: caret,
                caret,
            }
            .into(),
            before: self.selection_set(),
            bookmarks_before: self.bookmarks.clone(),
            marks_before: self.search_marks.clone(),
            bookmarks_after,
            marks_after,
            history: HistoryMove::Edit,
        });
        self.search_selection = false;
        self.reveal_caret = true;
        Ok(())
    }
    pub fn preedit(&mut self, value: String, cursor: Option<(usize, usize)>) {
        if self.read_only() {
            return;
        }
        if value.is_empty() {
            self.composition = None;
        } else if value.len() <= MAX_LAYOUT_BYTES && !value.contains(['\r', '\n']) {
            let cursor = cursor
                .filter(|&(a, b)| a <= b && value.is_char_boundary(a) && value.is_char_boundary(b));
            self.composition = Some((value, cursor));
        }
    }
    pub fn cancel_composition(&mut self) {
        self.composition = None;
    }
    pub fn commit(&mut self, value: String) {
        self.cancel_composition();
        self.enqueue_with_origin(Input::Insert(value),bareline_document::history::EditOrigin::Command);
    }
    pub fn take_acknowledged_inputs(&mut self) -> Vec<Input> { self.acknowledged.drain(..).collect() }
    fn acknowledge(&mut self, input: Input) {
        self.acknowledge_event(power::consumer::ReceiptEvent::Input(input.clone()));
        if self.acknowledged.len() == MAX_QUEUED_INPUTS { self.acknowledged.pop_front(); }
        self.acknowledged.push_back(input);
    }
    pub fn enqueue(&mut self, input: Input) {
        let origin=if matches!(&input,Input::Insert(text) if text.chars().count()==1) {bareline_document::history::EditOrigin::Typing}else{bareline_document::history::EditOrigin::Command};
        self.enqueue_with_origin(input,origin);
    }
    pub fn enqueue_with_origin(&mut self, input: Input, origin:bareline_document::history::EditOrigin) {
        if self.read_only()
            && matches!(
                input,
                Input::Insert(_) | Input::Backspace | Input::Delete | Input::Undo | Input::Redo
            )
        {
            self.error = Some("Loading — read only. Close the tab to cancel.".into());
            return;
        }
        self.search_selection = false;
        if self.composition.is_some() {
            return;
        }
        if self.queue.len() >= MAX_QUEUED_INPUTS {
            self.error = Some("Input queue is full. Wait for the pending edit.".into());
            return;
        }
        self.queue.push_back(input);
        self.queue_origins.push_back(origin);
        self.pump();
    }
    pub fn pump(&mut self) -> bool {
        let navigation_changed = self.pump_virtual_navigation();
        if self.column_measurement_pending { return navigation_changed; }
        if self.group_pending {
            return navigation_changed;
        }
        let mut changed = navigation_changed;
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(completion) => {
                    let pending = self.pending.take().unwrap();
                    self.snapshot = completion.snapshot;
                    match completion.result {
                        Ok(_) => {
                            if let Some(command) = self.pending_command.take() { self.acknowledge_command(command); }
                            self.restore_fold_anchors(&pending.folds_after);
                            if let Some(input) = pending.input.clone() { self.acknowledge(input); }
                            self.power_rectangle = None;
                            self.selection = pending.after.primary();
                            self.selections = pending.after.clone();
                            self.bookmarks = pending.bookmarks_after.clone();
                            self.search_marks = pending.marks_after.clone();
                            match pending.history {
                                HistoryMove::Edit => {
                                    let merged=completion.metadata.as_ref().is_some_and(|metadata| metadata.origin==bareline_document::history::EditOrigin::Typing && self.undo_selection.last().is_some_and(|entry|power::consumer::history_selections(&entry.before)==metadata.before));
                                    let entry=SelectionHistory {
                                        folds_before: pending.folds_before,
                                        folds_after: pending.folds_after,
                                        before: pending.before,
                                        after: pending.after,
                                        bookmarks_before: pending.bookmarks_before,
                                        marks_before: pending.marks_before,
                                        bookmarks_after: pending.bookmarks_after,
                                        marks_after: pending.marks_after,
                                        group: None,
                                    };
                                    if merged {
                                        let previous=self.undo_selection.last_mut().unwrap();
                                        previous.after=entry.after;previous.bookmarks_after=entry.bookmarks_after;previous.folds_after=entry.folds_after;previous.marks_after=entry.marks_after;
                                    }else{self.undo_selection.push(entry);}
                                    self.redo_selection.clear();
                                }
                                HistoryMove::Undo => {
                                    if let Some(entry) = self.undo_selection.pop() {
                                        self.redo_selection.push(entry);
                                    }
                                }
                                HistoryMove::Redo => {
                                    if let Some(entry) = self.redo_selection.pop() {
                                        self.undo_selection.push(entry);
                                    }
                                }
                            }
                            if self.undo_selection.len()>completion.undo_depth {self.undo_selection.drain(..self.undo_selection.len()-completion.undo_depth);}
                            if self.redo_selection.len()>completion.redo_depth {self.redo_selection.drain(..self.redo_selection.len()-completion.redo_depth);}
                            self.error = None;
                        }
                        Err(error) => {
                            self.pending_command = None;
                            self.error = Some(format!("Edit was not applied: {error:?}"));
                            self.queue.clear(); self.queue_origins.clear();
                        }
                    }
                    changed = true;
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.pending_command = None;
                    self.queue.clear(); self.queue_origins.clear();
                    self.error = Some("Document worker stopped.".into());
                    return true;
                }
            }
        }
        while self.pending.is_none() {
            let Some(input) = self.queue.pop_front() else {
                break;
            };
            let mut origin=self.queue_origins.pop_front().unwrap_or_default();
            if self.selection_set().selections.len()>1 {origin=bareline_document::history::EditOrigin::MultiCursor;}
            if origin!=bareline_document::history::EditOrigin::Typing {self.history_boundary=power::consumer::next_receipt_sequence();}
            if matches!(input, Input::SetCaret(..)) { self.power_rectangle = None; }
            let before = self.selection_set();
            let smart = self.smart_typing && (self.language != bareline_syntax::Language::PlainText || self.udl.is_some());
            if smart && self.smart_pairs && self.power_rectangle.is_none() && let Input::Insert(value) = &input && value.chars().count() == 1 {
                if let Ok(Some(next)) = completion::overtype_closer(&self.snapshot, &before, value.chars().next().unwrap(), self.power_limits()) {
                    self.selection = next.primary(); self.selections = next;
                    self.acknowledge(input); changed = true; continue;
                }
            }
            let mut history = HistoryMove::Edit;
            let rectangle = self.power_rectangle;
            let operation = match &input {
                Input::Insert(value) if rectangle.is_some() => Some(self.prepare_rectangle_paste(rectangle.unwrap(), value)),
                Input::Backspace | Input::Delete if rectangle.is_some() => Some(self.prepare_rectangle_paste(rectangle.unwrap(), "")),
                Input::Insert(value) if smart && self.smart_indent && matches!(value.as_str(), "\n" | "\r\n" | "\r") => {
                    let inside_literal = self.typing_syntax.as_ref().filter(|s| s.is_current(&self.snapshot)).is_some_and(|s| s.spans.iter().any(|span| span.range.start.0 < self.selection.caret && self.selection.caret <= span.range.end.0 && matches!(span.kind, bareline_syntax::StyleKind::Comment | bareline_syntax::StyleKind::String)));
                    Some(completion::smart_newline(&self.snapshot, &before, if inside_literal { bareline_syntax::Language::PlainText } else { self.language }, self.power_limits()))
                }
                Input::Insert(value) if smart && self.smart_pairs && value.chars().count() == 1 => Some(completion::smart_pair_configured(&self.snapshot, &before, value.chars().next().unwrap(), self.language, self.typing_syntax.as_ref(), self.power_limits(), self.udl.as_deref())),
                Input::Backspace if smart && self.smart_pairs => Some(completion::pair_backspace(&self.snapshot, &before, self.power_limits())),
                Input::Insert(value) => Some(power::replace(
                    &self.snapshot,
                    &before,
                    value,
                    power::Limits::default(),
                )),
                Input::Backspace | Input::Delete => Some(power::delete(
                    &self.snapshot,
                    &before,
                    matches!(input, Input::Backspace),
                    power::Limits::default(),
                )),
                _ => None,
            };
            let mut after = before.clone();
            let bookmarks_before = self.bookmarks.clone();
            let folds_before = self.fold_anchors();
            let mut folds_after = folds_before.clone();
            let mut bookmarks_after = bookmarks_before.clone();
            let marks_before = self.search_marks.clone();
            let mut marks_after = marks_before.clone();
            let mutation = if let Some(operation) = operation {
                let prepared = match operation {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.error = Some(format!("Edit was not applied: {error:?}"));
                        self.queue.clear(); self.queue_origins.clear();
                        break;
                    }
                };
                if prepared
                    .transaction
                    .edits
                    .iter()
                    .all(|edit| edit.range.is_empty() && edit.insert.is_empty())
                {
                    continue;
                }
                after = prepared.selections;
                bookmarks_after.map_edits(&prepared.transaction);
                marks_after = self.search_marks.mapped(&prepared.transaction);
                folds_after = self.mapped_folds(&prepared.transaction);
                Some(Mutation::Apply(prepared.transaction))
            } else {
                match &input {
                    Input::Undo if !self.undo_selection.is_empty() => {
                        history = HistoryMove::Undo;
                        let entry = self.undo_selection.last().unwrap();
                        after = entry.before.clone();
                        bookmarks_after = entry.bookmarks_before.clone();
                        marks_after = entry.marks_before.clone();
                        folds_after = entry.folds_before.clone();
                        Some(Mutation::Undo)
                    }
                    Input::Redo if !self.redo_selection.is_empty() => {
                        history = HistoryMove::Redo;
                        let entry = self.redo_selection.last().unwrap();
                        after = entry.after.clone();
                        bookmarks_after = entry.bookmarks_after.clone();
                        marks_after = entry.marks_after.clone();
                        folds_after = entry.folds_after.clone();
                        Some(Mutation::Redo)
                    }
                    _ => None,
                }
            };
            if let Some(mutation) = mutation {
                let service=self.service.as_ref().expect("mutations are rejected for loading previews");
                let submission=match mutation {
                    Mutation::Apply(transaction) if before.selections.len()<=1024 && after.selections.len()<=1024 => {
                        let metadata=bareline_document::history::EditMetadata {
                            before:power::consumer::history_selections(&before),after:power::consumer::history_selections(&after),origin,boundary:self.history_boundary,monotonic_ms:power::consumer::monotonic_ms(),
                        };
                        service.submit_with_metadata(transaction,metadata,Some(self.notify.clone())).map_err(|(error,transaction,_)|(error,Mutation::Apply(transaction)))
                    }
                    mutation=>service.submit_with_notify(mutation,Some(self.notify.clone())),
                };
                match submission
                {
                    Ok(receiver) => {
                        self.pending = Some(Pending {
                            folds_before,
                            folds_after,
                            input: Some(input.clone()),
                            receiver,
                            after,
                            before,
                            bookmarks_before,
                            bookmarks_after,
                            marks_before,
                            marks_after,
                            history,
                        })
                    }
                    Err((SubmitError::Saturated, _)) => {
                        self.queue.push_front(input);
                        self.queue_origins.push_front(origin);
                        break;
                    }
                    Err((SubmitError::Closed | SubmitError::InvalidGroup, _)) => {
                        self.error = Some("Document service closed.".into());
                        self.queue.clear(); self.queue_origins.clear();
                        break;
                    }
                }
            } else {
                let acknowledged = input.clone();
                self.navigate(input);
                self.acknowledge(acknowledged);
                self.selections = self.selection.into();
                changed = true;
            }
        }
        let dirty = self.dirty();
        if let Some(recovery) = &mut self.recovery { changed |= recovery.observe(self.snapshot.clone(), dirty); }
        if changed {
            self.reveal_caret = true;
        }
        changed
    }
    fn previous_grapheme(&self, offset: usize) -> Option<usize> {
        if offset == 0 {
            return None;
        }
        let mut start = offset.saturating_sub(MAX_LAYOUT_BYTES);
        while !self.snapshot.is_boundary(TextOffset(start)) {
            start += 1;
        }
        let text = self
            .snapshot
            .read(TextOffset(start)..TextOffset(offset), MAX_LAYOUT_BYTES)
            .ok()?;
        let (index, _) = text.grapheme_indices(true).next_back()?;
        if index == 0 && start > 0 {
            return None;
        } // don't split a cluster extending beyond the bounded window
        Some(start + index)
    }
    fn next_grapheme(&self, offset: usize) -> Option<usize> {
        if offset == self.snapshot.len() {
            return None;
        }
        let mut end = (offset + MAX_LAYOUT_BYTES).min(self.snapshot.len());
        while !self.snapshot.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        let text = self
            .snapshot
            .read(TextOffset(offset)..TextOffset(end), MAX_LAYOUT_BYTES)
            .ok()?;
        let cluster = text.graphemes(true).next()?;
        if cluster.len() == text.len() && end < self.snapshot.len() {
            return None;
        }
        Some(offset + cluster.len())
    }
    fn content_range(&self, line: usize) -> Option<std::ops::Range<usize>> {
        let range = self.snapshot.line_range(line).ok()?;
        let start = range.start.0;
        let mut end = range.end.0;
        let tail_start = end.saturating_sub(2).max(start);
        // Only inspect ASCII newline bytes; avoid slicing a UTF-8 continuation in the prefix.
        let mut safe = tail_start;
        while safe < end && !self.snapshot.is_boundary(TextOffset(safe)) {
            safe += 1;
        }
        let tail = self
            .snapshot
            .read(TextOffset(safe)..TextOffset(end), 2)
            .ok()?;
        if tail.ends_with('\n') {
            end -= 1;
        }
        if tail.trim_end_matches('\n').ends_with('\r') {
            end -= 1;
        }
        Some(start..end)
    }
    fn navigate(&mut self, input: Input) {
        self.reset_caret_blink();
        if matches!(input, Input::Up(_) | Input::Down(_)) {
            if self.visual_navigation.len() < MAX_QUEUED_INPUTS { self.visual_navigation.push_back(input); (self.notify)(); }
            return;
        }
        self.preferred_x = None;
        if let Input::Left(extend) | Input::Right(extend) = input {
            let forward = matches!(input, Input::Right(_));
            let origin = self.selection.caret;
            let target = if forward { self.next_grapheme(origin) } else { self.previous_grapheme(origin) };
            if target.is_none() && ((forward && origin < self.snapshot.len()) || (!forward && origin > 0)) {
                if self.grapheme_navigation.is_none() {
                    match grapheme_navigation::Navigation::start(self.snapshot.clone(),origin,forward,extend,self.notify.clone()) {
                        Ok(job)=>self.grapheme_navigation=Some(job), Err(error)=>self.error=Some(error),
                    }
                }
                return;
            }
        }
        let caret = self.selection.caret;
        let line = self.snapshot.line_at(TextOffset(caret)).unwrap_or(0);
        let (target, extend) = match input {
            Input::Left(extend) => (self.previous_grapheme(caret).unwrap_or(caret), extend),
            Input::Right(extend) => (self.next_grapheme(caret).unwrap_or(caret), extend),
            Input::Home(extend) => (self.content_range(line).map_or(caret, |r| r.start), extend),
            Input::End(extend) => (self.content_range(line).map_or(caret, |r| r.end), extend),
            Input::SetCaret(target, extend) if self.snapshot.is_boundary(TextOffset(target)) => {
                (target, extend)
            }
            Input::Up(extend) | Input::Down(extend) => {
                let target_line = if matches!(input, Input::Up(_)) {
                    line.saturating_sub(1)
                } else {
                    (line + 1).min(self.snapshot.line_count() - 1)
                };
                let current = self.content_range(line).unwrap();
                let target = self.content_range(target_line).unwrap();
                let prefix = self
                    .snapshot
                    .read(
                        TextOffset(current.start)..TextOffset(caret.min(current.end)),
                        MAX_LAYOUT_BYTES,
                    )
                    .unwrap_or_default();
                let count = prefix.graphemes(true).count();
                let mut end = target.end.min(target.start + MAX_LAYOUT_BYTES);
                while !self.snapshot.is_boundary(TextOffset(end)) {
                    end -= 1;
                }
                let text = self
                    .snapshot
                    .read(TextOffset(target.start)..TextOffset(end), MAX_LAYOUT_BYTES)
                    .unwrap_or_default();
                let offset = text
                    .grapheme_indices(true)
                    .nth(count)
                    .map_or(text.len(), |(i, _)| i);
                (target.start + offset, extend)
            }
            Input::SelectAll => {
                self.selection = Selection {
                    anchor: 0,
                    caret: self.snapshot.len(),
                };
                return;
            }
            _ => return,
        };
        self.selection.caret = target;
        if !extend {
            self.selection.anchor = target;
        }
    }
    /// Copies bounded glyph geometry from existing layouts only. Coordinates are
    /// logical editor pixels; no shaping or source paging occurs here.
    pub fn accessibility_geometry(&self, backend: &impl TextBackend, width: f32, height: f32) -> Vec<(std::ops::Range<usize>, Rect)> {
        let mut result = Vec::new();
        let mut remaining = MAX_LAYOUT_BYTES;
        let composition = self.composition.as_ref().map(|(text,_)| text.as_str());
        let caret = self.selection.caret;
        let caret_line = self.snapshot.line_at(TextOffset(caret)).ok();
        for (line, layout) in &self.layouts {
            let start = layout.start.max(self.visible_text.start.0);
            let end = layout.end.min(self.visible_text.end.0);
            if start > end || end-start > remaining { continue; }
            let Ok(mut text) = self.snapshot.read(TextOffset(start)..TextOffset(end), remaining) else { continue; };
            remaining -= text.len();
            let composed = composition.is_some() && caret_line == Some(*line);
            let draw_id = if composed {
                let (Some(id), Some(preedit)) = (self.composition_layout, composition) else { continue; };
                if caret < start || caret > end || preedit.len() > remaining { continue; }
                text.insert_str(caret-start, preedit);
                remaining -= preedit.len();
                id
            } else { layout.id };
            let shift = if !composed && start >= caret { composition.map_or(0,str::len) } else { 0 };
            let origin_y = self.top() + ((self.visual_line(*line)+layout.row_origin) as f64 * self.line_height() as f64-self.scroll_y) as f32;
            for (offset, grapheme) in text.grapheme_indices(true) {
                if result.len() >= 4096 { return result; }
                let a = start+shift+offset;
                let b = a+grapheme.len();
                let local = start-layout.start+offset;
                let Ok(rects) = backend.range_rects(draw_id, local..local+grapheme.len()) else { continue; };
                for r in rects {
                    let x = r.x+LEFT+(layout.x_origin-self.scroll_x) as f32;
                    let y = r.y+origin_y;
                    let left = x.max(LEFT);
                    let top = y.max(self.top());
                    let right = (x+r.width).min(width);
                    let bottom = (y+r.height).min(height-STATUS_HEIGHT-self.bottom_inset);
                    if right > left && bottom > top {
                        result.push((a..b, rect(left, top, right-left, bottom-top)));
                    }
                    if result.len() >= 4096 { return result; }
                }
            }
        }
        result
    }
    /// Reveal a canonical text offset without changing selection or document.
    pub fn accessibility_scroll_to(&mut self, offset: usize, height: f32) -> bool {
        let Ok(line) = self.snapshot.line_at(TextOffset(offset)) else { return false; };
        let destination = self.visual_line(line) as f64*self.line_height() as f64;
        self.reveal_caret = false;
        self.scroll(destination-self.scroll_y, height);
        true
    }
    pub fn scroll(&mut self, delta: f64, height: f32) {
        let max = (self.visual_line(self.snapshot.line_count()) as f64 * self.line_height() as f64
            - (height - self.top() - STATUS_HEIGHT) as f64)
            .max(0.0);
        self.scroll_y = (self.scroll_y + delta).clamp(0.0, max);
    }
    fn top(&self) -> f32 {
        TAB_HEIGHT + self.top_inset
    }
    pub fn release_layouts(&mut self, backend: &mut impl TextBackend) {
        self.virtual_lines.clear();
        self.wrap_rows.clear();
        for (_, layout) in std::mem::take(&mut self.layouts) {
            backend.release_layout(layout.id);
        }
        if let Some(id) = self.composition_layout.take() {
            backend.release_layout(id);
        }
        self.layout_revision = None;
    }
    pub fn click(
        &mut self,
        backend: &impl TextBackend,
        p: Point,
        extend: bool,
    ) -> Result<(), LayoutError> {
        if !self.busy() && (38.0..LEFT).contains(&p.x) && p.y >= self.top() {
            let row = (((p.y - self.top()) as f64 + self.scroll_y) / self.line_height() as f64).floor() as usize;
            let line = self.logical_line(row);
            if self.known_folds.iter().any(|fold| fold.header == line) {
                self.fold_state.toggle(line); self.refresh_hidden_lines();
            }
            return Ok(());
        }
        if self.busy() || self.composition.is_some() || p.x < LEFT || p.y < self.top() {
            return Ok(());
        }
        let line = ((p.y - self.top()) as f64 + self.scroll_y) / self.line_height() as f64;
        let number = self.logical_line(line.floor() as usize);
        if let Some(layout) = self.layouts.get(&number) {
            let hit = backend.hit_test(
                layout.id,
                Point {
                    x: p.x - LEFT + (self.scroll_x-layout.x_origin) as f32,
                    y: ((line-(self.visual_line(number)+layout.row_origin) as f64) * self.line_height() as f64) as f32,
                },
            )?;
            let offset = (layout.start + hit.byte_offset).min(layout.end);
            let text = self
                .snapshot
                .read(
                    TextOffset(layout.start)..TextOffset(layout.end),
                    MAX_LAYOUT_BYTES,
                )
                .unwrap_or_default();
            let snapped = layout.start
                + text
                    .grapheme_indices(true)
                    .map(|(i, _)| i)
                    .chain(Some(text.len()))
                    .take_while(|i| *i <= offset - layout.start)
                    .last()
                    .unwrap_or(0);
            if layout.start > self.content_range(number).map_or(layout.start,|range|range.start) {
                if self.grapheme_navigation.is_none() {
                    self.grapheme_navigation=Some(grapheme_navigation::Navigation::start_snap(self.snapshot.clone(),self.selection.caret,snapped,extend,self.notify.clone()).map_err(|_|LayoutError::BackendFailure)?);
                }
            } else {self.enqueue(Input::SetCaret(snapped, extend));}
        }
        Ok(())
    }
    pub fn draw(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        self.draw_styled(
            backend,
            width,
            height,
            ops,
            SyntaxView {
                result: None,
                language: "Plain text",
                unavailable: false,
            },
        )
    }
    pub fn draw_styled(
        &mut self,
        backend: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
        styling: SyntaxView<'_>,
    ) -> Result<Option<Rect>, LayoutError> {
        let syntax = styling
            .result
            .filter(|result| result.is_current(&self.snapshot));
        let language = styling.language;
        if let Some(syntax) = syntax {
            if !self.typing_syntax.as_ref().is_some_and(|old| old.is_current(&self.snapshot) && old.range == syntax.range && old.language == syntax.language) {
                self.typing_syntax = Some(syntax.clone());
            }
        }
        if self.fold_revision.is_some_and(|r| r != self.snapshot.revision.0) {
            self.known_folds.clear(); self.refresh_hidden_lines(); self.fold_revision = None;
            self.folds_incomplete = true;
        }
        let body_height = (height - self.top() - STATUS_HEIGHT - self.bottom_inset).max(0.0);
        let body = rect(0.0, self.top(), width, body_height);
        let reveal_requested=self.reveal_caret;
        let caret_line = self
            .snapshot
            .line_at(TextOffset(self.selection.caret))
            .unwrap_or(0);
        if self.reveal_caret {
            // Keyboard navigation into a collapsed body reveals its containing fold.
            if self.hidden_lines.iter().any(|range| range.contains(&caret_line)) {
                self.manual_hidden.retain(|range| !range.contains(&caret_line));
                for fold in &self.known_folds {
                    if fold.header < caret_line && caret_line <= fold.end { self.fold_state.collapsed.remove(&fold.header); }
                }
                self.refresh_hidden_lines();
            }
            let caret_y = self.layouts.get(&caret_line).filter(|layout| (layout.start..=layout.end).contains(&self.selection.caret)).and_then(|layout| backend.caret(layout.id, self.selection.caret-layout.start).ok()).map_or(0.0, |rect|rect.y);
            let top = self.visual_line(caret_line) as f64 * self.line_height() as f64 + f64::from(caret_y);
            if top < self.scroll_y {
                self.scroll_y = top;
            } else if top + self.line_height() as f64 > self.scroll_y + body_height as f64 {
                self.scroll_y = (top + self.line_height() as f64 - body_height as f64).max(0.0);
            }
            self.reveal_caret = false;
        }
        if self.layout_revision != Some(self.snapshot.revision.0)
            || self.layout_width != width.to_bits()
        {
            self.release_layouts(backend);
            self.layout_revision = Some(self.snapshot.revision.0);
            self.layout_width = width.to_bits();
        }
        let visible = visible_rows(
            self.scroll_y,
            body_height as f64,
            self.line_height() as f64,
            Some(self.visual_line(self.snapshot.line_count())),
            1,
        );
        self.visible_text = self
            .snapshot
            .line_range(self.logical_line(visible.start))
            .map_or(TextOffset(0), |r| r.start)
            ..self
                .snapshot
                .line_range(self.logical_line(visible.end.saturating_sub(1)))
                .map_or(TextOffset(self.snapshot.len()), |r| r.end);
        let visible_lines: std::collections::BTreeSet<_> = visible.clone().map(|row| self.logical_line(row)).filter(|line| *line < self.snapshot.line_count()).collect();
        self.virtual_lines.retain(|line,_|visible_lines.contains(line));
        if self.wrap_rows.len()>MAX_LAYOUTS {
            self.wrap_rows.retain(|line,_|visible_lines.contains(line));
        }
        let evicted: Vec<_> = self
            .layouts
            .keys()
            .copied()
            .filter(|n| !visible_lines.contains(n))
            .collect();
        for number in evicted {
            backend.release_layout(self.layouts.remove(&number).unwrap().id);
        }
        ops.push(DrawOp::PushClip(body));
        ops.push(DrawOp::Fill(body, self.theme.ui.editor));
        let mut caret_rect = None;
        for number in visible_lines {
            let row = self.visual_line(number);
            if self.hidden_lines.iter().any(|range| range.contains(&number)) { continue; }
            let y = self.top() + (row as f64 * self.line_height() as f64 - self.scroll_y) as f32;
            let range = self.content_range(number).unwrap();
            let long=range.end-range.start>MAX_LAYOUT_BYTES;
            let mut fragment=None;
            if long {
                if self.virtual_lines.get(&number).is_none_or(|state|!state.matches(&self.snapshot,&range)) {
                    self.virtual_lines.insert(number,virtual_layout::VirtualLine::new(&self.snapshot,range.clone()));
                }
                let target_row=(self.scroll_y/self.line_height() as f64).floor() as usize;
                let state=self.virtual_lines.get_mut(&number).unwrap();
                state.seek(self.scroll_x,target_row.saturating_sub(row),(number==caret_line&&reveal_requested).then_some(self.selection.caret),self.wrap);
                if !state.prepare(&self.snapshot,self.notify.clone()).map_err(|_|LayoutError::BackendFailure)? {
                    text(ops,LEFT,y,"Preparing line…",self.font_pixels,self.theme.gutter);
                    if reveal_requested {self.reveal_caret=true;}
                    continue;
                }
                let (start,x,rows)=state.origin();
                fragment=Some((start,state.end,state.text.as_ref().unwrap().clone(),x,rows));
            }
            let mut start = range.start;
            if !long && number == caret_line && self.selection.caret > start + MAX_LAYOUT_BYTES / 2 {
                start = self
                    .selection
                    .caret
                    .saturating_sub(MAX_LAYOUT_BYTES / 2)
                    .max(range.start);
                while !self.snapshot.is_boundary(TextOffset(start)) {
                    start += 1;
                }
            }
            let mut end = (start + MAX_LAYOUT_BYTES).min(range.end);
            while !self.snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let (x_origin,row_origin)=if let Some((a,b,_,x,rows))=&fragment {start=*a;end=*b;(*x,*rows)}else{(0.0,0)};
            let y=y+row_origin as f32*self.line_height();
            if self
                .layouts
                .get(&number)
                .is_some_and(|l| l.start != start || l.end != end)
            {
                backend.release_layout(self.layouts.remove(&number).unwrap().id);
            }
            if !self.layouts.contains_key(&number) {
                let value = if let Some((_,_,value,_,_))=&fragment {value.clone()}else{ self
                    .snapshot
                    .read(TextOffset(start)..TextOffset(end), MAX_LAYOUT_BYTES)
                    .map_err(|_| LayoutError::InvalidOffset)? };
                self.layouts.insert(
                    number,
                    LineLayout {
                        id: if self.wrap { backend.shape_wrapped(
                            &value, self.font_pixels, (width-LEFT-16.0).max(1.0), &self.font_family,
                        )? } else { backend.shape_with_font_family(
                            &value,
                            self.font_pixels,
                            (width - LEFT - 16.0).max(1.0),
                            &self.font_family,
                        )? },
                        start,
                        end,
                        x_origin,
                        row_origin,
                    },
                );
            }
            if self.wrap {
                let rows = (backend.layout_size(self.layouts[&number].id)?.1/self.line_height()).ceil().max(1.0) as usize;
                let total=row_origin.saturating_add(rows).saturating_add(usize::from(end<range.end));
                if self.wrap_rows.insert(number, total) != Some(total) { (self.notify)(); }
            }
            if long {
                let (width,height)=backend.layout_size(self.layouts[&number].id)?;
                let line_height=self.line_height();
                if self.virtual_lines.get_mut(&number).unwrap().measured(width,height,line_height,self.wrap) {if reveal_requested {self.reveal_caret=true;}(self.notify)();}
            }
            let layout = &self.layouts[&number];
            if number == caret_line && self.highlight_current_line {
                ops.push(DrawOp::Fill(
                    rect(49.0, y, width - 65.0, self.line_height()),
                    self.theme.current_line,
                ));
            }
            if self.line_numbers {
                text(
                    ops,
                    14.0,
                    y,
                    (number + 1).to_string(),
                    self.font_pixels,
                    self.theme.gutter,
                );
            }
            if self.known_folds.iter().any(|fold| fold.header == number) {
                text(ops, 40.0, y, if self.fold_state.collapsed.contains(&number) { "+" } else { "−" }, self.font_pixels, self.theme.gutter);
            }
            for (style,marked) in self.search_marks.iter() {
                let a=marked.start.0.max(start);let b=marked.end.0.min(end);
                if a<b {for r in backend.range_rects(layout.id,a-start..b-start)? {
                    ops.push(DrawOp::Fill(rect(LEFT+(x_origin-self.scroll_x) as f32+r.x,y+r.y,r.width,r.height),self.theme.marks[(style-1)as usize]));
                }}
            }
            for selection in self.selection_set().selections {
                let selected = selection.range();
                let a = selected.start.max(start);
                let b = selected.end.min(end);
                if a < b {
                    for r in backend.range_rects(layout.id, a - start..b - start)? {
                        ops.push(DrawOp::Fill(
                            rect(LEFT + (x_origin-self.scroll_x) as f32 + r.x, y + r.y, r.width, r.height),
                            self.theme.ui.selection,
                        ));
                    }
                }
            }
            let mut draw_id = layout.id;
            let mut caret_offset = self.selection.caret.saturating_sub(start);
            if number == caret_line
                && let Some((composition, cursor)) = &self.composition
            {
                let mut displayed = self
                    .snapshot
                    .read(TextOffset(start)..TextOffset(end), MAX_LAYOUT_BYTES)
                    .map_err(|_| LayoutError::InvalidOffset)?;
                if displayed.len() + composition.len() <= MAX_LAYOUT_BYTES
                    && caret_offset <= displayed.len()
                {
                    displayed.insert_str(caret_offset, composition);
                    if let Some(old) = self.composition_layout.take() {
                        backend.release_layout(old);
                    }
                    draw_id = if self.wrap { backend.shape_wrapped(
                        &displayed, self.font_pixels, (width-LEFT-16.0).max(1.0), &self.font_family,
                    )? } else { backend.shape_with_font_family(
                        &displayed,
                        self.font_pixels,
                        (width - LEFT - 16.0).max(1.0),
                        &self.font_family,
                    )? };
                    self.composition_layout = Some(draw_id);
                    for r in backend
                        .range_rects(draw_id, caret_offset..caret_offset + composition.len())?
                    {
                        ops.push(DrawOp::Line {
                            from: Point {
                                x: LEFT + (x_origin-self.scroll_x) as f32 + r.x,
                                y: y + r.y + r.height,
                            },
                            to: Point {
                                x: LEFT + (x_origin-self.scroll_x) as f32 + r.x + r.width,
                                y: y + r.y + r.height,
                            },
                            color: self.theme.ui.caret,
                            width: 1.0,
                        });
                    }
                    caret_offset += cursor.map_or(composition.len(), |(_, end)| end);
                }
            }
            // Apply colors to the existing shaped layout; geometry and UTF-8 hit
            // testing stay identical. Composition uses conservative plain text.
            let styles: Vec<_> = if draw_id == layout.id {
                syntax
                    .into_iter()
                    .flat_map(|result| {
                        let first = result
                            .spans
                            .partition_point(|span| span.range.end.0 <= start);
                        result.spans[first..]
                            .iter()
                            .take_while(|span| span.range.start.0 < end)
                    })
                    .map(|span| bareline_renderer::TextStyle {
                        bytes: span.range.start.0.max(start) - start
                            ..span.range.end.0.min(end) - start,
                        color: match span.kind {
                            bareline_syntax::StyleKind::Keyword => {
                                self.theme.keyword
                            }
                            bareline_syntax::StyleKind::String => {
                                self.theme.string
                            }
                            bareline_syntax::StyleKind::Number => {
                                self.theme.number
                            }
                            bareline_syntax::StyleKind::Comment => {
                                self.theme.comment
                            }
                            bareline_syntax::StyleKind::Operator => self.theme.operator,
                        },
                    })
                    .collect()
            } else {
                Vec::new()
            };
            backend.set_styles(draw_id, &styles)?;
            ops.push(DrawOp::Layout {
                origin: Point { x: LEFT + (x_origin-self.scroll_x) as f32, y },
                layout: draw_id,
                color: self.theme.ui.text,
            });
            if number == caret_line && (start..=end).contains(&self.selection.caret) {
                let r = backend.caret(draw_id, caret_offset)?;
                let caret = rect(LEFT + (x_origin-self.scroll_x) as f32 + r.x, y + r.y, r.width, r.height);
                if reveal_requested && !self.wrap {
                    let x=x_origin+f64::from(r.x);
                    let viewport=f64::from((width-LEFT-16.0).max(1.0));
                    let next=if x<self.scroll_x {x}else if x>self.scroll_x+viewport {(x-viewport+f64::from(self.font_pixels)).max(0.0)}else{self.scroll_x};
                    if next!=self.scroll_x {self.scroll_x=next;(self.notify)();}
                }
                if self.caret_visible() { ops.push(DrawOp::Fill(caret, self.theme.ui.caret)); }
                caret_rect = Some(caret);
            }
            if self.composition.is_none() && self.caret_visible() {
                for selection in self.selection_set().selections {
                    if selection == self.selection || !(start..=end).contains(&selection.caret) {
                        continue;
                    }
                    let r = backend.caret(draw_id, selection.caret - start)?;
                    ops.push(DrawOp::Fill(
                        rect(LEFT + (x_origin-self.scroll_x) as f32 + r.x, y + r.y, r.width, r.height),
                        self.theme.ui.caret,
                    ));
                }
            }
            if start > range.start || end < range.end {
                text(ops, width - 34.0, y, "…", 13.0, self.theme.gutter);
            }
        }
        self.resolve_visual_navigation(backend)?;
        ops.push(DrawOp::Fill(
            rect(48.0, self.top(), 1.0, body_height),
            self.theme.ui.border,
        ));
        Scrollbar {
            bounds: rect(width - 12.0, self.top(), 12.0, body_height),
            offset: self.scroll_y,
            viewport: body_height as f64,
            total: Some(self.visual_line(self.snapshot.line_count()) as f64 * self.line_height() as f64),
        }
        .paint_with_theme(self.theme.ui, ops);
        ops.push(DrawOp::PopClip);
        let status_y = height - STATUS_HEIGHT;
        ops.push(DrawOp::Fill(
            rect(0.0, status_y, width, STATUS_HEIGHT),
            self.theme.ui.chrome,
        ));
        ops.push(DrawOp::Fill(rect(0.0, status_y, width, 1.0), self.theme.ui.border));
        let line_start = self.snapshot.line_range(caret_line).unwrap().start.0;
        let column = self
            .snapshot
            .read(
                TextOffset(line_start)..TextOffset(self.selection.caret),
                MAX_LAYOUT_BYTES,
            )
            .map(|prefix| (prefix.graphemes(true).count() + 1).to_string())
            .unwrap_or_else(|_| "indexing…".into());
        let labels = [
            language.to_string(),
            if !self.snapshot.is_complete() {
                format!("{} B loaded · indexing…", self.snapshot.len())
            } else {
                format!(
                    "{} B · {} lines",
                    self.snapshot.len(),
                    self.snapshot.line_count()
                )
            },
            format!("Ln {}, Col {}", caret_line + 1, column),
            self.eol_status_label().into(),
            self.encoding_label.clone(),
            if self.read_only() { "RO" } else { "INS" }.into(),
        ];
        for (x, label) in [
            16.0,
            130.0,
            (width - 420.0).max(310.0),
            width - 240.0,
            width - 155.0,
            width - 50.0,
        ]
        .into_iter()
        .zip(labels)
        {
            text(ops, x, status_y + 4.0, label, 13.0, self.theme.gutter);
        }
        if let Some(error) = &self.error {
            text(
                ops,
                LEFT,
                height - STATUS_HEIGHT - 28.0,
                error,
                13.0,
                self.theme.ui.caret,
            );
        }
        if language != "Plain text"
            && !syntax.is_some_and(|result| {
                result.range.start <= self.visible_text.start
                    && result.range.end >= self.visible_text.end
            })
        {
            text(
                ops,
                (width - 185.0).max(LEFT),
                height - STATUS_HEIGHT - 28.0,
                if styling.unavailable {
                    "Styling unavailable"
                } else {
                    "Styling…"
                },
                13.0,
                self.theme.gutter,
            );
        }
        Ok(caret_rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, service::Scheduler};
    use bareline_renderer::{FrameStatus, RenderBackend};
    use bareline_renderer_recording::RecordingBackend;
    #[test]
    fn fold_mapping_and_pending_restore_are_view_local() {
        let document = Document::from_utf8("a\nb\nc\nd\ne\nf\n", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let mut view = EditorSurface::loading(document.snapshot(), Arc::new(|| {}));
        let folds = vec![bareline_syntax::folding::Fold { header: 0, end: 2, level: 1 }, bareline_syntax::folding::Fold { header: 3, end: 5, level: 1 }];
        view.restore_folds(&[0..3]);
        view.set_known_folds(folds, 1, false);
        assert_eq!(view.persisted_folds(), vec![0..3]);
        assert_eq!(view.logical_line(1), 3);
        assert_eq!(view.visual_line(3), 1);
        let mut clone = view.clone_view();
        clone.unfold_all();
        assert_eq!(clone.logical_line(1), 1);
        assert_eq!(view.logical_line(1), 3);
    }
    #[test]
    fn fold_anchors_follow_acknowledged_edit_and_undo() {
        let scheduler = Scheduler::new(1, 16).unwrap();
        let document = Document::from_utf8("{\nx\n}\n", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = document.snapshot();
        let mut view = EditorSurface::new(scheduler.document(document, 16), snapshot, Arc::new(|| {}));
        view.set_known_folds(vec![bareline_syntax::folding::Fold { header: 0, end: 2, level: 1 }], 1, false);
        let drain = |view: &mut EditorSurface| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while view.busy() { view.pump(); assert!(std::time::Instant::now() < deadline); std::thread::yield_now(); }
        };
        view.enqueue(Input::Insert("\n".into()));
        drain(&mut view);
        assert_eq!(view.persisted_folds(), vec![1..4]);
        assert!(view.known_folds.is_empty());
        view.enqueue(Input::Undo);
        drain(&mut view);
        assert_eq!(view.persisted_folds(), vec![0..3]);
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(view.persisted_folds(), vec![1..4]);
    }
    #[test]
    fn actual_input_pairs_and_backspace_are_one_undo_each() {
        let scheduler = Scheduler::new(1, 16).unwrap();
        let document = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = document.snapshot();
        let mut view = EditorSurface::new(scheduler.document(document, 16), snapshot, Arc::new(|| {}));
        view.language = bareline_syntax::Language::Rust;
        let drain = |view: &mut EditorSurface| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while view.busy() { view.pump(); assert!(std::time::Instant::now() < deadline); std::thread::yield_now(); }
        };
        view.enqueue(Input::Insert("(".into())); drain(&mut view);
        assert_eq!(view.snapshot.read(TextOffset(0)..TextOffset(2), 2).unwrap(), "()");
        assert_eq!(view.selection.caret, 1);
        view.enqueue(Input::Backspace); drain(&mut view);
        assert_eq!(view.snapshot.len(), 0);
        view.enqueue(Input::Undo); drain(&mut view);
        assert_eq!(view.snapshot.len(), 2);
        assert_eq!(view.selection.caret, 1);
    }
    #[test]
    fn syntax_styles_preserve_layout_geometry_and_reject_foreign_results() {
        let scheduler = Scheduler::new(1, 16).unwrap();
        let text = "let crab = \"🦀\";\n";
        let document =
            Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = document.snapshot();
        let mut view = EditorSurface::new(
            scheduler.document(document, 16),
            snapshot.clone(),
            Arc::new(|| {}),
        );
        let syntax = bareline_syntax::lex(
            snapshot,
            bareline_syntax::Language::Rust,
            TextOffset(0)..TextOffset(text.len()),
            None,
            &Default::default(),
        )
        .unwrap();
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        let plain_caret = view.draw(&mut backend, 1100.0, 700.0, &mut ops).unwrap();
        let ids: Vec<_> = view.layouts.values().map(|line| line.id).collect();
        ops.clear();
        let styled_caret = view
            .draw_styled(
                &mut backend,
                1100.0,
                700.0,
                &mut ops,
                SyntaxView {
                    result: Some(&syntax),
                    language: "Rust",
                    unavailable: false,
                },
            )
            .unwrap();
        assert_eq!(plain_caret, styled_caret);
        assert_eq!(
            ids,
            view.layouts
                .values()
                .map(|line| line.id)
                .collect::<Vec<_>>()
        );
        assert!(backend.styles.values().any(|styles| !styles.is_empty()));
        // Theme changes recolor resident shaped layouts without re-layout or
        // altering document geometry/identity.
        view.theme.keyword = bareline_renderer::Color(0x102030);
        view.theme.ui.editor = bareline_renderer::Color(0xFAF8F5);
        ops.clear();
        let themed_caret = view.draw_styled(&mut backend, 1100.0, 700.0, &mut ops,
            SyntaxView { result: Some(&syntax), language: "Rust", unavailable: false }).unwrap();
        assert_eq!(themed_caret, styled_caret);
        assert_eq!(ids, view.layouts.values().map(|line| line.id).collect::<Vec<_>>());
        assert!(backend.styles.values().flatten().any(|style| style.color == view.theme.keyword));
        assert!(ops.iter().any(|op| matches!(op, DrawOp::Fill(_, color) if *color == view.theme.ui.editor)));
        let foreign = Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot();
        let foreign_syntax = bareline_syntax::lex(
            foreign,
            bareline_syntax::Language::Rust,
            TextOffset(0)..TextOffset(text.len()),
            None,
            &Default::default(),
        )
        .unwrap();
        ops.clear();
        view.draw_styled(
            &mut backend,
            1100.0,
            700.0,
            &mut ops,
            SyntaxView {
                result: Some(&foreign_syntax),
                language: "Rust",
                unavailable: false,
            },
        )
        .unwrap();
        assert!(backend.styles.values().all(Vec::is_empty));
        assert!(bareline_renderer::balanced_clips(&ops));
    }
    #[test]
    fn power_carets_typing_bookmarks_and_group_undo_use_committed_snapshots() {
        let scheduler = Scheduler::new(2, 16).unwrap();
        let make = |text| {
            let document =
                Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
            let snapshot = document.snapshot();
            EditorSurface::new(scheduler.document(document, 16), snapshot, Arc::new(|| {}))
        };
        let drain = |view: &mut EditorSurface| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while view.busy() {
                view.pump();
                assert!(std::time::Instant::now() < until);
                std::thread::yield_now();
            }
        };
        let mut first = make("a\nb");
        first.execute_power("editor.caret.below").unwrap();
        assert_eq!(first.selection_set().selections.len(), 2);
        first.execute_power("editor.bookmark.toggle").unwrap();
        first.enqueue(Input::Insert("x".into()));
        drain(&mut first);
        assert_eq!(
            first
                .snapshot
                .read(TextOffset(0)..TextOffset(5), 5)
                .unwrap(),
            "xa\nxb"
        );
        assert!(first.bookmarks.anchors.contains(&4));
        first.enqueue(Input::Undo);
        drain(&mut first);
        assert_eq!(first.selection_set().selections.len(), 2);
        assert!(first.bookmarks.anchors.contains(&2));
        let mut second = make("z");
        let before1 = first.snapshot.clone();
        let before2 = second.snapshot.clone();
        let edit1 = power::replace(
            &before1,
            &Selection {
                anchor: 0,
                caret: 1,
            }
            .into(),
            "",
            power::Limits::default(),
        )
        .unwrap();
        let edit2 = power::replace(
            &before2,
            &Selection::default().into(),
            "a",
            power::Limits::default(),
        )
        .unwrap();
        let mut group = group_view::SurfaceGroup::apply(
            &scheduler,
            &mut [&mut first, &mut second],
            vec![(before1, edit1), (before2, edit2)],
        )
        .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let id = loop {
            if let Some(id) = group.pump(&mut [&mut first, &mut second]).unwrap() {
                break id;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        };
        assert_eq!(first.linked_undo_group(), Some(id));
        let mut undo =
            group_view::SurfaceGroup::undo(&scheduler, &mut [&mut first, &mut second], id).unwrap();
        loop {
            if undo.pump(&mut [&mut first, &mut second]).unwrap().is_some() {
                break;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(
            first
                .snapshot
                .read(TextOffset(0)..TextOffset(3), 3)
                .unwrap(),
            "a\nb"
        );
        assert_eq!(
            second
                .snapshot
                .read(TextOffset(0)..TextOffset(1), 1)
                .unwrap(),
            "z"
        );
    }
    #[test]
    fn queued_typing_ime_cancel_and_undo_use_worker_snapshots() {
        let scheduler = Scheduler::new(2, 16).unwrap();
        let doc = Document::from_utf8("", Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = doc.snapshot();
        let mut view = EditorSurface::new(scheduler.document(doc, 16), snapshot, Arc::new(|| {}));
        fn drain(view: &mut EditorSurface) {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while view.busy() {
                view.pump();
                assert!(std::time::Instant::now() < until);
                std::thread::yield_now();
            }
        }
        view.enqueue(Input::Insert("a👩🏽‍💻\nمرحبا".into()));
        view.enqueue(Input::Insert("!".into()));
        drain(&mut view);
        let state = view.snapshot.content_state;
        view.preedit("未確定".into(), None);
        view.cancel_composition();
        assert_eq!(view.snapshot.content_state, state);
        view.enqueue(Input::Undo);
        drain(&mut view);
        view.enqueue(Input::Redo);
        drain(&mut view);
        assert_eq!(
            view.snapshot
                .read(TextOffset(0)..TextOffset(view.snapshot.len()), 1024)
                .unwrap(),
            "a👩🏽‍💻\nمرحبا!"
        );
        let mut backend = RecordingBackend::default();
        let mut ops = Vec::new();
        assert!(
            view.draw(&mut backend, 1200.0, 800.0, &mut ops)
                .unwrap()
                .is_some()
        );
        assert_eq!(backend.render(&ops).unwrap(), FrameStatus::Presented);
    }
}
