// SPDX-License-Identifier: MPL-2.0
//! Recovery-restore completions (ARC-01).
use super::*;
impl Workspace {
    /// Publish each restored document whose recovered text became terminal.
    pub(super) fn pump_recovery_restore_publications(&mut self) -> bool {
        let mut changed = false;
        let mut publication = 0;
        while publication < self.pending_recovery_restore_publications.len() {
            let terminal = self.pending_recovery_restore_publications[publication]
                .receipt
                .terminal();
            let Some(terminal) = terminal else {
                publication += 1;
                continue;
            };
            let pending = self.pending_recovery_restore_publications.remove(publication);
            match terminal {
                Ok(revision)
                    if self
                        .editors
                        .iter()
                        .any(|editor| editor.document_identity().0 == pending.document_id) =>
                {
                    self.record_recovery_restore(Some(pending.request_id), Ok((pending.document_id, revision.0)));
                }
                Ok(_) => self.record_recovery_restore(
                    Some(pending.request_id),
                    Err("Recovered document closed before its text was published.".into()),
                ),
                Err(error) => self.record_recovery_restore(Some(pending.request_id), Err(error)),
            }
            changed = true;
        }
        changed
    }
    /// A recovered document that fits in memory is adopted as a resident tab.
    /// Returns false when the paged open must continue instead.
    pub(super) fn complete_recovered_resident(
        &mut self,
        pending: &PendingIo,
        opened: &mut bareline_file_io::lifecycle::PagedOpened,
        unrestored: Option<&str>,
    ) -> bool {
        let launch_request = pending.launch_request;
        let recovery_restore_request = pending.recovery_restore_request;
        match self.adopt_recovered_resident(opened) {
            Ok(Some((document_id, receipt))) => {
                // The adopted editor takes over the restore's loading
                // tab: its place in the shell, and the focus if it had it (PED-23).
                let adopted = self
                    .editors
                    .iter()
                    .position(|editor| editor.document_identity().0 == document_id)
                    .and_then(|index| self.tab_id(index));
                self.discard_preview_to(pending.preview.as_ref(), adopted);
                if let Some(unrestored) = unrestored {
                    self.message = Some(unrestored.to_owned());
                }
                // The restore the user asked for shows its tab (APP-07).
                if let Some(document) = self
                    .editors
                    .iter()
                    .map(WorkspaceEditor::document_identity)
                    .find(|document| document.0 == document_id)
                {
                    self.record_launch_open(launch_request, Ok(document));
                }
                if let Some(request_id) = recovery_restore_request {
                    self.pending_recovery_restore_publications
                        .push(PendingRecoveryRestorePublication {
                            request_id,
                            document_id,
                            receipt,
                        });
                }
                true
            }
            Err(error) => {
                self.discard_preview(pending.preview.as_ref());
                self.message = Some(error.clone());
                self.record_launch_open(launch_request, Err(error.clone()));
                self.record_recovery_restore(recovery_restore_request, Err(error));
                true
            }
            Ok(None) => false,
        }
    }
}
