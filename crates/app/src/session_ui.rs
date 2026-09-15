// SPDX-License-Identifier: MPL-2.0
//! Session capture and first-frame-gated restore scheduling; no filesystem access.
use bareline_file_io::session::{SessionDocument, SessionManifest, SessionTab, ViewState};
use bareline_platform::SerializedPath;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::PathBuf,
};

pub struct CapturedTab {
    pub id: u64,
    pub document_id: u64,
    pub path: Option<PathBuf>,
    pub title: String,
    pub caret: usize,
    pub anchor: usize,
    pub scroll_y: f64,
    pub pinned: bool,
    pub split: u32,
}
pub fn capture(
    tabs: impl IntoIterator<Item = CapturedTab>,
    active_tab: Option<u64>,
    mru: Vec<u64>,
    recent: Vec<PathBuf>,
) -> io::Result<SessionManifest> {
    let mut manifest = SessionManifest {
        active_tab,
        mru,
        recent: recent.iter().map(|path| SerializedPath::from_native(path)).collect(),
        ..Default::default()
    };
    let mut documents = HashSet::new();
    for tab in tabs {
        if documents.insert(tab.document_id) {
            manifest.documents.push(SessionDocument {
                id: tab.document_id,
                path: tab.path.as_deref().map(SerializedPath::from_native),
                title: tab.title,
            });
        }
        manifest.tabs.push(SessionTab {
            id: tab.id,
            document_id: tab.document_id,
            pinned: tab.pinned,
            view: ViewState {
                caret: tab.caret as u64,
                anchor: tab.anchor as u64,
                scroll_y_bits: tab.scroll_y.to_bits(),
                split: tab.split,
                ..Default::default()
            },
        });
    }
    manifest.layout.split = manifest.tabs.iter().any(|tab| tab.view.split != 0);
    if let Some(active) = manifest.tabs.iter().find(|tab| Some(tab.id) == active_tab) {
        if active.view.split <= 1 {
            manifest.layout.active_pane = active.view.split;
            manifest.layout.active_tabs[active.view.split as usize] = active_tab;
        }
    }
    manifest.validate()?;
    Ok(manifest)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreState {
    AwaitingTrust,
    Ready,
    Loading,
    Loaded,
    Failed(String),
    Untitled,
}
#[derive(Clone, Debug)]
pub struct RestoreOpen {
    pub document_id: u64,
    pub path: PathBuf,
    pub tabs: Vec<SessionTab>,
}
pub struct RestoreQueue {
    manifest: SessionManifest,
    order: VecDeque<u64>,
    states: HashMap<u64, RestoreState>,
    approved: HashMap<u64, PathBuf>,
    first_frame: bool,
    in_flight: usize,
}
impl RestoreQueue {
    pub fn new(manifest: SessionManifest) -> io::Result<Self> {
        manifest.validate()?;
        let states = manifest
            .documents
            .iter()
            .map(|doc| {
                (
                    doc.id,
                    if doc.path.is_some() {
                        RestoreState::AwaitingTrust
                    } else {
                        RestoreState::Untitled
                    },
                )
            })
            .collect();
        Ok(Self {
            order: manifest.restore_order().into(),
            manifest,
            states,
            approved: HashMap::new(),
            first_frame: false,
            in_flight: 0,
        })
    }
    pub fn manifest(&self) -> &SessionManifest {
        &self.manifest
    }
    pub fn state(&self, document_id: u64) -> Option<&RestoreState> {
        self.states.get(&document_id)
    }
    pub fn first_frame_presented(&mut self) {
        self.first_frame = true;
    }
    /// Data-only trust candidate. Calling this does not canonicalize or open the path.
    pub fn candidate(&self, document_id: u64) -> Option<&SerializedPath> {
        self.manifest
            .documents
            .iter()
            .find(|doc| doc.id == document_id)?
            .path
            .as_ref()
    }
    /// Called only after the caller's PathOrigin::Session policy has approved read
    /// access and resolved this candidate. The queue never grants that trust itself.
    pub fn approve(&mut self, document_id: u64, canonical_path: PathBuf) -> bool {
        if !matches!(self.states.get(&document_id), Some(RestoreState::AwaitingTrust)) {
            return false;
        }
        self.approved.insert(document_id, canonical_path);
        self.states.insert(document_id, RestoreState::Ready);
        true
    }
    pub fn reject(&mut self, document_id: u64, reason: String) {
        if matches!(
            self.states.get(&document_id),
            Some(RestoreState::AwaitingTrust | RestoreState::Ready)
        ) {
            self.states.insert(document_id, RestoreState::Failed(reason));
            self.approved.remove(&document_id);
        }
    }
    /// At most two opens may be outstanding. The active/MRU order is preserved;
    /// unapproved candidates remain placeholders and cannot initiate document I/O.
    pub fn next_open(&mut self) -> Option<RestoreOpen> {
        if !self.first_frame || self.in_flight >= 2 {
            return None;
        }
        loop {
            let id = *self.order.front()?;
            match self.states.get(&id) {
                Some(RestoreState::AwaitingTrust) => return None,
                Some(RestoreState::Ready) => break,
                _ => {
                    self.order.pop_front();
                }
            }
        }
        let id = self.order.pop_front()?;
        let path = self.approved.remove(&id)?;
        self.states.insert(id, RestoreState::Loading);
        self.in_flight += 1;
        Some(RestoreOpen {
            document_id: id,
            path,
            tabs: self
                .manifest
                .tabs
                .iter()
                .filter(|tab| tab.document_id == id)
                .cloned()
                .collect(),
        })
    }
    pub fn completed(&mut self, document_id: u64, result: Result<(), String>) {
        if matches!(self.states.get(&document_id), Some(RestoreState::Loading)) {
            self.in_flight -= 1;
            self.states.insert(
                document_id,
                match result {
                    Ok(()) => RestoreState::Loaded,
                    Err(reason) => RestoreState::Failed(reason),
                },
            );
        }
    }
    pub fn pending(&self) -> bool {
        self.states.values().any(|state| {
            matches!(
                state,
                RestoreState::AwaitingTrust | RestoreState::Ready | RestoreState::Loading
            )
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn captured(id: u64) -> CapturedTab {
        CapturedTab {
            id,
            document_id: id,
            path: Some(PathBuf::from(format!("file-{id}"))),
            title: id.to_string(),
            caret: 42,
            anchor: 20,
            scroll_y: 123.75,
            pinned: false,
            split: 1,
        }
    }
    #[test]
    fn capture_preserves_exact_view_and_cloned_document_identity() {
        let mut cloned = captured(1);
        cloned.document_id = 0;
        let manifest = capture([captured(0), cloned], Some(1), vec![0], vec![]).unwrap();
        assert_eq!(manifest.documents.len(), 1);
        assert_eq!(manifest.tabs.len(), 2);
        assert_eq!(f64::from_bits(manifest.tabs[0].view.scroll_y_bits), 123.75);
    }
    #[test]
    fn five_hundred_restore_is_first_frame_and_trust_gated_with_two_in_flight() {
        let manifest = capture((0..500).map(captured), Some(499), vec![], vec![]).unwrap();
        let mut queue = RestoreQueue::new(manifest).unwrap();
        for id in 0..500 {
            assert!(queue.approve(id, PathBuf::from(format!("approved-{id}"))));
        }
        assert!(queue.next_open().is_none());
        queue.first_frame_presented();
        assert_eq!(queue.next_open().unwrap().document_id, 499);
        assert_eq!(queue.next_open().unwrap().document_id, 0);
        assert!(queue.next_open().is_none());
        queue.completed(499, Ok(()));
        assert_eq!(queue.next_open().unwrap().document_id, 1);
    }
    #[test]
    fn untrusted_or_rejected_path_never_becomes_open_request() {
        let manifest = capture([captured(0)], Some(0), vec![], vec![]).unwrap();
        let mut queue = RestoreQueue::new(manifest).unwrap();
        queue.first_frame_presented();
        assert!(queue.next_open().is_none());
        queue.reject(0, "Trust denied".into());
        assert!(!queue.approve(0, PathBuf::from("unsafe")));
        assert!(queue.next_open().is_none());
        assert!(!queue.pending());
    }
}
