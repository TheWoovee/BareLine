// SPDX-License-Identifier: MPL-2.0
//! Lazy explorer. Path authorization belongs to the caller before `add_root`.
use bareline_renderer::{DrawOp, LayoutError, Point, Rect, TextBackend};
use bareline_ui::{
    controls::{Key, UiEvent},
    virtual_tree::{NodeId, Tree, TreeAction, TreeItem, TreeSource},
    widgets::Theme,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
    root: PathBuf,
}
struct Listing {
    parent: NodeId,
    entries: Vec<(PathBuf, bool)>,
    status: Option<String>,
    cursor: Option<DirectoryCursor>,
}
struct PendingListing {
    receiver: Receiver<Listing>,
    cancel: Arc<AtomicBool>,
    replace: bool,
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
    pending: Option<PendingListing>,
    notify: Arc<dyn Fn() + Send + Sync>,
    pub message: Option<String>,
    selected: Option<NodeId>,
    directory_guard: Option<DirectoryGuard>,
    excludes: Arc<Vec<String>>,
    dirty: BTreeSet<u64>,
    expanded: BTreeSet<u64>,
}
impl Drop for WorkspacePanel {
    fn drop(&mut self) {
        if let Some(pending) = &self.pending { pending.cancel.store(true, Ordering::Relaxed); }
    }
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
            dirty: BTreeSet::new(),
            expanded: BTreeSet::new(),
        }
    }
    pub fn set_directory_guard(
        &mut self,
        guard: impl Fn(&std::path::Path) -> std::io::Result<Box<dyn Send>> + Send + Sync + 'static,
    ) {
        self.directory_guard = Some(Arc::new(guard));
    }
    /// Bounded basename glob patterns (`*` and `?`), evaluated off the UI thread.
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
            self.request(id, false);
        }
    }
    pub fn watch_roots(&self) -> Vec<PathBuf> {
        let mut ids: Vec<_> = self.expanded.iter().copied().collect();
        ids.sort_by_key(|id| (Some(NodeId(*id)) != self.selected, *id));
        ids.into_iter().filter_map(|id| self.model.nodes.get(&id))
            .filter(|n| n.directory && n.children.is_some())
            .take(256).map(|n| n.path.clone()).collect()
    }
    /// Invalidate only the changed directory; unaffected path/node IDs survive.
    pub fn directory_changed(&mut self, directory: &std::path::Path) {
        for (&id, node) in &self.model.nodes {
            if node.path == directory && node.children.is_some() {
                self.dirty.insert(id);
            }
        }
    }
    fn release_children(&mut self, id: NodeId) {
        let mut pending = self.model.nodes.get_mut(&id.0)
            .and_then(|n| n.children.take()).unwrap_or_default();
        while let Some(child) = pending.pop() {
            if let Some(node) = self.model.nodes.remove(&child.0) {
                pending.extend(node.children.unwrap_or_default());
            }
            self.dirty.remove(&child.0);
            self.expanded.remove(&child.0);
            if self.selected == Some(child) { self.selected = Some(id); }
        }
        self.tree.update_child_count(id, 0);
    }
    pub fn selected_path(&self) -> Option<&std::path::Path> {
        Some(&self.model.nodes.get(&self.selected?.0)?.path)
    }
    pub fn refresh_tree(&mut self) {
        if let Some(pending) = &self.pending { pending.cancel.store(true, Ordering::Relaxed); }
        self.dirty.extend(self.model.nodes.iter().filter(|(_, n)| n.directory && n.children.is_some()).map(|(&id, _)| id));
        self.message = Some("Refreshing loaded folders…".into());
    }
    pub fn width(&self) -> f32 {
        if self.open { 238.0 } else { 0.0 }
    }
    pub fn show(&mut self) {
        self.open = true;
    }
    pub fn hide(&mut self) {
        self.open = false;
        if let Some(pending) = &self.pending { pending.cancel.store(true, Ordering::Relaxed); }
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
    fn request(&mut self, id: NodeId, replace: bool) {
        if self.pending.is_some() {
            self.message = Some("Folder discovery busy; try again when ready".into());
            return;
        }
        if self.model.nodes.get(&id.0).is_none_or(|node| !node.directory || (!replace && node.children.is_some() && node.cursor.is_none())) { return; }
        // Continuation replaces the previous page at the resident-node ceiling.
        // The live ReadDir cursor advances, so every entry remains reachable.
        let Some(target) = self.model.nodes.get(&id.0).map(|n| n.path.clone()) else { return; };
        let root = self.model.roots.iter().filter_map(|id| self.model.nodes.get(&id.0)).filter(|n| target.starts_with(&n.path)).max_by_key(|n| n.path.components().count()).map(|n| n.path.clone()).unwrap_or_else(|| target.clone());
        while NODE_LIMIT.saturating_sub(self.model.nodes.len()) < ENTRY_LIMIT {
            let candidate = self.model.nodes.iter()
                .filter(|(candidate, n)| **candidate != id.0 && !target.starts_with(&n.path) && n.children.as_ref().is_some_and(|c| !c.is_empty()))
                .min_by_key(|(candidate, _)| (self.expanded.contains(candidate), **candidate)).map(|(&candidate, _)| NodeId(candidate));
            let Some(candidate) = candidate else { break; };
            self.release_children(candidate);
            if let Some(node) = self.model.nodes.get_mut(&candidate.0) { node.cursor = None; }
            self.expanded.remove(&candidate.0);
        }
        if !replace && NODE_LIMIT.saturating_sub(self.model.nodes.len()) < ENTRY_LIMIT {
            self.release_children(id);
        }
        let replaced = if replace { self.model.nodes.get(&id.0).and_then(|n| n.children.as_ref()).map_or(0, Vec::len) } else { 0 };
        let capacity = ENTRY_LIMIT.min(NODE_LIMIT.saturating_sub(self.model.nodes.len()) + replaced);
        if capacity == 0 {
            self.message =
                Some("Tree budget reached; Refresh Workspace to release loaded branches".into());
            return;
        }
        let Some(node) = self.model.nodes.get_mut(&id.0) else {
            return;
        };
        if !node.directory || (!replace && node.children.is_some() && node.cursor.is_none()) {
            return;
        }
        let cursor = if replace { node.cursor = None; None } else { node.cursor.take() };
        let path = node.path.clone();
        let guard = self.directory_guard.clone();
        let excludes = self.excludes.clone();
        let notify = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
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
                            root,
                        })
                    })(),
                };
                let result = match cursor {
                    Ok(cursor) => enumerate_page(id, cursor, capacity, &excludes, &worker_cancel),
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
                self.pending = Some(PendingListing { receiver: rx, cancel, replace });
                self.message = Some("Loading folder…".into());
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }
    pub fn pump(&mut self) -> bool {
        if self.open && self.pending.is_none() {
            if let Some(id) = self.dirty.pop_first() {
                let id = NodeId(id);
                self.request(id, true);
                return true;
            }
        }
        let Some(pending) = &self.pending else {
            return false;
        };
        let listing = match pending.receiver.try_recv() {
            Ok(v) => v,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(_) => {
                self.pending = None;
                self.message = Some("Folder worker stopped".into());
                return true;
            }
        };
        let pending = self.pending.take().unwrap();
        if pending.cancel.load(Ordering::Relaxed) {
            self.dirty.insert(listing.parent.0);
            return true;
        }
        self.apply_listing(listing, pending.replace);
        true
    }
    fn apply_listing(&mut self, listing: Listing, replace: bool) {
        if !self.model.nodes.contains_key(&listing.parent.0) { return; }
        self.message = listing.status;
        let old = self.model.nodes[&listing.parent.0].children.clone().unwrap_or_default();
        let mut resident: BTreeMap<_, _> = old.iter().filter_map(|id| self.model.nodes.get(&id.0).map(|n| (n.path.clone(), (*id, n.directory)))).collect();
        if replace {
            let incoming: BTreeSet<_> = listing.entries.iter().map(|(path, _)| path).collect();
            let removed: Vec<_> = resident.iter().filter(|(path, _)| !incoming.contains(path)).map(|(path, (id, _))| (path.clone(), *id)).collect();
            for (path, id) in removed {
                self.release_children(id);
                self.model.nodes.remove(&id.0);
                self.dirty.remove(&id.0); self.expanded.remove(&id.0);
                if self.selected == Some(id) { self.selected = Some(listing.parent); }
                resident.remove(&path);
            }
        }
        let mut children = if replace { Vec::new() } else { old.clone() };
        let mut retained: BTreeSet<_> = children.iter().map(|id| id.0).collect();
        let mut page_paths = BTreeSet::new();
        for (path, directory) in listing.entries {
            if !page_paths.insert(path.clone()) { continue; }
            let id = if let Some((id, old_directory)) = resident.remove(&path) {
                if old_directory != directory {
                    self.release_children(id);
                    let node = self.model.nodes.get_mut(&id.0).unwrap();
                    node.directory = directory; node.cursor = None;
                }
                id
            } else { self.model.insert(path, directory) };
            if retained.insert(id.0) { children.push(id); }
        }
        if replace {
            for (_, (id, _)) in resident {
                self.release_children(id);
                self.model.nodes.remove(&id.0);
                self.dirty.remove(&id.0); self.expanded.remove(&id.0);
                if self.selected == Some(id) { self.selected = Some(listing.parent); }
            }
        }
        self.tree.replace_child_ids(listing.parent, &old, &children);
        if let Some(node) = self.model.nodes.get_mut(&listing.parent.0) {
            node.children = Some(children);
            node.cursor = listing.cursor;
            self.tree
                .update_child_count(listing.parent, node.children.as_ref().unwrap().len());
        }
        // The control requested this selected branch before its children existed.
        self.tree.expand_selected(&self.model);
    }
    fn action(&mut self, action: Option<TreeAction>) -> Option<PanelAction> {
        match action {
            Some(TreeAction::Selected(id)) => self.selected = Some(id),
            Some(TreeAction::RequestChildren(Some(id))) => {
                self.selected = Some(id);
                self.expanded.insert(id.0);
                self.request(id, false);
            }
            Some(TreeAction::Expanded(id)) => { self.selected = Some(id); self.expanded.insert(id.0); }
            Some(TreeAction::Collapsed(id)) => { self.selected = Some(id); self.expanded.remove(&id.0); }
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
    pub fn semantics(&self, parent: bareline_ui::ViewId, prefix: u64) -> Vec<bareline_ui::semantics::SemanticEntry> {
        if !self.open { return Vec::new(); }
        let mut entries = self.tree.visible_semantics(&self.model, parent, |id| bareline_ui::ViewId(prefix + id.0), "workspace.activateEntry");
        for entry in &mut entries { entry.node.actions.push(bareline_ui::widgets::SemanticAction::Focus); }
        entries
    }
    pub fn accessibility_action(&mut self, id: NodeId, action: bareline_ui::widgets::SemanticAction) -> Option<PanelAction> {
        self.tree.state.focused = true;
        let action = if action == bareline_ui::widgets::SemanticAction::Invoke && self.model.nodes.get(&id.0).is_some_and(|n| n.directory) {
            if self.expanded.contains(&id.0) { bareline_ui::widgets::SemanticAction::Collapse } else { bareline_ui::widgets::SemanticAction::Expand }
        } else { action };
        let action = self.tree.accessibility_action(&self.model, id, action);
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
    cancel: &AtomicBool,
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
        if cancel.load(Ordering::Relaxed) { break; }
        let Some(entry) = cursor.entries.next() else {
            exhausted = true;
            break;
        };
        match entry.and_then(|e| Ok((e.path(), e.file_type()?))) {
            Ok((path, kind)) => {
                if !excludes
                    .iter()
                    .any(|s| excluded(s, &path, &cursor.root))
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
fn excluded(pattern: &str, path: &std::path::Path, root: &std::path::Path) -> bool {
    if pattern.contains(['/', '\\']) {
        let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let pattern = pattern.replace('\\', "/");
        glob_matches(&pattern, &relative) || pattern.strip_suffix("/**").is_some_and(|folder| glob_matches(folder, &relative))
    } else { path.file_name().is_some_and(|name| glob_matches(pattern, &name.to_string_lossy())) }
}
fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<_> = pattern.chars().collect();
    let name: Vec<_> = name.chars().collect();
    let (mut p, mut n, mut star, mut retry) = (0, 0, None, 0);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1; n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p); p += 1; retry = n;
        } else if let Some(s) = star {
            retry += 1; n = retry; p = s + 1;
        } else { return false; }
    }
    while p < pattern.len() && pattern[p] == '*' { p += 1; }
    p == pattern.len()
}
#[cfg(test)]
fn enumerate(parent: NodeId, path: PathBuf, capacity: usize) -> Listing {
    match std::fs::read_dir(&path) {
        Ok(entries) => enumerate_page(
            parent,
            DirectoryCursor {
                entries,
                _guard: None,
                root: path,
            },
            capacity,
            &[],
            &AtomicBool::new(false),
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
    fn listing(parent: NodeId, entries: Vec<(PathBuf, bool)>) -> Listing {
        Listing { parent, entries, status: None, cursor: None }
    }
    #[test]
    fn incremental_refresh_preserves_survivors_and_other_branches() {
        let mut panel = WorkspacePanel::new(Arc::new(|| {}));
        let root = PathBuf::from("root");
        panel.add_root(root.clone());
        panel.add_root(PathBuf::from("other"));
        let parent = panel.model.roots[0];
        let other = panel.model.roots[1];
        panel.apply_listing(listing(parent, vec![(root.join("a"), false), (root.join("b"), true)]), false);
        let survivor = panel.model.nodes[&parent.0].children.as_ref().unwrap()[1];
        panel.apply_listing(listing(survivor, vec![(root.join("b/child"), false)]), false);
        let child = panel.model.nodes[&survivor.0].children.as_ref().unwrap()[0];
        panel.apply_listing(listing(parent, vec![(root.join("b"), true), (root.join("c"), false)]), true);
        assert_eq!(panel.model.nodes[&parent.0].children.as_ref().unwrap()[0], survivor);
        assert!(panel.model.nodes.contains_key(&child.0));
        assert!(panel.model.nodes.contains_key(&other.0));
        assert!(!panel.model.nodes.values().any(|n| n.path == root.join("a")));
        panel.directory_changed(&root);
        panel.directory_changed(&root);
        assert_eq!(panel.dirty.len(), 1);
    }
    #[test]
    fn cancelled_discovery_cannot_publish_or_start_concurrent_work() {
        let mut panel = WorkspacePanel::new(Arc::new(|| {}));
        panel.add_root(PathBuf::from("root"));
        let parent = panel.model.roots[0];
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        panel.pending = Some(PendingListing { receiver, cancel: cancel.clone(), replace: false });
        panel.hide();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(panel.pending.is_some());
        assert!(sender.send(listing(parent, vec![(PathBuf::from("stale"), false)])).is_ok());
        assert!(panel.pump());
        assert_eq!(panel.model.nodes.len(), 1);
        assert!(panel.dirty.contains(&parent.0));
    }
    #[test]
    fn continuation_visits_excluded_entries_and_retains_position() {
        let root = std::env::temp_dir().join(format!("bareline-explorer-pages-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..9 { std::fs::write(root.join(format!("file{index}.tmp")), []).unwrap(); }
        let cursor = DirectoryCursor { entries: std::fs::read_dir(&root).unwrap(), _guard: None, root: root.clone() };
        let first = enumerate_page(NodeId(1), cursor, 3, &["*.tmp".into()], &AtomicBool::new(false));
        assert!(first.entries.is_empty());
        assert!(first.cursor.is_some());
        let mut cursor = first.cursor;
        let mut seen = BTreeSet::new();
        while let Some(next) = cursor {
            let page = enumerate_page(NodeId(1), next, 3, &[], &AtomicBool::new(false));
            for (path, _) in page.entries { assert!(seen.insert(path)); }
            cursor = page.cursor;
        }
        assert_eq!(seen.len(), 6);
        assert!(glob_matches("f?le*.tmp", "fileλ.tmp"));
        assert!(!glob_matches("*.rs", "file.toml"));
        assert!(excluded("target/**", &root.join("target"), &root));
        assert!(excluded("src/*.tmp", &root.join("src/cache.tmp"), &root));
        std::fs::remove_dir_all(root).unwrap();
    }
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
        (bareline_commands::CommandId("workspace.loadMore"), "Load More Entries"),
        (bareline_commands::CommandId("workspace.refresh"), "Refresh Workspace"),
        (bareline_commands::CommandId("workspace.undoDelete"), "Undo Workspace Delete"),
        (bareline_commands::CommandId("outline.importFunctionList"), "Import Notepad++ Function List…"),
        (bareline_commands::CommandId("outline.loadDefinition"), "Load Outline Definition…"),
        (bareline_commands::CommandId("outline.exportDefinition"), "Export Outline Definition…"),
        (bareline_commands::CommandId("outline.cancelImport"), "Cancel Outline Import"),
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
            "Delete Selected Entry (Retain for Undo)",
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
