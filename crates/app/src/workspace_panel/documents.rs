// SPDX-License-Identifier: MPL-2.0
//! Metadata-only document list; no filesystem reads while filtering or painting.
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_ui::{
    controls::Key,
    variable_list::{VariableItemSource, VariableList},
    widgets::Theme,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentItem {
    pub index: usize,
    pub title: String,
    pub path: String,
    pub dirty: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAction {
    Activate(usize),
    Save(usize),
    Close(usize),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    TabOrder,
    Name,
    Path,
}
#[derive(Default)]
struct Items {
    rows: Vec<DocumentItem>,
}
impl VariableItemSource for Items {
    fn len(&self) -> Option<usize> {
        Some(self.rows.len())
    }
    fn discovered(&self) -> usize {
        self.rows.len()
    }
    fn index_at(&self, offset: f64) -> usize {
        (offset.max(0.0) / 28.0) as usize
    }
    fn offset_of(&self, index: usize) -> f64 {
        index as f64 * 28.0
    }
    fn item_height(&self, _: usize) -> f32 {
        28.0
    }
    fn label(&self, index: usize) -> &str {
        &self.rows[index].title
    }
}
pub struct DocumentList {
    pub open: bool,
    all: Vec<DocumentItem>,
    items: Items,
    list: VariableList,
    filter: String,
    sort: Sort,
    generation: u64,
}
impl Default for DocumentList {
    fn default() -> Self {
        Self {
            open: false,
            all: vec![],
            items: Items::default(),
            list: VariableList {
                bounds: Rect::default(),
                offset: 0.0,
                selected: None,
            },
            filter: String::new(),
            sort: Sort::TabOrder,
            generation: 0,
        }
    }
}
impl DocumentList {
    /// Call when tab metadata changes. Equal data avoids rebuilding indexes.
    pub fn update(&mut self, items: Vec<DocumentItem>) {
        if self.all != items {
            self.generation = self.generation.wrapping_add(1);
            self.all = items;
            self.rebuild();
        }
    }
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.rebuild();
    }
    pub fn set_sort(&mut self, sort: Sort) {
        self.sort = sort;
        self.rebuild();
    }
    fn rebuild(&mut self) {
        let selected = self.list.selected.and_then(|i| self.items.rows.get(i)).map(|i| i.index);
        self.items.rows = self
            .all
            .iter()
            .filter(|i| i.title.to_lowercase().contains(&self.filter) || i.path.to_lowercase().contains(&self.filter))
            .cloned()
            .collect();
        match self.sort {
            Sort::TabOrder => self.items.rows.sort_by_key(|i| i.index),
            Sort::Name => self
                .items
                .rows
                .sort_by_cached_key(|i| (i.title.to_lowercase(), i.index)),
            Sort::Path => self.items.rows.sort_by_cached_key(|i| (i.path.to_lowercase(), i.index)),
        }
        self.list.selected = selected
            .and_then(|id| self.items.rows.iter().position(|i| i.index == id))
            .or(if self.items.rows.is_empty() { None } else { Some(0) });
    }
    pub fn selected(&self) -> Option<usize> {
        Some(self.items.rows.get(self.list.selected?)?.index)
    }
    pub fn semantics(
        &self,
        parent: bareline_ui::ViewId,
        prefix: u64,
        focused: bool,
    ) -> Vec<bareline_ui::semantics::SemanticEntry> {
        if !self.open {
            return Vec::new();
        }
        let mut nodes = bareline_ui::semantics::variable_list(
            &self.list,
            &self.items,
            parent,
            |row| bareline_ui::ViewId(prefix + 65536 + self.generation * 1_048_576 + self.items.rows[row].index as u64),
            "documents.activate",
            focused,
        );
        for node in &mut nodes {
            node.node.focused = focused && node.node.selected;
            node.node.actions.push(bareline_ui::widgets::SemanticAction::Focus);
            let item = self
                .items
                .rows
                .iter()
                .find(|i| prefix + 65536 + self.generation * 1_048_576 + i.index as u64 == node.node.id.0);
            if let Some(item) = item {
                node.node.value = Some(format!("{}{}", item.path, if item.dirty { " · unsaved" } else { "" }));
            }
        }
        nodes
    }
    pub fn accessibility_action(&mut self, id: u64, prefix: u64, invoke: bool) -> Option<DocumentAction> {
        let row =
            self.list.visible(&self.items, 0).into_iter().find(|row| {
                prefix + 65536 + self.generation * 1_048_576 + self.items.rows[row.index].index as u64 == id
            })?;
        self.list.selected = Some(row.index);
        invoke.then(|| DocumentAction::Activate(self.items.rows[row.index].index))
    }
    pub fn save_selected(&self) -> Option<DocumentAction> {
        self.selected().map(DocumentAction::Save)
    }
    pub fn close_selected(&self) -> Option<DocumentAction> {
        self.selected().map(DocumentAction::Close)
    }
    pub fn key(&mut self, key: Key) -> Option<DocumentAction> {
        if self.items.rows.is_empty() {
            return None;
        }
        let row = self.list.selected.unwrap_or(0);
        self.list.selected = Some(match key {
            Key::Up => row.saturating_sub(1),
            Key::Down => (row + 1).min(self.items.rows.len() - 1),
            Key::Home => 0,
            Key::End => self.items.rows.len() - 1,
            Key::Enter => return self.selected().map(DocumentAction::Activate),
            _ => row,
        });
        self.list.reveal(&self.items, self.list.selected.unwrap());
        None
    }
    pub fn pointer(&mut self, p: Point) -> Option<DocumentAction> {
        self.list.selected = self.list.hit_test(&self.items, p);
        self.selected().map(DocumentAction::Activate)
    }
    pub fn draw(&mut self, bounds: Rect, ops: &mut Vec<DrawOp>) {
        self.draw_with_theme(bounds, Theme::default(), ops);
    }
    pub fn draw_with_theme(&mut self, bounds: Rect, theme: Theme, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }

        ops.push(DrawOp::Fill(bounds, theme.surface));
        ops.push(DrawOp::Text {
            origin: Point {
                x: bounds.x + 12.0,
                y: bounds.y + 8.0,
            },
            text: if self.filter.is_empty() {
                "Documents · type to filter".into()
            } else {
                format!("Documents · {}", self.filter)
            },
            size: 13.0,
            color: theme.text,
        });
        self.list.bounds = Rect {
            y: bounds.y + 34.0,
            height: (bounds.height - 34.0).max(0.0),
            ..bounds
        };
        self.list.paint(&self.items, theme, ops);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn five_thousand_metadata_rows_filter_sort_and_keep_tab_identity() {
        let mut panel = DocumentList::default();
        panel.update(
            (0..5000)
                .map(|index| DocumentItem {
                    index,
                    title: format!("doc{index:04}"),
                    path: format!("/dir/{index}"),
                    dirty: false,
                })
                .collect(),
        );
        panel.set_filter("doc499");
        panel.set_sort(Sort::Name);
        assert_eq!(panel.items.rows.len(), 10);
        assert_eq!(panel.key(Key::Enter), Some(DocumentAction::Activate(4990)));
        panel.list.bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 238.0,
            height: 280.0,
        };
        assert!(panel.list.visible(&panel.items, 1).len() <= 11);
    }
}
