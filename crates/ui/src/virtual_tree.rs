// SPDX-License-Identifier: MPL-2.0
//! Lazy tree indexing stores expanded branches, never a flattened node array.
use crate::controls::{ControlState, Key, UiEvent, visible_rows};
use crate::widgets::{Metrics, SemanticAction, SemanticRole, Semantics, Theme};
use crate::{ViewId, rect, text};
use bareline_renderer::{DrawOp, Rect};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(pub u64);
pub struct TreeItem<'a> {
    pub id: NodeId,
    pub label: &'a str,
    pub expandable: bool,
    pub enabled: bool,
}
/// `None` requests asynchronous discovery from the owner. Calls must read a
/// resident index only: no filesystem enumeration in this control or provider.
pub trait TreeSource {
    fn child_count(&self, parent: Option<NodeId>) -> Option<usize>;
    fn child(&self, parent: Option<NodeId>, index: usize) -> Option<TreeItem<'_>>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAction {
    Selected(NodeId),
    Activated(NodeId),
    Expanded(NodeId),
    Collapsed(NodeId),
    RequestChildren(Option<NodeId>),
}
struct Branch {
    id: NodeId,
    children: usize,
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    #[test]
    fn reordered_children_keep_descendants_and_deleted_selection_returns_parent() {
        let mut tree = Tree::new(rect(0.0, 0.0, 100.0, 100.0));
        tree.expanded.insert(
            vec![0],
            Branch {
                id: NodeId(10),
                children: 2,
            },
        );
        tree.expanded.insert(
            vec![0, 0],
            Branch {
                id: NodeId(20),
                children: 1,
            },
        );
        tree.selected = Some(vec![0, 0, 0]);
        tree.replace_child_ids(NodeId(10), &[NodeId(20), NodeId(30)], &[NodeId(30), NodeId(20)]);
        assert_eq!(tree.selected, Some(vec![0, 1, 0]));
        assert_eq!(tree.expanded.get(&vec![0, 1]).unwrap().id, NodeId(20));
        tree.replace_child_ids(NodeId(10), &[NodeId(30), NodeId(20)], &[NodeId(30)]);
        assert_eq!(tree.selected, Some(vec![0]));
        assert_eq!(tree.expanded.len(), 1);
    }
}
pub struct Tree {
    pub bounds: Rect,
    pub state: ControlState,
    pub metrics: Metrics,
    pub offset: f64,
    expanded: BTreeMap<Vec<usize>, Branch>,
    selected: Option<Vec<usize>>,
}
impl Tree {
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            state: ControlState::default(),
            metrics: Metrics::COMPACT,
            offset: 0.0,
            expanded: BTreeMap::new(),
            selected: None,
        }
    }
    /// Call when the owner replaces/reorders the source; paths are view indices,
    /// not durable identities. This prevents expansion leaking to a different node.
    pub fn reset(&mut self) {
        self.expanded.clear();
        self.selected = None;
        self.offset = 0.0;
    }
    /// Refresh a resident branch after append-only child discovery. Reordering
    /// existing children still requires `reset`, because paths are indices.
    pub fn update_child_count(&mut self, parent: NodeId, children: usize) {
        let Some(path) = self
            .expanded
            .iter()
            .find_map(|(path, branch)| (branch.id == parent).then(|| path.clone()))
        else {
            return;
        };
        self.expanded.get_mut(&path).unwrap().children = children;
        self.expanded.retain(|candidate, _| {
            !candidate.starts_with(&path) || candidate.len() <= path.len() || candidate[path.len()] < children
        });
        if self.selected.as_ref().is_some_and(|selected| {
            selected.starts_with(&path) && selected.len() > path.len() && selected[path.len()] >= children
        }) {
            self.selected = Some(path);
        }
    }
    /// Reconcile a refreshed branch by stable child identity, including every
    /// expanded descendant. Removed selection returns to its surviving parent.
    pub fn replace_child_ids(&mut self, parent: NodeId, old: &[NodeId], new: &[NodeId]) {
        let Some(parent_path) = self
            .expanded
            .iter()
            .find_map(|(path, branch)| (branch.id == parent).then(|| path.clone()))
        else {
            return;
        };
        let positions: BTreeMap<_, _> = new.iter().enumerate().map(|(index, id)| (id.0, index)).collect();
        let remap = |mut path: Vec<usize>| -> Option<Vec<usize>> {
            if path.starts_with(&parent_path) && path.len() > parent_path.len() {
                let id = old.get(path[parent_path.len()])?;
                path[parent_path.len()] = *positions.get(&id.0)?;
            }
            Some(path)
        };
        self.expanded = std::mem::take(&mut self.expanded)
            .into_iter()
            .filter_map(|(path, branch)| remap(path).map(|path| (path, branch)))
            .collect();
        self.expanded.get_mut(&parent_path).unwrap().children = new.len();
        self.selected = self
            .selected
            .take()
            .map(|path| remap(path).unwrap_or_else(|| parent_path.clone()));
    }
    fn subtree_rows(&self, path: &[usize]) -> usize {
        1usize.saturating_add(
            self.expanded
                .iter()
                .filter(|(p, _)| p.starts_with(path))
                .fold(0usize, |sum, (_, b)| sum.saturating_add(b.children)),
        )
    }
    pub fn rows(&self, source: &impl TreeSource) -> Option<usize> {
        Some(
            self.expanded
                .values()
                .fold(source.child_count(None)?, |sum, b| sum.saturating_add(b.children)),
        )
    }
    fn path_at(&self, mut row: usize, source: &impl TreeSource) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        let mut count = source.child_count(None)?;
        // Each expansion is limited to 64 levels; no unbounded recursion.
        for _ in 0..=64 {
            let mut first = 0usize;
            let mut found = None;
            for (branch_path, branch) in self
                .expanded
                .iter()
                .filter(|(p, _)| p.len() == path.len() + 1 && p.starts_with(&path))
            {
                let index = *branch_path.last()?;
                let plain = index.saturating_sub(first);
                if row < plain {
                    let mut result = path;
                    result.push(first + row);
                    return Some(result);
                }
                row -= plain;
                let span = self.subtree_rows(branch_path);
                if row < span {
                    path = branch_path.clone();
                    if row == 0 {
                        return Some(path);
                    }
                    row -= 1;
                    found = Some(branch.children);
                    break;
                }
                row -= span;
                first = index.saturating_add(1);
            }
            if let Some(children) = found {
                count = children;
                continue;
            }
            if row < count.saturating_sub(first) {
                path.push(first + row);
                return Some(path);
            }
            return None;
        }
        None
    }
    fn item<'a>(&self, path: &[usize], source: &'a impl TreeSource) -> Option<TreeItem<'a>> {
        let (&index, parent) = path.split_last()?;
        let parent = if parent.is_empty() {
            None
        } else {
            Some(self.expanded.get(parent)?.id)
        };
        source.child(parent, index)
    }
    fn row_of(&self, path: &[usize]) -> usize {
        let mut row = 0usize;
        for (depth, index) in path.iter().enumerate() {
            row = row.saturating_add(*index);
            for (p, _) in self
                .expanded
                .iter()
                .filter(|(p, _)| p.len() == depth + 1 && p.starts_with(&path[..depth]) && p[depth] < *index)
            {
                row = row.saturating_add(self.subtree_rows(p).saturating_sub(1));
            }
            if depth + 1 < path.len() {
                row = row.saturating_add(1);
            }
        }
        row
    }
    fn select(&mut self, path: Vec<usize>, source: &impl TreeSource) -> Option<TreeAction> {
        let item = self.item(&path, source)?;
        if !item.enabled {
            return None;
        }
        let row = self.row_of(&path);
        let top = row as f64 * self.metrics.row_height as f64;
        if top < self.offset {
            self.offset = top;
        } else if top + self.metrics.row_height as f64 > self.offset + self.bounds.height as f64 {
            self.offset = (top + self.metrics.row_height as f64 - self.bounds.height as f64).max(0.0);
        }
        self.selected = Some(path);
        Some(TreeAction::Selected(item.id))
    }
    pub fn expand_selected(&mut self, source: &impl TreeSource) -> Option<TreeAction> {
        let path = self.selected.as_ref()?;
        if path.len() >= 64 || self.expanded.contains_key(path) || self.state.disabled {
            return None;
        }
        let item = self.item(path, source)?;
        if !item.enabled || !item.expandable {
            return None;
        }
        let Some(children) = source.child_count(Some(item.id)) else {
            return Some(TreeAction::RequestChildren(Some(item.id)));
        };
        self.expanded.insert(path.clone(), Branch { id: item.id, children });
        Some(TreeAction::Expanded(item.id))
    }
    pub fn event(&mut self, event: UiEvent, source: &impl TreeSource) -> Option<TreeAction> {
        if let UiEvent::Focus(focused) = event {
            self.state.focused = focused && !self.state.disabled;
            return None;
        }
        if self.state.disabled {
            return None;
        }
        let Some(count) = self.rows(source) else {
            return Some(TreeAction::RequestChildren(None));
        };
        match event {
            UiEvent::PointerDown(p) if self.bounds.contains(p) && self.metrics.row_height > 0.0 => {
                let row =
                    (((p.y - self.bounds.y) as f64 + self.offset) / self.metrics.row_height as f64).max(0.0) as usize;
                let path = self.path_at(row, source)?;
                let expand = p.x < self.bounds.x + path.len() as f32 * 16.0;
                let action = self.select(path, source);
                if expand {
                    self.toggle_selected(source).or(action)
                } else {
                    action
                }
            }
            UiEvent::Key(key) if self.state.focused => match key {
                Key::Enter | Key::Space => self
                    .selected
                    .as_ref()
                    .and_then(|p| self.item(p, source))
                    .filter(|i| i.enabled)
                    .map(|i| TreeAction::Activated(i.id)),
                Key::Right => {
                    let path = self.selected.as_ref()?;
                    if self.expanded.get(path).is_some_and(|b| b.children > 0) {
                        let mut child = path.clone();
                        child.push(0);
                        self.select(child, source)
                    } else {
                        self.expand_selected(source)
                    }
                }
                Key::Left => {
                    let path = self.selected.as_ref()?;
                    if self.expanded.contains_key(path) {
                        self.toggle_selected(source)
                    } else if path.len() > 1 {
                        self.select(path[..path.len() - 1].to_vec(), source)
                    } else {
                        None
                    }
                }
                Key::Up | Key::Down | Key::Home | Key::End if count > 0 => {
                    let current = self.selected.as_ref().map(|p| self.row_of(p));
                    let reverse = matches!(key, Key::Up | Key::End);
                    let start = match key {
                        Key::Home => 0,
                        Key::End => count - 1,
                        Key::Up => current.unwrap_or(count).saturating_sub(1),
                        _ => current.map_or(0, |r| r.saturating_add(1)).min(count - 1),
                    };
                    // Disabled rows are skipped without retaining their data.
                    let indices: Box<dyn Iterator<Item = usize>> = if reverse {
                        Box::new((0..=start).rev())
                    } else {
                        Box::new(start..count)
                    };
                    for row in indices {
                        if let Some(path) = self.path_at(row, source)
                            && let Some(action) = self.select(path, source)
                        {
                            return Some(action);
                        }
                    }
                    None
                }
                _ => None,
            },
            _ => None,
        }
    }
    fn toggle_selected(&mut self, source: &impl TreeSource) -> Option<TreeAction> {
        let path = self.selected.as_ref()?;
        if let Some(branch) = self.expanded.remove(path) {
            self.expanded.retain(|p, _| !p.starts_with(path));
            Some(TreeAction::Collapsed(branch.id))
        } else {
            self.expand_selected(source)
        }
    }
    pub fn paint(&self, source: &impl TreeSource, theme: Theme, ops: &mut Vec<DrawOp>) {
        let Some(count) = self.rows(source) else {
            return;
        };
        ops.push(DrawOp::PushClip(self.bounds));
        for row in visible_rows(
            self.offset,
            self.bounds.height as f64,
            self.metrics.row_height as f64,
            Some(count),
            1,
        ) {
            let Some(path) = self.path_at(row, source) else {
                continue;
            };
            let Some(item) = self.item(&path, source) else {
                continue;
            };
            let bounds = rect(
                self.bounds.x,
                self.bounds.y + (row as f64 * self.metrics.row_height as f64 - self.offset) as f32,
                self.bounds.width,
                self.metrics.row_height,
            );
            if self.selected.as_ref() == Some(&path) {
                ops.push(DrawOp::Fill(bounds, theme.selection));
            }
            let x = bounds.x + (path.len() - 1) as f32 * 16.0;
            if item.expandable {
                text(
                    ops,
                    x,
                    bounds.y + 6.0,
                    if self.expanded.contains_key(&path) {
                        "⌄"
                    } else {
                        "›"
                    },
                    self.metrics.font_size,
                    theme.muted,
                );
            }
            text(
                ops,
                x + 18.0,
                bounds.y + 6.0,
                item.label,
                self.metrics.font_size,
                if self.state.disabled || !item.enabled {
                    theme.muted
                } else {
                    theme.text
                },
            );
        }
        ops.push(DrawOp::PopClip);
        if self.state.focused {
            ops.push(DrawOp::Stroke(self.bounds, theme.focus, 2.0));
        }
    }
    pub fn selected_semantics(
        &self,
        id: ViewId,
        localized_name: &str,
        command: &str,
        source: &impl TreeSource,
    ) -> Option<Semantics> {
        let path = self.selected.as_ref()?;
        let item = self.item(path, source)?;
        let row = self.row_of(path);
        let mut state = self.state;
        state.checked = true;
        state.disabled |= !item.enabled;
        let mut node = Semantics::new(
            id,
            SemanticRole::TreeItem,
            localized_name,
            command,
            rect(
                self.bounds.x,
                self.bounds.y + (row as f64 * self.metrics.row_height as f64 - self.offset) as f32,
                self.bounds.width,
                self.metrics.row_height,
            ),
            state,
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::Select)
        .action(SemanticAction::Invoke);
        node.value = Some(item.label.into());
        if item.expandable {
            let expanded = self.expanded.contains_key(path);
            node.expanded = Some(expanded);
            node = node.action(if expanded {
                SemanticAction::Collapse
            } else {
                SemanticAction::Expand
            });
        }
        Some(node)
    }
}

impl Tree {
    /// Visible rows plus their actual ancestors. Only resident discovered child
    /// metadata is consulted, and no unloaded branch is expanded by a query.
    pub fn visible_semantics(
        &self,
        source: &impl TreeSource,
        parent: ViewId,
        id: impl Fn(NodeId) -> ViewId,
        command: &str,
    ) -> Vec<crate::semantics::SemanticEntry> {
        let Some(count) = self.rows(source) else {
            return Vec::new();
        };
        if !self.metrics.row_height.is_finite() || self.metrics.row_height <= 0.0 {
            return Vec::new();
        }
        let visible = visible_rows(
            self.offset,
            self.bounds.height as f64,
            self.metrics.row_height as f64,
            Some(count),
            0,
        );
        let mut paths = std::collections::BTreeSet::new();
        for row in visible.take(4096) {
            if let Some(path) = self.path_at(row, source) {
                for depth in 1..=path.len() {
                    paths.insert(path[..depth].to_vec());
                }
            }
        }
        paths
            .into_iter()
            .filter_map(|path| {
                let item = self.item(&path, source)?;
                let parent = if path.len() == 1 {
                    parent
                } else {
                    id(self.item(&path[..path.len() - 1], source)?.id)
                };
                let selected = self.selected.as_ref() == Some(&path);
                let row = self.row_of(&path);
                let mut node = Semantics::new(
                    id(item.id),
                    SemanticRole::TreeItem,
                    item.label,
                    command,
                    rect(
                        self.bounds.x,
                        self.bounds.y + (row as f64 * self.metrics.row_height as f64 - self.offset) as f32,
                        self.bounds.width,
                        self.metrics.row_height,
                    ),
                    ControlState {
                        focused: self.state.focused && selected,
                        disabled: self.state.disabled || !item.enabled,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Select)
                .action(SemanticAction::Invoke);
                node.selected = selected;
                if item.expandable {
                    let expanded = self.expanded.contains_key(&path);
                    node.expanded = Some(expanded);
                    node = node.action(if expanded {
                        SemanticAction::Collapse
                    } else {
                        SemanticAction::Expand
                    });
                }
                Some(crate::semantics::SemanticEntry { parent, node })
            })
            .collect()
    }
    /// Resolve actions only within the current bounded semantic viewport.
    pub fn accessibility_action(
        &mut self,
        source: &impl TreeSource,
        target: NodeId,
        action: SemanticAction,
    ) -> Option<TreeAction> {
        let count = self.rows(source)?;
        let path = visible_rows(
            self.offset,
            self.bounds.height as f64,
            self.metrics.row_height as f64,
            Some(count),
            0,
        )
        .take(4096)
        .filter_map(|row| self.path_at(row, source))
        .find(|path| {
            self.item(path, source)
                .is_some_and(|item| item.id == target && item.enabled)
        })?;
        let selected = self.select(path, source)?;
        match action {
            SemanticAction::Focus | SemanticAction::Select => Some(selected),
            SemanticAction::Invoke => Some(TreeAction::Activated(target)),
            SemanticAction::Expand => self.expand_selected(source),
            SemanticAction::Collapse => {
                if self.selected.as_ref().is_some_and(|p| self.expanded.contains_key(p)) {
                    self.toggle_selected(source)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
