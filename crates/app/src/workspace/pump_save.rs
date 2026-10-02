// SPDX-License-Identifier: MPL-2.0
//! Save completions: retried cleanups, interrupted-save discovery and saved
//! documents (ARC-01).
use super::*;
use std::sync::mpsc::TryRecvError;
impl Workspace {
    /// Settle every retried save cleanup whose worker answered.
    pub(super) fn pump_save_cleanups(&mut self) -> bool {
        let mut changed = false;
        let mut cleanup = 0;
        while cleanup < self.pending_save_cleanup.len() {
            match self.pending_save_cleanup[cleanup].1.try_recv() {
                Err(TryRecvError::Empty) => cleanup += 1,
                received => {
                    let (transaction, _) = self.pending_save_cleanup.remove(cleanup);
                    changed = true;
                    match received {
                        Ok(IoCompletion::SaveCleanupRetried { cleanup, result: Ok(true) }) => {
                            self.save_cleanups.retain(|known| known.transaction != transaction);
                            if self.selected_save_cleanup.as_ref() == Some(&transaction) {
                                self.selected_save_cleanup = self
                                    .save_cleanups
                                    .first()
                                    .map(|cleanup| cleanup.transaction.clone());
                            }
                            self.message = Some("Saved recovery files were cleaned up.".into());
                            drop(cleanup);
                        }
                        Ok(IoCompletion::SaveCleanupRetried { cleanup, result }) => {
                            if !self.save_cleanups.iter().any(|known| known.transaction == transaction) {
                                self.record_save_cleanup(cleanup);
                            }
                            self.message = Some(match result {
                                Ok(false) => "Save cleanup was already completed.".into(),
                                Err(error) => format!("Saved document remains committed; recovery cleanup failed: {}", file_error(error)),
                                Ok(true) => unreachable!(),
                            });
                        }
                        _ => self.message = Some("Saved document remains committed; recovery cleanup stopped. Retry cleanup from File commands.".into()),
                    }
                }
            }
        }
        changed
    }
    /// Settle every interrupted-save inspection whose worker answered.
    pub(super) fn pump_save_recovery(&mut self) -> bool {
        let mut changed = false;
        let mut recovery = 0;
        while recovery < self.pending_save_recovery.len() {
            match self.pending_save_recovery[recovery].1.try_recv() {
                Err(TryRecvError::Empty) => recovery += 1,
                received => {
                    let (parent, _) = self.pending_save_recovery.remove(recovery);
                    changed = true;
                    match received {
                        Ok(IoCompletion::SaveRecoveryInspection { result: Ok(found), .. }) => {
                            self.failed_save_recovery.remove(&parent);
                            let count = self.merge_save_recovery(found.conflicts);
                            let cleanups = found.cleanups.len();
                            for cleanup in found.cleanups {
                                self.record_save_cleanup(cleanup);
                            }
                            if count != 0 {
                                self.message = Some(format!(
                                    "{count} interrupted save transaction(s) are available for compare, Save Elsewhere, or retention."
                                ));
                            } else if cleanups != 0 {
                                self.message = Some(format!(
                                    "{cleanups} interrupted cleanup transaction(s) are ready to retry."
                                ));
                            }
                        }
                        Ok(IoCompletion::SaveRecoveryInspection { result: Err(error), .. }) => {
                            self.scanned_save_recovery.remove(&parent);
                            self.failed_save_recovery.insert(parent.clone());
                            self.message = Some(format!(
                                "Could not check {} for interrupted saves ({}). Open documents are not affected; use File > Document > Recovery > Retry Save Recovery Discovery to check again.",
                                parent.display(),
                                file_error(error)
                            ));
                        }
                        _ => {
                            self.scanned_save_recovery.remove(&parent);
                            self.failed_save_recovery.insert(parent.clone());
                            self.message = Some(format!(
                                "Checking {} for interrupted saves stopped. Open documents are not affected; use File > Document > Recovery > Retry Save Recovery Discovery to check again.",
                                parent.display()
                            ));
                        }
                    }
                }
            }
        }
        changed
    }
    /// A save that committed: the document is clean at the captured revision
    /// unless it was a copy, and pending recovery-file cleanup is reported.
    pub(super) fn complete_save(
        &mut self,
        pending: PendingIo,
        saved: bareline_file_io::lifecycle::Saved,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        let cleanup = saved.cleanup.clone();
        if !pending.copy_only
            && let Some((tab, path, bom)) = pending.save
            && let Some(index) = self.tab_index(tab)
        {
            self.editors[index].mark_saved(&saved.captured);
            self.note_recent(path.clone());
            let encoding = self.tabs[index].file.as_ref().and_then(|file| file.encoding.clone());
            self.tabs[index].file = Some(FileState {
                binary_accepted: true,
                _lease: admission.take(),
                path,
                fingerprint: saved.fingerprint,
                bom,
                encoding,
            });
        }
        if let Some(cleanup) = cleanup {
            self.message = Some(format!(
                "Saved, but recovery-file cleanup is pending: {}",
                cleanup.error
            ));
            self.record_save_cleanup(cleanup);
        } else {
            self.message = None;
        }
    }
    /// An interrupted-save inspection that arrived through the file queue.
    pub(super) fn complete_queued_save_recovery(
        &mut self,
        result: Result<bareline_file_io::lifecycle::SaveRecovery, FileError>,
    ) {
        if let Ok(found) = result {
            self.merge_save_recovery(found.conflicts);
            for cleanup in found.cleanups {
                self.record_save_cleanup(cleanup);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::{PagedFileSystem, pending_io};

    fn saved(workspace: &Workspace) -> bareline_file_io::lifecycle::Saved {
        bareline_file_io::lifecycle::Saved {
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
        }
    }

    #[test]
    fn a_saved_copy_leaves_the_document_bound_to_its_own_file() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let target = std::env::temp_dir().join("bareline-handler-save.txt");
        let mut copy = pending_io(&mut workspace);
        copy.save = Some((workspace.tabs[0].id, target.clone(), false));
        copy.copy_only = true;
        workspace.message = Some("Saving…".into());
        let result = saved(&workspace);
        workspace.complete_save(copy, result, None);
        assert_eq!(workspace.message, None);
        assert_eq!(workspace.path(0), None);
        assert!(workspace.take_recent_events().is_empty());

        let mut save = pending_io(&mut workspace);
        save.save = Some((workspace.tabs[0].id, target.clone(), true));
        let result = saved(&workspace);
        workspace.complete_save(save, result, None);
        assert_eq!(workspace.message, None);
        assert_eq!(workspace.path(0), Some(target.as_path()));
        assert_eq!(workspace.take_recent_events(), [target]);
        assert!(
            workspace.tabs[0]
                .file
                .as_ref()
                .is_some_and(|file| file.bom && file.binary_accepted)
        );
    }

    #[test]
    fn a_queued_recovery_inspection_failure_changes_nothing() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.complete_queued_save_recovery(Err(FileError::Cancelled));
        workspace.complete_queued_save_recovery(Ok(bareline_file_io::lifecycle::SaveRecovery::default()));
        assert!(workspace.save_conflicts().is_empty());
        assert!(workspace.save_cleanups().is_empty());
        assert_eq!(workspace.message, None);
    }
}
