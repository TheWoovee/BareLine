// SPDX-License-Identifier: MPL-2.0
//! Lazy explorer. Path authorization belongs to the caller before `add_root`.
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_ui::{
    controls::{Key, UiEvent},
    virtual_tree::{NodeId, Tree, TreeAction, TreeItem, TreeSource},
    widgets::Theme,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
const ENTRY_LIMIT: usize = 4096;
const NODE_LIMIT: usize = 32768;
struct Node {
    path: PathBuf,
    label: String,
    directory: bool,
    children: Option<Vec<NodeId>>,
    cursor: Option<DirectoryCursor>,
}
#[derive(Default)]
struct Model {
    roots: Vec<NodeId>,
    nodes: BTreeMap<u64, Node>,
    next: u64,
}
impl Model {
    fn insert(&mut self, path: PathBuf, directory: bool) -> NodeId {
        self.next += 1;
        let id = NodeId(self.next);
        let label = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        self.nodes.insert(
            id.0,
            Node {
                path,
                label,
                directory,
                children: if directory { None } else { Some(Vec::new()) },
                cursor: None,
            },
        );
        id
    }
}
impl TreeSource for Model {
    fn child_count(&self, parent: Option<NodeId>) -> Option<usize> {
        Some(match parent {
            None => self.roots.len(),
            Some(id) => self.nodes.get(&id.0)?.children.as_ref()?.len(),
        })
    }
    fn child(&self, parent: Option<NodeId>, index: usize) -> Option<TreeItem<'_>> {
        let ids = match parent {
            None => &self.roots,
            Some(id) => self.nodes.get(&id.0)?.children.as_ref()?,
        };
        let id = *ids.get(index)?;
        let node = self.nodes.get(&id.0)?;
        Some(TreeItem {
            id,
            label: &node.label,
            expandable: node.directory,
            enabled: true,
        })
    }
}
struct DirectoryCursor {
    entries: std::fs::ReadDir,
    _guard: Option<Box<dyn Send>>,
}
struct Listing {
    parent: NodeId,
    entries: Vec<(PathBuf, bool)>,
    status: Option<String>,
    cursor: Option<DirectoryCursor>,
}
#[derive(Debug)]
pub enum PanelAction {
    Open(PathBuf),
}
type DirectoryGuard = Arc<dyn Fn(&std::path::Path) -> std::io::Result<Box<dyn Send>> + Send + Sync>;
pub struct WorkspacePanel {
    pub open: bool,
    model: Model,
    tree: Tree,
    pending: Option<Receiver<Listing>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    pub message: Option<String>,
    selected: Option<NodeId>,
    directory_guard: Option<DirectoryGuard>,
    excludes: Arc<Vec<String>>,
}
impl WorkspacePanel {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            open: false,
            model: Model::default(),
            tree: Tree::new(Rect::default()),
            pending: None,
            notify,
            message: None,
            selected: None,
            directory_guard: None,
            excludes: Arc::new(Vec::new()),
        }
    }
    pub fn set_directory_guard(
        &mut self,
        guard: impl Fn(&std::path::Path) -> std::io::Result<Box<dyn Send>> + Send + Sync + 'static,
    ) {
        self.directory_guard = Some(Arc::new(guard));
    }
    /// Exact directory/file basenames; opt-in and evaluated by the listing worker.
    pub fn set_excludes(&mut self, names: Vec<String>) {
        self.excludes = Arc::new(
            names
                .into_iter()
                .take(256)
                .map(|s| s.chars().take(256).collect())
                .collect(),
        );
        self.refresh_tree();
    }
    pub fn load_more(&mut self) {
        if let Some(id) = self.selected {
            self.request(id);
        }
    }
    pub fn selected_path(&self) -> Option<&std::path::Path> {
        Some(&self.model.nodes.get(&self.selected?.0)?.path)
    }
    pub fn refresh_tree(&mut self) {
        self.pending = None;
        let roots: Vec<_> = self
            .model
            .roots
            .iter()
            .map(|id| self.model.nodes[&id.0].path.clone())
            .collect();
        self.model = Model::default();
        self.tree.reset();
        self.selected = None;
        for root in roots {
            self.add_root(root);
        }
    }
    pub fn width(&self) -> f32 {
        if self.open { 238.0 } else { 0.0 }
    }
    pub fn show(&mut self) {
        self.open = true;
    }
    pub fn hide(&mut self) {
        self.open = false;
    }
    /// Caller must authorize the actual path before adding; this does no I/O.
    pub fn add_root(&mut self, path: PathBuf) {
        self.open = true;
        if self.model.roots.len() >= 64 {
            self.message = Some("Workspace root limit reached".into());
            return;
        }
        if self
            .model
            .roots
            .iter()
            .any(|id| self.model.nodes[&id.0].path == path)
        {
            return;
        }
        let id = self.model.insert(path, true);
        self.model.roots.push(id);
    }
    fn request(&mut self, id: NodeId) {
        if self.pending.is_some() {
            self.message = Some("Folder discovery busy; try again when ready".into());
            return;
        }
        let capacity = ENTRY_LIMIT.min(NODE_LIMIT.saturating_sub(self.model.nodes.len()));
        if capacity == 0 {
            self.message =
                Some("Tree budget reached; Refresh Workspace to release loaded branches".into());
            return;
        }
        let Some(node) = self.model.nodes.get_mut(&id.0) else {
            return;
        };
        if !node.directory || (node.children.is_some() && node.cursor.is_none()) {
            return;
        }
        let cursor = node.cursor.take();
        let path = node.path.clone();
        let guard = self.directory_guard.clone();
        let excludes = self.excludes.clone();
        let notify = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("workspace-discovery".into())
            .spawn(move || {
                let cursor = match cursor {
                    Some(cursor) => Ok(cursor),
                    None => (|| -> std::io::Result<DirectoryCursor> {
                        let retained = guard.as_ref().map(|g| g(&path)).transpose()?;
                        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                            return Err(std::io::Error::other("Linked folders are not traversed"));
                        }
                        Ok(DirectoryCursor {
                            entries: std::fs::read_dir(&path)?,
                            _guard: retained,
                        })
                    })(),
                };
                let result = match cursor {
                    Ok(cursor) => enumerate_page(id, cursor, capacity, &excludes),
                    Err(error) => Listing {
                        parent: id,
                        entries: vec![],
                        cursor: None,
                        status: Some(error.to_string()),
                    },
                };
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.message = Some("Loading folder…".into());
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }
    pub fn pump(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        let listing = match rx.try_recv() {
            Ok(v) => v,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.pending = None;
                self.message = Some("Folder worker stopped".into());
                return true;
            }
        };
        self.pending = None;
        self.message = listing.status;
        let children: Vec<_> = listing
            .entries
            .into_iter()
            .map(|(path, directory)| self.model.insert(path, directory))
            .collect();
        if let Some(node) = self.model.nodes.get_mut(&listing.parent.0) {
            node.children.get_or_insert_with(Vec::new).extend(children);
            node.cursor = listing.cursor;
            self.tree
                .update_child_count(listing.parent, node.children.as_ref().unwrap().len());
        }
        // The control requested this selected branch before its children existed.
        self.tree.expand_selected(&self.model);
        true
    }
    fn action(&mut self, action: Option<TreeAction>) -> Option<PanelAction> {
        match action {
            Some(TreeAction::Selected(id)) => self.selected = Some(id),
            Some(TreeAction::RequestChildren(Some(id))) => {
                self.selected = Some(id);
                self.request(id);
            }
            Some(TreeAction::Expanded(id) | TreeAction::Collapsed(id)) => self.selected = Some(id),
            Some(TreeAction::Activated(id)) => {
                let node = self.model.nodes.get(&id.0)?;
                if !node.directory {
                    return Some(PanelAction::Open(node.path.clone()));
                }
                let action = self.tree.expand_selected(&self.model);
                return self.action(action);
            }
            _ => {}
        }
        None
    }
    pub fn key(&mut self, key: Key) -> Option<PanelAction> {
        if !self.open {
            return None;
        }
        self.tree.state.focused = true;
        let action = self.tree.event(UiEvent::Key(key), &self.model);
        self.action(action)
    }
    /// Select the contextual target without opening it or toggling a branch.
    pub fn select_context(&mut self, point: Point) {
        if !self.open || !self.tree.bounds.contains(point) {
            return;
        }
        self.tree.state.focused = true;
        let action = self.tree.event(
            UiEvent::PointerDown(Point {
                x: self.tree.bounds.x + self.tree.bounds.width - 1.0,
                y: point.y,
            }),
            &self.model,
        );
        self.action(action);
    }
    pub fn pointer(&mut self, point: Point) -> Option<PanelAction> {
        if !self.open || !self.tree.bounds.contains(point) {
            return None;
        }
        self.tree.state.focused = true;
        let selected = self.tree.event(UiEvent::PointerDown(point), &self.model);
        let toggled = matches!(
            selected,
            Some(
                TreeAction::Expanded(_) | TreeAction::Collapsed(_) | TreeAction::RequestChildren(_)
            )
        );
        self.action(selected);
        if toggled {
            return None;
        }
        let action = self.tree.event(UiEvent::Key(Key::Enter), &self.model);
        self.action(action)
    }
    pub fn draw(
        &mut self,
        _backend: &mut impl TextBackend,
        _width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<Option<Rect>, LayoutError> {
        if !self.open {
            return Ok(None);
        }
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: self.width(),
            height,
        };
        let theme = Theme::default();
        ops.push(DrawOp::Fill(bounds, theme.surface));
        ops.push(DrawOp::Text {
            origin: Point { x: 16.0, y: 8.0 },
            text: "Workspace".into(),
            size: 13.0,
            color: theme.text,
        });
        self.tree.bounds = Rect {
            y: 34.0,
            height: (height - 62.0).max(0.0),
            ..bounds
        };
        self.tree.paint(&self.model, theme, ops);
        let status = self
            .message
            .as_deref()
            .unwrap_or(if self.model.roots.is_empty() {
                "Open a folder to browse files"
            } else {
                ""
            });
        ops.push(DrawOp::PushClip(bounds));
        ops.push(DrawOp::Text {
            origin: Point {
                x: 10.0,
                y: (height - 22.0).max(34.0),
            },
            text: status.into(),
            size: 12.0,
            color: theme.muted,
        });
        ops.push(DrawOp::PopClip);
        Ok(Some(bounds))
    }
}
fn enumerate_page(
    parent: NodeId,
    mut cursor: DirectoryCursor,
    capacity: usize,
    excludes: &[String],
) -> Listing {
    let mut result = Listing {
        parent,
        entries: vec![],
        status: None,
        cursor: None,
    };
    let mut exhausted = false;
    // Count visited entries, not only matches: an excluded million-entry folder
    // still yields after one page and exposes continuation to the user.
    for _ in 0..capacity {
        let Some(entry) = cursor.entries.next() else {
            exhausted = true;
            break;
        };
        match entry.and_then(|e| Ok((e.path(), e.file_type()?))) {
            Ok((path, kind)) => {
                if !excludes
                    .iter()
                    .any(|s| path.file_name().is_some_and(|n| n == s.as_str()))
                {
                    result
                        .entries
                        .push((path, kind.is_dir() && !kind.is_symlink()));
                }
            }
            Err(error) => result.status = Some(format!("Partial folder: {error}")),
        }
    }
    if !exhausted {
        result.cursor = Some(cursor);
        result.status = Some("Partial folder · Load More Entries to continue".into());
    }
    result
        .entries
        .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    result
}
#[cfg(test)]
fn enumerate(parent: NodeId, path: PathBuf, capacity: usize) -> Listing {
    match std::fs::read_dir(path) {
        Ok(entries) => enumerate_page(
            parent,
            DirectoryCursor {
                entries,
                _guard: None,
            },
            capacity,
            &[],
        ),
        Err(error) => Listing {
            parent,
            entries: vec![],
            status: Some(error.to_string()),
            cursor: None,
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_shallow_bounded_and_reports_missing_paths() {
        let root = std::env::temp_dir().join(format!("bareline-explorer-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for i in 0..8 {
            std::fs::write(root.join(format!("file{i}")), b"").unwrap();
        }
        std::fs::write(root.join("nested/hidden"), b"").unwrap();
        let result = enumerate(NodeId(1), root.clone(), 3);
        assert_eq!(result.entries.len(), 3);
        assert!(result.status.unwrap().contains("Partial"));
        assert!(
            result
                .entries
                .iter()
                .all(|(p, _)| p.parent() == Some(root.as_path()))
        );
        assert!(
            enumerate(NodeId(1), root.join("missing"), 3)
                .status
                .is_some()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub const TOGGLE: bareline_commands::CommandId = bareline_commands::CommandId("view.workspace");
pub const OPEN_FOLDER: bareline_commands::CommandId =
    bareline_commands::CommandId("workspace.openFolder");
pub fn register_commands(registry: &mut bareline_commands::CommandRegistry) {
    for (id, title) in [
        (TOGGLE, "Toggle Workspace"),
        (OPEN_FOLDER, "Open Workspace Folder…"),
        (
            bareline_commands::CommandId("view.documents"),
            "Toggle Document List",
        ),
        (
            bareline_commands::CommandId("view.outline"),
            "Toggle Outline",
        ),
        (
            bareline_commands::CommandId("view.documentMap"),
            "Toggle Document Map",
        ),
        (
            bareline_commands::CommandId("documents.sortName"),
            "Sort Documents by Name",
        ),
        (
            bareline_commands::CommandId("documents.sortPath"),
            "Sort Documents by Path",
        ),
        (
            bareline_commands::CommandId("documents.sortTabOrder"),
            "Sort Documents by Tab Order",
        ),
        (
            bareline_commands::CommandId("documents.save"),
            "Save Selected Document",
        ),
        (
            bareline_commands::CommandId("documents.close"),
            "Close Selected Document",
        ),
        (
            bareline_commands::CommandId("workspace.createFile"),
            "Create File…",
        ),
        (
            bareline_commands::CommandId("workspace.createFolder"),
            "Create Folder…",
        ),
        (
            bareline_commands::CommandId("workspace.rename"),
            "Rename Selected Entry…",
        ),
        (
            bareline_commands::CommandId("workspace.delete"),
            "Delete Selected Entry (Empty Folders Only)",
        ),
    ] {
        let _ = registry.register(bareline_commands::CommandSpec {
            id,
            title,
            category: "Workspace",
            shortcut: "",
            action: bareline_commands::Action::Contributed(id),
        });
    }
}
pub mod documents;
pub mod map;
pub mod outline;
