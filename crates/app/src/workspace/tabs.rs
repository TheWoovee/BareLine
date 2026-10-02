// SPDX-License-Identifier: MPL-2.0
//! Stable tab identity (P6-01, ARC-02). A tab keeps one [`TabId`] from the
//! moment it appears until it closes, while its document may be replaced in
//! place: an open that finishes in its loading or failed tab, Reload or
//! Interpret As. The per-tab state beside each editor lives in one
//! [`TabSlot`], pending saves name their tab by id rather than by position,
//! and a shell follows the active tab by id across a pump, so a tab that
//! finishes, fails or closes elsewhere never moves the user (PED-23).
use super::*;

/// A tab's identity while it is open. Ids count up and are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

/// A change to the tab list, oldest first (P6-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    /// A tab showing `document` was added at the end of the tab list.
    Inserted { tab: TabId, document: u64 },
    /// The tab kept its place while document `old` was replaced by `new`.
    Replaced { tab: TabId, old: u64, new: u64 },
    /// The tab showing `document` was removed. `successor` is the tab that took
    /// over its content, such as the tab already holding the file a duplicate
    /// open was loading (PED-24).
    Removed {
        tab: TabId,
        document: u64,
        successor: Option<TabId>,
    },
}

/// What a shell captured for each tab before a pump, to follow its active tab
/// across it: the tab's [`TabId`], or the document id the tab showed.
pub trait TabKey: Copy {
    /// The tab this key named, if the workspace still knows it.
    fn tab(self, workspace: &Workspace) -> Option<TabId>;
}
impl TabKey for TabId {
    fn tab(self, _: &Workspace) -> Option<TabId> {
        Some(self)
    }
}
/// A document id names the tab that shows it, or showed it before it was
/// replaced or removed.
impl TabKey for u64 {
    fn tab(self, workspace: &Workspace) -> Option<TabId> {
        workspace.tab_of_document(self)
    }
}

/// The per-tab state kept beside `Workspace::editors`, in the same order. One
/// record per tab replaces the parallel per-position vectors (ARC-02).
pub(super) struct TabSlot {
    pub(super) id: TabId,
    /// The file the tab's document is bound to.
    pub(super) file: Option<FileState>,
    /// Title of a tab without a file, such as "Untitled 1" or "name (loading)".
    pub(super) label: String,
    /// Where the tab's document stands in its file's life (P6-02). It changes
    /// only through [`lifecycle::transition`].
    pub(super) lifecycle: FileLifecycle,
}

/// Tab-list changes kept to resolve tabs and documents captured before a pump.
/// Every added tab is logged too, so the bound leaves room for a session
/// restore's burst of tabs within one pump. Document and tab ids are never
/// reused, so an evicted entry only means an old capture no longer resolves.
const MAX_TAB_EVENTS: usize = 1024;

#[derive(Default)]
pub(super) struct TabLog {
    /// `(sequence, event)`, oldest first.
    events: std::collections::VecDeque<(u64, TabEvent)>,
    next: u64,
    /// The last sequence handed out by [`Workspace::take_tab_events`].
    taken: u64,
}
impl TabLog {
    fn record(&mut self, event: TabEvent) {
        if self.events.len() == MAX_TAB_EVENTS {
            self.events.pop_front();
        }
        self.next += 1;
        self.events.push_back((self.next, event));
    }
    fn events(&self) -> impl DoubleEndedIterator<Item = &TabEvent> {
        self.events.iter().map(|(_, event)| event)
    }
}

impl Workspace {
    /// Add a tab at the end of the tab list and return its position. `event`
    /// says how the tab came to be; a creation the lifecycle refuses falls back
    /// to the state the tab's file binding implies.
    pub(super) fn push_tab(
        &mut self,
        editor: WorkspaceEditor,
        file: Option<FileState>,
        label: String,
        event: LifecycleEvent,
    ) -> usize {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        let document = editor.document_identity().0;
        let facts = TabFacts {
            bound: file.is_some(),
            dirty: editor.dirty(),
        };
        let lifecycle = lifecycle::transition(FileLifecycle::Closed, event, facts)
            .unwrap_or_else(|_| FileLifecycle::settled(facts));
        self.editors.push(editor);
        self.tabs.push(TabSlot {
            id,
            file,
            label,
            lifecycle,
        });
        self.tab_log.record(TabEvent::Inserted { tab: id, document });
        self.editors.len() - 1
    }
    /// Remove the tab at `index`. `successor` is the tab that takes over its
    /// content, if any.
    pub(super) fn remove_tab(&mut self, index: usize, successor: Option<TabId>) -> (WorkspaceEditor, TabSlot) {
        let editor = self.editors.remove(index);
        let slot = self.tabs.remove(index);
        self.tab_log.record(TabEvent::Removed {
            tab: slot.id,
            document: editor.document_identity().0,
            successor,
        });
        (editor, slot)
    }
    /// Record a tab whose document was replaced in place (PED-23): an open that
    /// finished in its loading or failed tab, a Reload or Interpret As. `old`
    /// is the document the tab at `index` showed before. The shell keeps that
    /// tab's position, pin and colour for the new document.
    pub(super) fn note_tab_replaced(&mut self, index: usize, old: (u64, u64)) {
        let (Some(slot), Some(editor)) = (self.tabs.get(index), self.editors.get(index)) else {
            return;
        };
        let (tab, new) = (slot.id, editor.document_identity().0);
        if old.0 != new {
            self.tab_log.record(TabEvent::Replaced { tab, old: old.0, new });
        }
    }
    /// The tab at `index` as the lifecycle sees it now.
    pub(super) fn tab_facts(&self, index: usize) -> TabFacts {
        TabFacts {
            bound: self.tabs.get(index).is_some_and(|tab| tab.file.is_some()),
            dirty: self.editors.get(index).is_some_and(WorkspaceEditor::dirty),
        }
    }
    /// Move the tab at `index` through `event`, given the tab as it is now. A
    /// refused event leaves the tab's state unchanged.
    pub(super) fn apply_lifecycle(
        &mut self,
        index: usize,
        event: LifecycleEvent,
    ) -> Result<FileLifecycle, LifecycleRefusal> {
        let facts = self.tab_facts(index);
        let tab = self.tabs.get_mut(index).ok_or(LifecycleRefusal::Illegal)?;
        tab.lifecycle = lifecycle::transition(tab.lifecycle, event, facts)?;
        Ok(tab.lifecycle)
    }
    /// The file lifecycle of the tab at `index` (P6-02).
    pub fn lifecycle(&self, index: usize) -> Option<FileLifecycle> {
        self.tabs.get(index).map(|tab| tab.lifecycle)
    }
    /// Each tab's id, in tab order. A shell captures it before [`Self::pump`]
    /// for [`Self::active_after_pump`].
    pub fn tab_ids(&self) -> Vec<TabId> {
        self.tabs.iter().map(|tab| tab.id).collect()
    }
    /// The id of the tab at `index`.
    pub fn tab_id(&self, index: usize) -> Option<TabId> {
        self.tabs.get(index).map(|tab| tab.id)
    }
    /// The position of an open tab.
    pub fn tab_index(&self, tab: TabId) -> Option<usize> {
        self.tabs.iter().position(|slot| slot.id == tab)
    }
    /// Tab-list changes since the last call, oldest first. Changes older than
    /// the bounded log are dropped.
    pub fn take_tab_events(&mut self) -> Vec<TabEvent> {
        let taken = self.tab_log.taken;
        self.tab_log.taken = self.tab_log.next;
        self.tab_log
            .events
            .iter()
            .filter(|(sequence, _)| *sequence > taken)
            .map(|(_, event)| *event)
            .collect()
    }
    /// The tab that shows `document`, or showed it before it was replaced or
    /// removed.
    fn tab_of_document(&self, document: u64) -> Option<TabId> {
        if let Some(index) = self
            .editors
            .iter()
            .position(|editor| editor.document_identity().0 == document)
        {
            return self.tab_id(index);
        }
        self.tab_log.events().rev().find_map(|event| match *event {
            TabEvent::Replaced { tab, old, .. } if old == document => Some(tab),
            TabEvent::Removed {
                tab, document: removed, ..
            } if removed == document => Some(tab),
            _ => None,
        })
    }
    /// `tab`, or the tab that took over its content when it was removed.
    fn resolve_tab(&self, mut tab: TabId) -> TabId {
        for _ in 0..self.tab_log.events.len() {
            let successor = self.tab_log.events().find_map(|event| match *event {
                TabEvent::Removed {
                    tab: removed,
                    successor,
                    ..
                } if removed == tab => successor,
                _ => None,
            });
            match successor {
                Some(next) => tab = next,
                None => break,
            }
        }
        tab
    }
    /// The document id whose tab `document` became through opens that finished
    /// in place, or `document` itself when its tab was not replaced.
    pub fn replacement_document(&self, document: u64) -> u64 {
        self.tab_of_document(document)
            .map(|tab| self.resolve_tab(tab))
            .and_then(|tab| self.tab_index(tab))
            .and_then(|index| self.editors.get(index))
            .map_or(document, |editor| editor.document_identity().0)
    }
    /// Each tab's document id, in tab order.
    pub fn tab_documents(&self) -> Vec<u64> {
        self.editors.iter().map(|editor| editor.document_identity().0).collect()
    }
    /// The tab to show after a pump that began with `before` tabs while the
    /// shell showed tab `active`: the tab an explicit open or restore asked
    /// for (APP-07), otherwise the tab the active one became. A tab keeps its
    /// id when its document is replaced in place, so a tab added in the
    /// background never takes focus, a loading tab that finishes, fails or
    /// closes elsewhere never changes the active tab, and a duplicate open's
    /// tab resolves to the tab already holding the file (PED-23, PED-24).
    /// Consumes the pending activation.
    pub fn active_after_pump<K: TabKey>(&mut self, before: &[K], active: usize) -> usize {
        let shown = match before.get(active) {
            Some(key) => key.tab(self),
            None => None,
        };
        if let Some(index) = self.take_tab_activation(shown) {
            return index;
        }
        let last = self.editors.len().saturating_sub(1);
        shown
            .map(|tab| self.resolve_tab(tab))
            .and_then(|tab| self.tab_index(tab))
            .unwrap_or(active)
            .min(last)
    }
    /// The tab an explicit open or restore asked to show, once it exists.
    /// `active` is the document the shell had active before the pump: a result
    /// that replaced a loading tab the user has since left is not shown (APP-07).
    /// Both sides are followed through tabs that finished in place, and the
    /// requested tab may itself have finished in place since it asked (PED-23).
    pub fn take_activation(&mut self, active: Option<u64>) -> Option<usize> {
        self.take_activation_for(active)
    }
    /// [`Self::take_activation`] for whatever the shell captured before the
    /// pump: the active tab's id or its document id.
    pub fn take_activation_for<K: TabKey>(&mut self, active: Option<K>) -> Option<usize> {
        let active = match active {
            Some(key) => key.tab(self),
            None => None,
        };
        self.take_tab_activation(active)
    }
    /// [`Self::take_activation`] where `active` is the tab the shell had active
    /// before the pump.
    pub fn take_tab_activation(&mut self, active: Option<TabId>) -> Option<usize> {
        let activation = self.activation.take()?;
        if let Some(from) = activation.from {
            let from = self.tab_of_document(from).map(|tab| self.resolve_tab(tab));
            if from.is_none() || active.map(|tab| self.resolve_tab(tab)) != from {
                return None;
            }
        }
        let tab = self.resolve_tab(self.tab_of_document(activation.document)?);
        self.tab_index(tab)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{PagedFileSystem, pending_io};

    fn fixture(tabs: usize) -> Workspace {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        for _ in 0..tabs {
            workspace.new_document().unwrap();
        }
        workspace
    }

    /// Replace the document of the tab at `index` in place, as a finished open does.
    fn replace_in_place(workspace: &mut Workspace, index: usize) {
        workspace.new_document().unwrap();
        let (replacement, _) = workspace.remove_tab(workspace.editors.len() - 1, None);
        let old = std::mem::replace(&mut workspace.editors[index], replacement);
        workspace.note_tab_replaced(index, old.document_identity());
        workspace.retired.push(old);
    }

    #[test]
    fn a_tab_keeps_its_id_in_place_and_ids_are_never_reused() {
        let mut workspace = fixture(2);
        let ids = workspace.tab_ids();
        assert_ne!(ids[0], ids[1]);
        let old = workspace.editors[1].document_identity().0;
        replace_in_place(&mut workspace, 1);
        let new = workspace.editors[1].document_identity().0;
        assert_ne!(old, new);
        assert_eq!(workspace.tab_ids(), ids, "an in-place replacement keeps the tab");
        assert_eq!(workspace.replacement_document(old), new);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        workspace.new_document().unwrap();
        let reopened = workspace.tab_ids();
        assert_eq!(reopened[0], ids[1]);
        assert!(!ids.contains(&reopened[1]), "a closed tab's id came back");
        let events = workspace.take_tab_events();
        assert!(matches!(
            events.as_slice(),
            [
                TabEvent::Inserted { .. },
                TabEvent::Inserted { .. },
                // The replacement editor's own short-lived tab.
                TabEvent::Inserted { .. },
                TabEvent::Removed { successor: None, .. },
                TabEvent::Replaced { .. },
                TabEvent::Removed { successor: None, .. },
                TabEvent::Inserted { .. },
            ]
        ));
        assert!(matches!(
            events[4],
            TabEvent::Replaced { tab, old: from, new: to } if tab == ids[1] && from == old && to == new
        ));
        assert!(workspace.take_tab_events().is_empty(), "events are handed out once");
    }

    #[test]
    fn the_active_tab_follows_its_id_across_a_pump() {
        let mut workspace = fixture(3);
        let ids = workspace.tab_ids();
        let documents = workspace.tab_documents();
        // An in-place replacement of the active tab keeps it active.
        replace_in_place(&mut workspace, 1);
        assert_eq!(workspace.active_after_pump(&ids, 1), 1);
        assert_eq!(workspace.active_after_pump(&documents, 1), 1);
        // A tab closed before the active one moves it left, not to another tab.
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(0, false, &mut renderer).unwrap();
        assert_eq!(workspace.active_after_pump(&ids, 1), 0);
        assert_eq!(workspace.active_after_pump(&documents, 1), 0);
        // A removed tab whose content another tab took over resolves to it.
        let removed = workspace.editors[1].snapshot().clone();
        workspace.discard_preview_to(Some(&removed), Some(ids[1]));
        assert_eq!(workspace.active_after_pump(&ids, 2), 0);
        assert_eq!(
            workspace.replacement_document(documents[2]),
            workspace.editors[0].document_identity().0
        );
    }

    #[test]
    fn a_pending_save_names_its_tab_by_id() {
        let mut workspace = fixture(3);
        let target = std::env::temp_dir().join("bareline-tab-save-target.txt");
        let mut save = pending_io(&mut workspace);
        save.save = Some((workspace.tabs[2].id, target.clone(), false));
        workspace.pending_io.push(save);
        workspace
            .apply_lifecycle(2, LifecycleEvent::SaveStarted { copy: false })
            .unwrap();
        assert!(workspace.document_busy(2));
        assert!(!workspace.document_busy(1));
        // Removing earlier tabs needs no bookkeeping for the save.
        workspace.remove_tab(0, None);
        workspace.remove_tab(0, None);
        assert!(workspace.document_busy(0));
        let save = workspace.pending_io.pop().unwrap();
        let saved = bareline_file_io::lifecycle::Saved {
            fingerprint: Fingerprint {
                identity: bareline_platform::FileIdentity {
                    volume: 1,
                    file: 2,
                    length: 0,
                    modified: 0,
                },
                sha256: [0; 32],
            },
            captured: workspace.editors[0].snapshot().clone(),
            cleanup: None,
        };
        workspace.complete_save(save, saved, None);
        assert_eq!(workspace.path(0), Some(target.as_path()));
        assert_eq!(workspace.lifecycle(0), Some(FileLifecycle::Loaded));
        assert!(!workspace.document_busy(0));
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "bareline-lifecycle-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        directory
    }

    /// P6-02: a save moves its tab through `Saving`, which refuses a second
    /// save and the close that would release the file's lease, and binds the
    /// tab to its file when it settles.
    #[test]
    fn a_save_holds_its_tab_in_saving_until_it_settles() {
        let directory = scratch("save");
        let mut workspace = fixture(1);
        assert_eq!(workspace.lifecycle(0), Some(FileLifecycle::Untitled));
        let path = directory.join("saved.txt");
        assert!(workspace.save(0, path.clone()), "{:?}", workspace.message);
        assert_eq!(
            workspace.lifecycle(0),
            Some(FileLifecycle::Saving {
                from: SaveOrigin::Untitled
            })
        );
        assert!(workspace.document_busy(0));
        assert!(!workspace.save(0, path.clone()));
        assert_eq!(workspace.message.as_deref(), Some("This document is already saving."));
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        assert_eq!(workspace.close(0, true, &mut renderer), Err(CloseError::Busy));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert_eq!(
            workspace.lifecycle(0),
            Some(FileLifecycle::Loaded),
            "{:?}",
            workspace.message
        );
        assert_eq!(workspace.path(0), Some(path.as_path()));
        assert!(!workspace.document_busy(0));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// P6-02/REC-13: a compare view is a preview: it never binds a recovery
    /// journal, is not in Save All, is never discarded, saves only as a copy
    /// and comes back from Restore Closed Tab as a preview.
    #[test]
    fn a_preview_owns_no_text_and_restores_as_a_preview() {
        let directory = scratch("preview");
        let mut workspace = fixture(1);
        workspace.recovery_root = Some(directory.join("recovery"));
        let source = workspace.editors[0].snapshot().clone();
        let index = workspace.add_snapshot_preview(&source, "Compare".into()).unwrap();
        assert_eq!(workspace.lifecycle(index), Some(FileLifecycle::Preview));
        workspace.pump();
        assert!(workspace.save_all_targets().iter().all(|(target, _)| *target != index));
        assert_eq!(
            workspace.discard_recoveries(&[index]),
            bareline_file_io::recovery_retirement::DiscardPoll::Durable
        );
        assert!(!workspace.save(index, directory.join("preview.txt")));
        assert_eq!(
            workspace.message.as_deref(),
            Some("Document is not ready or is read only; saving is unavailable.")
        );
        assert_eq!(workspace.lifecycle(index), Some(FileLifecycle::Preview));
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(index, true, &mut renderer).unwrap();
        let restored = workspace.restore_last_closed().expect("the preview is retained");
        assert_eq!(workspace.lifecycle(restored), Some(FileLifecycle::Preview));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// P6-02: a failed open's tab is `Failed`, and a retry takes it back to
    /// `Loading` in place, where nothing can be saved.
    #[test]
    fn a_failed_open_retries_through_loading_in_its_own_tab() {
        let directory = scratch("failed");
        let mut workspace = fixture(0);
        let path = directory.join("missing.txt");
        workspace.open(path.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.io_busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
        assert_eq!(workspace.failed_open(0).map(|(failed, _)| failed), Some(path.as_path()));
        assert_eq!(workspace.lifecycle(0), Some(FileLifecycle::Failed));
        let tab = workspace.tab_id(0);
        workspace.retry_failed_open(0).unwrap();
        assert_eq!(workspace.lifecycle(0), Some(FileLifecycle::Loading));
        assert_eq!(workspace.tab_id(0), tab, "the retry keeps the tab");
        assert!(!workspace.save(0, directory.join("copy.txt")));
        drop(workspace);
        let _ = std::fs::remove_dir_all(directory);
    }
}
