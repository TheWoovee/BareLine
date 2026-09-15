// SPDX-License-Identifier: MPL-2.0
//! Two-pane projection of PR-004 session tabs. Document bytes are never copied.
pub use bareline_document::DocumentSnapshot as ViewSnapshot;
pub use bareline_editor_surface::EditorSurface as SharedEditorView;
pub use bareline_editor_surface::SyntaxView as SharedSyntaxView;
pub use bareline_file_io::session::{SessionLayout, SessionManifest, SessionTab, SplitOrientation, ViewState};
use bareline_renderer::Rect;
use bareline_ui::rect;
use std::ops::Range;

/// Maximum persisted tab id accepted by native view/provider projections.
/// Each id owns a collision-free 256 KiB semantic/text-run block.
pub const MAX_VIEW_TAB_ID: u64 = (1 << 44) - 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Vertical,
    Horizontal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    Missing,
    InvalidPane,
    InvalidState,
    IdentityExhausted,
    LastDirtyReference,
    PinnedBoundary,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosedView {
    pub document_id: u64,
    pub last_reference: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPosition {
    pub line: u64,
    pub fraction: f64,
    pub x: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollUpdate {
    pub origin: u64,
    pub pane: u32,
    pub position: ScrollPosition,
}
pub struct PaneGeometry {
    pub panes: [Option<Rect>; 2],
    pub splitter: Option<Rect>,
}

#[derive(Clone)]
pub struct ViewController {
    pub tab_colors: std::collections::BTreeMap<u64, u32>,
    pub vertical_tabs: bool,
    pub tab_sort: String,
    tabs: Vec<SessionTab>,
    pub orientation: Orientation,
    pub ratio: f64,
    pub split: bool,
    pub sync_vertical: bool,
    pub sync_horizontal: bool,
    active_pane: u32,
    active: [Option<u64>; 2],
    next_id: u64,
    mru: Vec<u64>,
    scroll: [ScrollPosition; 2],
    last_origin: [u64; 2],
    next_origin: u64,
}
impl ViewController {
    pub fn from_session(manifest: &SessionManifest) -> Result<Self, ViewError> {
        manifest.validate().map_err(|_| ViewError::InvalidState)?;
        let mut controller = Self::new(manifest.tabs.clone(), manifest.active_tab)?;
        controller.orientation = match manifest.layout.orientation {
            SplitOrientation::Vertical => Orientation::Vertical,
            SplitOrientation::Horizontal => Orientation::Horizontal,
        };
        controller.ratio = f64::from_bits(manifest.layout.ratio_bits);
        controller.split = manifest.layout.split;
        controller.active_pane = manifest.layout.active_pane;
        controller.active = manifest.layout.active_tabs;
        controller.sync_horizontal = manifest.layout.sync_horizontal;
        controller.tab_colors = manifest
            .layout
            .tab_colors
            .iter()
            .filter(|(id, color)| manifest.tabs.iter().any(|tab| tab.id == **id) && **color <= 0xffffff)
            .map(|(id, color)| (*id, *color))
            .collect();
        controller.vertical_tabs = manifest.layout.vertical_tabs;
        controller.tab_sort = match manifest.layout.tab_sort.as_str() {
            "name" | "name_descending" | "path" => manifest.layout.tab_sort.clone(),
            _ => "manual".into(),
        };
        controller.sync_vertical = manifest.layout.sync_vertical;
        controller.repair_active();
        controller.mru.clear();
        for document in &manifest.mru {
            controller.mru.extend(
                controller
                    .tabs
                    .iter()
                    .filter(|t| t.document_id == *document)
                    .map(|t| t.id),
            );
        }
        if let Some(id) = manifest.active_tab {
            controller.activate(id)?;
        }
        Ok(controller)
    }
    pub fn new(tabs: Vec<SessionTab>, active: Option<u64>) -> Result<Self, ViewError> {
        if tabs.len() > 10_000 {
            return Err(ViewError::InvalidState);
        }
        let mut ids = std::collections::HashSet::new();
        let mut unpinned = false;
        for tab in &tabs {
            if tab.id > MAX_VIEW_TAB_ID
                || !ids.insert(tab.id)
                || tab.view.split > 1
                || tab.view.folds.len() > 100_000
                || tab.view.folds.iter().any(|r| r.start >= r.end)
                || !f64::from_bits(tab.view.scroll_y_bits).is_finite()
                || f64::from_bits(tab.view.scroll_y_bits) < 0.0
            {
                return Err(ViewError::InvalidState);
            }
            if tab.pinned && unpinned {
                return Err(ViewError::PinnedBoundary);
            }
            unpinned |= !tab.pinned;
        }
        let next_id = tabs
            .iter()
            .map(|t| t.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ViewError::IdentityExhausted)?;
        let split = tabs.iter().any(|t| t.view.split == 1);
        let mut controller = Self {
            tab_colors: Default::default(),
            vertical_tabs: false,
            tab_sort: "manual".into(),
            tabs,
            orientation: Orientation::Vertical,
            ratio: 0.5,
            split,
            sync_vertical: false,
            sync_horizontal: false,
            active_pane: 0,
            active: [None, None],
            next_id,
            mru: Vec::new(),
            scroll: [ScrollPosition::default(); 2],
            last_origin: [0, 0],
            next_origin: 1,
        };
        controller.repair_active();
        if let Some(id) = active {
            controller.activate(id)?;
        }
        Ok(controller)
    }
    pub fn tabs(&self) -> &[SessionTab] {
        &self.tabs
    }
    pub fn retain_documents(&mut self, live: &[u64]) {
        self.tabs.retain(|tab| live.contains(&tab.document_id));
        self.mru.retain(|id| self.tabs.iter().any(|tab| tab.id == *id));
        self.tab_colors
            .retain(|id, _| self.tabs.iter().any(|tab| tab.id == *id));
        self.repair_active();
    }
    pub fn pin(&mut self, id: u64, pinned: bool) -> Result<(), ViewError> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(ViewError::Missing)?
            .pinned = pinned;
        self.tabs.sort_by_key(|tab| !tab.pinned);
        Ok(())
    }
    pub fn color(&mut self, id: u64, color: Option<u32>) -> Result<(), ViewError> {
        if self.tab(id).is_none() || color.is_some_and(|color| color > 0xffffff) {
            return Err(ViewError::InvalidState);
        }
        if let Some(color) = color {
            self.tab_colors.insert(id, color);
        } else {
            self.tab_colors.remove(&id);
        }
        Ok(())
    }
    pub fn sort_by_label(&mut self, labels: &[(u64, String)], descending: bool) {
        let labels: std::collections::HashMap<_, _> =
            labels.iter().map(|(id, label)| (*id, label.to_lowercase())).collect();
        self.tabs.sort_by(|a, b| {
            (!a.pinned).cmp(&(!b.pinned)).then_with(|| {
                let a = labels.get(&a.document_id).map(String::as_str).unwrap_or("");
                let b = labels.get(&b.document_id).map(String::as_str).unwrap_or("");
                if descending { b.cmp(a) } else { a.cmp(b) }
            })
        });
        self.tab_sort = if descending { "name_descending" } else { "name" }.into();
    }
    pub fn move_to_pane(&mut self, id: u64, pane: u32, before: Option<u64>) -> Result<(), ViewError> {
        if pane > 1 {
            return Err(ViewError::InvalidPane);
        }
        let source = self.tab(id).ok_or(ViewError::Missing)?;
        if let Some(target) = before.and_then(|id| self.tab(id)) {
            if source.pinned != target.pinned {
                return Err(ViewError::PinnedBoundary);
            }
            if target.view.split != pane {
                return Err(ViewError::InvalidPane);
            }
        }
        let old = source.view.split;
        self.tabs.iter_mut().find(|tab| tab.id == id).unwrap().view.split = pane;
        if let Err(error) = self.reorder(id, before) {
            self.tabs.iter_mut().find(|tab| tab.id == id).unwrap().view.split = old;
            return Err(error);
        }
        self.split = self.tabs.iter().any(|tab| tab.view.split == 1);
        self.repair_active();
        self.activate(id)
    }
    /// Attach a newly opened document identity to the active pane. The owner has
    /// already created the document service; this allocates view metadata only.
    /// Restore a previously closed view in this process without creating a second tab model.
    pub fn restore_tab(&mut self, tab: SessionTab, position: usize, color: Option<u32>) -> Result<(), ViewError> {
        if self.tabs.len() >= 10_000 || tab.view.split > 1 || self.tabs.iter().any(|existing| existing.id == tab.id) {
            return Err(ViewError::InvalidState);
        }
        let mut probe = tab.clone();
        probe.view.split = 0;
        Self::new(vec![probe], None)?;
        if color.is_some_and(|color| color > 0xffffff) {
            return Err(ViewError::InvalidState);
        }
        let next = tab.id.checked_add(1).ok_or(ViewError::IdentityExhausted)?;
        let id = tab.id;
        self.split |= tab.view.split == 1;
        self.tabs.insert(position.min(self.tabs.len()), tab);
        self.tabs.sort_by_key(|tab| !tab.pinned);
        self.next_id = self.next_id.max(next);
        if let Some(color) = color {
            self.tab_colors.insert(id, color);
        }
        self.repair_active();
        self.activate(id)
    }
    pub fn add_document(&mut self, document_id: u64) -> Result<u64, ViewError> {
        if self.tabs.len() >= 10_000 {
            return Err(ViewError::InvalidState);
        }
        let id = self.next_id;
        if id > MAX_VIEW_TAB_ID {
            return Err(ViewError::IdentityExhausted);
        }
        self.next_id = id.checked_add(1).ok_or(ViewError::IdentityExhausted)?;
        self.tabs.push(SessionTab {
            id,
            document_id,
            pinned: false,
            view: ViewState {
                split: self.active_pane,
                ..Default::default()
            },
        });
        self.activate(id)?;
        Ok(id)
    }
    pub fn tab(&self, id: u64) -> Option<&SessionTab> {
        self.tabs.iter().find(|t| t.id == id)
    }
    pub fn assign_document(&mut self, id: u64, document_id: u64) -> Result<(), ViewError> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or(ViewError::Missing)?
            .document_id = document_id;
        Ok(())
    }
    pub fn active_pane(&self) -> u32 {
        self.active_pane
    }
    pub fn active_tab(&self, pane: u32) -> Option<u64> {
        self.active.get(pane as usize).copied().flatten()
    }
    pub fn pane_tabs(&self, pane: u32) -> impl Iterator<Item = &SessionTab> {
        self.tabs.iter().filter(move |t| t.view.split == pane)
    }
    fn repair_active(&mut self) {
        for pane in 0..2 {
            if !self
                .tabs
                .iter()
                .any(|t| Some(t.id) == self.active[pane] && t.view.split == pane as u32)
            {
                self.active[pane] = self.tabs.iter().find(|t| t.view.split == pane as u32).map(|t| t.id);
            }
        }
        if self.active[self.active_pane as usize].is_none() {
            self.active_pane = if self.active[0].is_some() { 0 } else { 1 };
        }
        if self.active == [None, None] {
            self.active_pane = 0;
        }
    }
    pub fn activate(&mut self, id: u64) -> Result<(), ViewError> {
        let pane = self.tab(id).ok_or(ViewError::Missing)?.view.split;
        self.active[pane as usize] = Some(id);
        self.active_pane = pane;
        self.mru.retain(|seen| *seen != id);
        self.mru.insert(0, id);
        Ok(())
    }
    pub fn mru(&self) -> impl Iterator<Item = u64> + '_ {
        let mut seen = std::collections::HashSet::new();
        self.mru
            .iter()
            .copied()
            .chain(self.tabs.iter().map(|t| t.id))
            .filter(move |id| seen.insert(*id))
    }
    pub fn clone_to_other(&mut self, id: u64) -> Result<u64, ViewError> {
        if self.tabs.len() >= 10_000 {
            return Err(ViewError::InvalidState);
        }
        let mut tab = self.tab(id).ok_or(ViewError::Missing)?.clone();
        let new_id = self.next_id;
        if new_id > MAX_VIEW_TAB_ID {
            return Err(ViewError::IdentityExhausted);
        }
        self.next_id = new_id.checked_add(1).ok_or(ViewError::IdentityExhausted)?;
        tab.id = new_id;
        tab.view.split = 1 - tab.view.split;
        let insertion = if tab.pinned {
            self.tabs.iter().take_while(|t| t.pinned).count()
        } else {
            self.tabs.len()
        };
        self.tabs.insert(insertion, tab);
        if let Some(color) = self.tab_colors.get(&id).copied() {
            self.tab_colors.insert(new_id, color);
        }
        self.split = true;
        self.activate(new_id)?;
        Ok(new_id)
    }
    pub fn move_to_other(&mut self, id: u64) -> Result<(), ViewError> {
        let tab = self.tabs.iter_mut().find(|t| t.id == id).ok_or(ViewError::Missing)?;
        tab.view.split = 1 - tab.view.split;
        self.split = true;
        self.repair_active();
        self.activate(id)
    }
    pub fn close(&mut self, id: u64, dirty: bool, discard_last: bool) -> Result<ClosedView, ViewError> {
        let tab = self.tab(id).ok_or(ViewError::Missing)?;
        let document_id = tab.document_id;
        let last_reference = self.tabs.iter().filter(|t| t.document_id == document_id).count() == 1;
        if last_reference && dirty && !discard_last {
            return Err(ViewError::LastDirtyReference);
        }
        self.tabs.retain(|t| t.id != id);
        self.tab_colors.remove(&id);
        self.mru.retain(|seen| *seen != id);
        self.repair_active();
        if self.active[0].is_none() || self.active[1].is_none() {
            self.collapse();
        }
        Ok(ClosedView {
            document_id,
            last_reference,
        })
    }
    pub fn collapse(&mut self) {
        let active = self.active[self.active_pane as usize]
            .or(self.active[0])
            .or(self.active[1]);
        for tab in &mut self.tabs {
            tab.view.split = 0;
        }
        self.active = [active, None];
        self.active_pane = 0;
        self.split = false;
        self.repair_active();
    }
    pub fn set_view_state(&mut self, id: u64, mut state: ViewState) -> Result<(), ViewError> {
        let tab = self.tabs.iter_mut().find(|t| t.id == id).ok_or(ViewError::Missing)?;
        let y = f64::from_bits(state.scroll_y_bits);
        if !y.is_finite() || y < 0.0 || state.folds.len() > 100_000 || state.folds.iter().any(|r| r.start >= r.end) {
            return Err(ViewError::InvalidState);
        }
        state.split = tab.view.split;
        tab.view = state;
        Ok(())
    }
    pub fn folds(&self, id: u64) -> &[Range<u64>] {
        self.tab(id).map_or(&[], |tab| tab.view.folds.as_slice())
    }
    pub fn set_folds(&mut self, id: u64, folds: Vec<Range<u64>>) -> Result<(), ViewError> {
        if self.tab(id).is_none() {
            return Err(ViewError::Missing);
        }
        if folds.len() > 100_000 || folds.iter().any(|r| r.start >= r.end) {
            return Err(ViewError::InvalidState);
        }
        self.tabs.iter_mut().find(|t| t.id == id).unwrap().view.folds = folds;
        Ok(())
    }
    /// Reorder within one pane/pin region. `None` moves to that region's end.
    pub fn reorder(&mut self, id: u64, before: Option<u64>) -> Result<(), ViewError> {
        if before == Some(id) {
            return Ok(());
        }
        let source = self.tab(id).ok_or(ViewError::Missing)?;
        let pane = source.view.split;
        let pinned = source.pinned;
        if let Some(target) = before {
            let target = self.tab(target).ok_or(ViewError::Missing)?;
            if target.pinned != pinned {
                return Err(ViewError::PinnedBoundary);
            }
            if target.view.split != pane {
                return Err(ViewError::InvalidPane);
            }
        }
        let index = self.tabs.iter().position(|t| t.id == id).unwrap();
        let tab = self.tabs.remove(index);
        let insertion = before
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .unwrap_or_else(|| {
                self.tabs
                    .iter()
                    .rposition(|t| t.pinned == pinned && t.view.split == pane)
                    .map_or_else(|| if pinned { 0 } else { self.tabs.len() }, |i| i + 1)
            });
        self.tabs.insert(insertion, tab);
        Ok(())
    }
    pub fn keyboard_reorder(&mut self, id: u64, backwards: bool) -> Result<(), ViewError> {
        let tab = self.tab(id).ok_or(ViewError::Missing)?;
        let peers: Vec<_> = self
            .tabs
            .iter()
            .filter(|t| t.view.split == tab.view.split && t.pinned == tab.pinned)
            .map(|t| t.id)
            .collect();
        let index = peers.iter().position(|seen| *seen == id).unwrap();
        if backwards && index > 0 {
            self.reorder(id, Some(peers[index - 1]))
        } else if !backwards && index + 1 < peers.len() {
            self.reorder(id, peers.get(index + 2).copied())
        } else {
            Ok(())
        }
    }
    pub fn geometry(&self, bounds: Rect) -> PaneGeometry {
        if !self.split {
            return PaneGeometry {
                panes: [Some(bounds), None],
                splitter: None,
            };
        }
        let ratio = if self.ratio.is_finite() {
            self.ratio.clamp(0.1, 0.9) as f32
        } else {
            0.5
        };
        let gap = 6.0f32.min(if self.orientation == Orientation::Vertical {
            bounds.width
        } else {
            bounds.height
        });
        if self.orientation == Orientation::Vertical {
            let first = (bounds.width - gap).max(0.0) * ratio;
            PaneGeometry {
                panes: [
                    Some(rect(bounds.x, bounds.y, first, bounds.height)),
                    Some(rect(
                        bounds.x + first + gap,
                        bounds.y,
                        (bounds.width - first - gap).max(0.0),
                        bounds.height,
                    )),
                ],
                splitter: Some(rect(bounds.x + first, bounds.y, gap, bounds.height)),
            }
        } else {
            let first = (bounds.height - gap).max(0.0) * ratio;
            PaneGeometry {
                panes: [
                    Some(rect(bounds.x, bounds.y, bounds.width, first)),
                    Some(rect(
                        bounds.x,
                        bounds.y + first + gap,
                        bounds.width,
                        (bounds.height - first - gap).max(0.0),
                    )),
                ],
                splitter: Some(rect(bounds.x, bounds.y + first, bounds.width, gap)),
            }
        }
    }
    pub fn begin_scroll(
        &mut self,
        pane: u32,
        position: ScrollPosition,
        alignment: Option<&AlignmentMap>,
    ) -> Result<Option<ScrollUpdate>, ViewError> {
        let origin = self.next_origin;
        self.next_origin = self.next_origin.checked_add(1).ok_or(ViewError::IdentityExhausted)?;
        self.receive_scroll(ScrollUpdate { origin, pane, position }, alignment)
    }
    /// Replaying the returned update never emits an update back to its origin.
    pub fn receive_scroll(
        &mut self,
        update: ScrollUpdate,
        alignment: Option<&AlignmentMap>,
    ) -> Result<Option<ScrollUpdate>, ViewError> {
        if update.pane > 1 {
            return Err(ViewError::InvalidPane);
        }
        if !update.position.fraction.is_finite() || !update.position.x.is_finite() || update.position.x < 0.0 {
            return Err(ViewError::InvalidState);
        }
        let source = update.pane as usize;
        if update.origin <= self.last_origin[source] {
            return Ok(None);
        }
        self.next_origin = self
            .next_origin
            .max(update.origin.checked_add(1).ok_or(ViewError::IdentityExhausted)?);
        self.last_origin[source] = update.origin;
        self.scroll[source] = update.position;
        if !self.split || (!self.sync_vertical && !self.sync_horizontal) {
            return Ok(None);
        }
        let target = 1 - source;
        let mut position = self.scroll[target];
        if self.sync_vertical {
            position.line = alignment.map_or(update.position.line, |map| map.map_scroll(source, update.position.line));
            position.fraction = update.position.fraction.clamp(0.0, 1.0);
        }
        if self.sync_horizontal {
            position.x = update.position.x;
        }
        self.scroll[target] = position;
        self.last_origin[target] = update.origin;
        Ok(Some(ScrollUpdate {
            origin: update.origin,
            pane: target as u32,
            position,
        }))
    }
    pub fn write_tabs(&self, manifest: &mut SessionManifest) {
        manifest.tabs = self.tabs.clone();
        manifest.active_tab = self.active[self.active_pane as usize];
    }
    pub fn write_session(&self, manifest: &mut SessionManifest) {
        self.write_tabs(manifest);
        let bottom_panel = manifest.layout.bottom_panel.clone();
        let bottom_panel_collapsed = manifest.layout.bottom_panel_collapsed;
        let bottom_panel_height_bits = manifest.layout.bottom_panel_height_bits;
        manifest.layout = SessionLayout {
            tab_colors: self.tab_colors.clone(),
            vertical_tabs: self.vertical_tabs,
            tab_sort: self.tab_sort.clone(),
            split: self.split,
            orientation: match self.orientation {
                Orientation::Vertical => SplitOrientation::Vertical,
                Orientation::Horizontal => SplitOrientation::Horizontal,
            },
            ratio_bits: if self.ratio.is_finite() {
                self.ratio.clamp(0.1, 0.9)
            } else {
                0.5
            }
            .to_bits(),
            active_pane: self.active_pane,
            active_tabs: self.active,
            sync_horizontal: self.sync_horizontal,
            sync_vertical: self.sync_vertical,
            bottom_panel,
            bottom_panel_collapsed,
            bottom_panel_height_bits,
        };
        let mut seen = std::collections::HashSet::new();
        manifest.mru = self
            .mru()
            .filter_map(|id| self.tab(id).map(|t| t.document_id))
            .filter(|doc| seen.insert(*doc))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_file_io::session::{SessionDocument, decode, encode};
    fn tab(id: u64, pinned: bool) -> SessionTab {
        SessionTab {
            id,
            document_id: id,
            pinned,
            view: ViewState::default(),
        }
    }
    #[test]
    fn clones_share_document_identity_with_independent_state_and_last_reference_close_guard() {
        let mut views = ViewController::new(vec![tab(1, true), tab(2, false)], Some(1)).unwrap();
        views.set_folds(1, vec![3..9]).unwrap();
        let clone = views.clone_to_other(1).unwrap();
        assert_eq!(views.tab(clone).unwrap().document_id, 1);
        assert_eq!(views.folds(clone), [3..9]);
        views.set_folds(clone, vec![12..18]).unwrap();
        views
            .set_view_state(
                clone,
                ViewState {
                    caret: 50,
                    anchor: 30,
                    scroll_y_bits: 600.0f64.to_bits(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(views.tab(1).unwrap().view.caret, 0);
        assert_eq!(views.folds(1), [3..9]);
        assert_eq!(views.tab(clone).unwrap().view.split, 1);
        assert_eq!(
            views.close(clone, true, false).unwrap(),
            ClosedView {
                document_id: 1,
                last_reference: false
            }
        );
        assert_eq!(views.close(1, true, false), Err(ViewError::LastDirtyReference));
        assert!(views.tab(1).is_some());
        assert!(!views.split);
    }

    #[test]
    fn restored_tab_ids_must_fit_the_native_provider_block_contract() {
        let mut valid = tab(MAX_VIEW_TAB_ID, false);
        valid.document_id = 1;
        assert!(ViewController::new(vec![valid], Some(MAX_VIEW_TAB_ID)).is_ok());
        let mut invalid = tab(MAX_VIEW_TAB_ID + 1, false);
        invalid.document_id = 1;
        assert!(matches!(
            ViewController::new(vec![invalid], None),
            Err(ViewError::InvalidState)
        ));
    }
    #[test]
    fn five_hundred_tabs_pin_reorder_and_layout_roundtrip() {
        let tabs: Vec<_> = (1..=500).map(|id| tab(id, id <= 10)).collect();
        let mut views = ViewController::new(tabs, Some(1)).unwrap();
        assert_eq!(views.reorder(1, Some(11)), Err(ViewError::PinnedBoundary));
        views.keyboard_reorder(1, false).unwrap();
        assert_eq!(views.tabs()[0].id, 2);
        views.move_to_other(1).unwrap();
        views.set_folds(1, vec![5..12]).unwrap();
        views.orientation = Orientation::Horizontal;
        views.ratio = 0.6;
        views.sync_vertical = true;
        views.sync_horizontal = true;
        views.vertical_tabs = true;
        views.color(1, Some(0x36c9c6)).unwrap();
        views.color(500, Some(0xc678dd)).unwrap();
        views.sort_by_label(
            &(1..=500)
                .map(|id| (id, format!("Document {id:03}")))
                .collect::<Vec<_>>(),
            true,
        );
        views.activate(499).unwrap();
        views.activate(1).unwrap();
        let mut manifest = SessionManifest {
            documents: (1..=500)
                .map(|id| SessionDocument {
                    id,
                    path: None,
                    title: format!("Document {id}"),
                })
                .collect(),
            ..Default::default()
        };
        manifest.layout.bottom_panel = Some("output".into());
        manifest.layout.bottom_panel_collapsed = false;
        manifest.layout.bottom_panel_height_bits = 344.0f32.to_bits();
        views.write_session(&mut manifest);
        assert_eq!(manifest.layout.bottom_panel.as_deref(), Some("output"));
        assert!(!manifest.layout.bottom_panel_collapsed);
        assert_eq!(f32::from_bits(manifest.layout.bottom_panel_height_bits), 344.0);
        let restored = ViewController::from_session(&decode(&encode(&manifest).unwrap()).unwrap()).unwrap();
        assert_eq!(restored.tabs(), views.tabs());
        assert_eq!(restored.active_tab(1), Some(1));
        assert_eq!(restored.folds(1), [5..12]);
        assert_eq!(restored.orientation, Orientation::Horizontal);
        assert_eq!(restored.ratio, 0.6);
        assert!(restored.sync_vertical);
        assert!(restored.sync_horizontal);
        assert!(restored.vertical_tabs);
        assert_eq!(restored.tab_colors, views.tab_colors);
        assert_eq!(restored.tab_sort, "name_descending");
        assert_eq!(restored.mru().collect::<Vec<_>>(), views.mru().collect::<Vec<_>>());
        let geometry = restored.geometry(rect(10.0, 20.0, 900.0, 700.0));
        let first = geometry.panes[0].unwrap();
        let second = geometry.panes[1].unwrap();
        let splitter = geometry.splitter.unwrap();
        assert_eq!(first.height + splitter.height + second.height, 700.0);
        assert_eq!(second.y, splitter.y + splitter.height);
        assert_eq!(restored.mru().count(), 500);
    }
    #[test]
    fn cross_pane_drag_is_atomic_at_pin_boundary_and_closed_metadata_restores() {
        let mut views = ViewController::new(vec![tab(1, true), tab(2, false)], Some(1)).unwrap();
        let clone = views.clone_to_other(2).unwrap();
        let before = views.tabs().to_vec();
        assert_eq!(views.move_to_pane(1, 1, Some(clone)), Err(ViewError::PinnedBoundary));
        assert_eq!(views.tabs(), before);
        views.color(1, Some(0x123456)).unwrap();
        let saved = views.tab(1).unwrap().clone();
        views.close(1, false, false).unwrap();
        views.restore_tab(saved.clone(), 0, Some(0x123456)).unwrap();
        assert_eq!(views.tabs()[0], saved);
        assert_eq!(views.tab_colors.get(&1), Some(&0x123456));
        assert_eq!(views.restore_tab(saved, 0, None), Err(ViewError::InvalidState));
        assert_eq!(views.color(1, Some(0x1000000)), Err(ViewError::InvalidState));
        assert_eq!(views.tab_colors.get(&1), Some(&0x123456));
        views.move_to_pane(2, 1, Some(clone)).unwrap();
        assert_eq!(views.tab(2).unwrap().view.split, 1);
    }
    #[test]
    fn alignment_spacers_have_no_line_number_and_sync_tokens_do_not_echo() {
        let map = AlignmentMap::new(vec![
            AlignmentBlock {
                left: 3..5,
                right: 3..7,
            },
            AlignmentBlock {
                left: 8..11,
                right: 10..11,
            },
        ])
        .unwrap();
        assert_eq!(map.spacers_in_window(0, 4, 4), vec![(1, 2)]);
        assert_eq!(map.spacers_in_window(0, 6, 4), Vec::<(u64, u64)>::new());
        let adjacent = AlignmentMap::new(vec![
            AlignmentBlock {
                left: 0..0,
                right: 0..2,
            },
            AlignmentBlock {
                left: 0..0,
                right: 2..3,
            },
        ])
        .unwrap();
        assert_eq!(adjacent.spacers(0), vec![(0, 3)]);
        assert_eq!(map.document_line(0, 5), None);
        assert_eq!(map.document_line(0, 6), None);
        assert_eq!(map.document_line(0, 7), Some(5));
        assert_eq!(map.document_line(1, 11), None);
        for side in [0, 1] {
            for line in 0..30 {
                assert_eq!(map.document_line(side, map.view_row(side, line)), Some(line));
            }
        }
        let mut views = ViewController::new(vec![tab(1, false)], Some(1)).unwrap();
        views.clone_to_other(1).unwrap();
        views.sync_vertical = true;
        views.sync_horizontal = true;
        let update = views
            .begin_scroll(
                1,
                ScrollPosition {
                    line: 6,
                    fraction: 0.25,
                    x: 48.0,
                },
                Some(&map),
            )
            .unwrap()
            .unwrap();
        assert_eq!(update.pane, 0);
        assert_eq!(update.position.line, 5);
        assert_eq!(update.position.x, 48.0);
        assert_eq!(views.receive_scroll(update, Some(&map)).unwrap(), None);
        let next = views
            .begin_scroll(
                0,
                ScrollPosition {
                    line: 12,
                    fraction: 0.5,
                    x: 24.0,
                },
                Some(&map),
            )
            .unwrap()
            .unwrap();
        assert!(next.origin > update.origin);
        assert_eq!(views.receive_scroll(next, Some(&map)).unwrap(), None);
        assert!(
            AlignmentMap::new(vec![AlignmentBlock {
                left: 4..5,
                right: 3..4
            }])
            .is_err()
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlignmentBlock {
    pub left: Range<u64>,
    pub right: Range<u64>,
}
#[derive(Clone)]
struct IndexedBlock {
    block: AlignmentBlock,
    row: u64,
    height: u64,
}
#[derive(Clone)]
pub struct AlignmentMap {
    blocks: Vec<IndexedBlock>,
}
impl AlignmentMap {
    /// Insert view-only rows before each logical line; document offsets are unchanged.
    pub fn spacers(&self, side: usize) -> Vec<(u64, u64)> {
        self.spacers_in_window(side, 0, u64::MAX)
    }
    /// Project global alignment rows into a paged viewport's logical line domain.
    pub fn spacers_in_window(&self, side: usize, first_line: u64, line_count: u64) -> Vec<(u64, u64)> {
        let end = first_line.saturating_add(line_count);
        let mut rows: Vec<(u64, u64)> = Vec::new();
        for entry in &self.blocks {
            let range = if side == 0 {
                &entry.block.left
            } else {
                &entry.block.right
            };
            let count = entry.height - (range.end - range.start);
            if count == 0 || range.end < first_line || range.end > end {
                continue;
            }
            let line = range.end - first_line;
            if let Some((previous, total)) = rows.last_mut().filter(|(previous, _)| *previous == line) {
                let _ = previous;
                *total = total.saturating_add(count);
            } else {
                rows.push((line, count));
            }
        }
        rows
    }
    pub fn new(blocks: Vec<AlignmentBlock>) -> Result<Self, ViewError> {
        if blocks.len() > 100_000 {
            return Err(ViewError::InvalidState);
        }
        let mut indexed = Vec::with_capacity(blocks.len());
        let mut end = [0, 0];
        let mut row = 0u64;
        for block in blocks {
            if block.left.start > block.left.end
                || block.right.start > block.right.end
                || block.left.start < end[0]
                || block.right.start < end[1]
                || block.left.start - end[0] != block.right.start - end[1]
            {
                return Err(ViewError::InvalidState);
            }
            row = row
                .checked_add(block.left.start - end[0])
                .ok_or(ViewError::InvalidState)?;
            let height = (block.left.end - block.left.start).max(block.right.end - block.right.start);
            end = [block.left.end, block.right.end];
            let next = row.checked_add(height).ok_or(ViewError::InvalidState)?;
            indexed.push(IndexedBlock { block, row, height });
            row = next;
        }
        Ok(Self { blocks: indexed })
    }
    pub fn view_row(&self, side: usize, line: u64) -> u64 {
        let mut extra = 0u64;
        for entry in &self.blocks {
            let range = if side == 0 {
                &entry.block.left
            } else {
                &entry.block.right
            };
            if line < range.start {
                break;
            }
            if line < range.end {
                return entry.row.saturating_add(line - range.start);
            }
            extra = extra.saturating_add(entry.height - (range.end - range.start));
        }
        line.saturating_add(extra)
    }
    /// None denotes a view-only spacer; never attach a document line number to it.
    pub fn document_line(&self, side: usize, row: u64) -> Option<u64> {
        let mut extra = 0u64;
        for entry in &self.blocks {
            let range = if side == 0 {
                &entry.block.left
            } else {
                &entry.block.right
            };
            if row < entry.row {
                break;
            }
            if row < entry.row.saturating_add(entry.height) {
                let offset = row - entry.row;
                return (offset < range.end - range.start).then(|| range.start + offset);
            }
            extra = extra.saturating_add(entry.height - (range.end - range.start));
        }
        Some(row.saturating_sub(extra))
    }
    pub fn map_scroll(&self, from: usize, line: u64) -> u64 {
        let row = self.view_row(from, line);
        let target = usize::from(from == 0);
        if let Some(line) = self.document_line(target, row) {
            return line;
        }
        self.blocks
            .iter()
            .find(|entry| row >= entry.row && row < entry.row.saturating_add(entry.height))
            .map_or(line, |entry| {
                if target == 0 {
                    entry.block.left.end
                } else {
                    entry.block.right.end
                }
            })
    }
}
