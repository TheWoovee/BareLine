// SPDX-License-Identifier: MPL-2.0
//! Per-editor pump work and paged file-state updates (ARC-01).
use super::*;
impl Workspace {
    /// Apply resource policy to every editor, pump it, and settle its paged
    /// save terminals, conflicts and cleanups.
    pub(super) fn pump_editors(&mut self) -> bool {
        let mut changed = false;
        for (index, editor) in self.editors.iter_mut().enumerate() {
            let policy_ready = match editor {
                WorkspaceEditor::Resident(surface) => surface
                    .document_service()
                    .is_none_or(|service| service.configure_history_limit(self.undo_max_changes)),
                WorkspaceEditor::Paged(surface) => surface.configure_history_limit(self.undo_max_changes),
            };
            if !policy_ready {
                (self.notify)();
            }
            // A preview's journal belongs to the document it shows (REC-13).
            if let Some(root) = &self.recovery_root
                && self.tabs[index].lifecycle.owns_text()
            {
                match editor {
                    WorkspaceEditor::Resident(surface) => surface.enable_recovery(
                        root.clone(),
                        self.file_system.clone(),
                        self.tabs[index].file.as_ref().and_then(|file| file.encoding.clone()),
                        self.tabs[index].file.as_ref().map(|file| file.path.clone()),
                        self.bytes.clone(),
                    ),
                    WorkspaceEditor::Paged(surface) => surface.enable_recovery(root.clone(), self.file_system.clone()),
                }
            }
            if let WorkspaceEditor::Paged(paged) = editor {
                paged.set_streaming_quota(self.transcode_quota_bytes);
            }
            changed |= editor.pump();
            if let WorkspaceEditor::Paged(paged) = editor {
                while let Some(terminal) = paged.take_save_terminal() {
                    let owned = terminal.receipt.requested.document == terminal.owner.document.0
                        && self.pending_paged_saves.remove(&terminal.owner);
                    if !owned {
                        continue;
                    }
                    changed = true;
                    match terminal.receipt.terminal {
                        Ok(bareline_file_io::paged_service::PagedTerminalOutcome::Saved { .. }) => {
                            let resident_save_pending = self.pending_io.iter().any(|pending| pending.save.is_some());
                            if self.pending_paged_saves.is_empty()
                                && !resident_save_pending
                                && self.message.as_deref() == Some("Saving…")
                            {
                                self.message = None;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            if let bareline_file_io::paged_service::PagedLifecycleError::Failed(error) = &error {
                                Self::record_deleted_destination(&mut self.deleted_destinations, error);
                            }
                            self.message = Some(error.to_string());
                        }
                    }
                }
                if let Some(error) = &paged.error {
                    self.message = Some(error.clone());
                }
                if let Some(conflict) = paged.take_save_conflict() {
                    upsert_save_issue(&mut self.save_conflicts, conflict, |issue| &issue.transaction);
                }
                if let Some(cleanup) = paged.take_save_cleanup() {
                    self.message = Some(format!(
                        "Saved, but recovery-file cleanup is pending: {}",
                        cleanup.error
                    ));
                    self.selected_save_cleanup = Some(cleanup.transaction.clone());
                    upsert_save_issue(&mut self.save_cleanups, cleanup, |issue| &issue.transaction);
                }
            }
        }
        let active_paged_saves: std::collections::BTreeSet<_> = self
            .editors
            .iter()
            .filter_map(|editor| match editor {
                WorkspaceEditor::Paged(paged) => paged.pending_save_owner(),
                WorkspaceEditor::Resident(_) => None,
            })
            .collect();
        self.pending_paged_saves
            .retain(|owner| active_paged_saves.contains(owner));
        changed
    }
    /// A paged editor that saved or moved re-registers its file, so Replace in
    /// Files and duplicate opens see the path and identity it now holds.
    pub(super) fn sync_paged_files(&mut self) {
        let mut saved_paths = Vec::new();
        for (index, editor) in self.editors.iter().enumerate() {
            if let WorkspaceEditor::Paged(paged) = editor
                && let Some(file) = &mut self.tabs[index].file
                && (file.path != paged.path() || file.fingerprint != paged.fingerprint())
            {
                let path = paged.path();
                let fingerprint = paged.fingerprint();
                match self
                    .replacement_registry
                    .try_register(path.clone(), &fingerprint.identity)
                {
                    Ok(lease) => {
                        if !paged.save_as_required() {
                            saved_paths.push(path.clone());
                        }
                        file._lease = Some(lease);
                        file.path = path;
                        file.fingerprint = fingerprint;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => self.message = Some(format!("File admission update failed: {error}")),
                }
            }
        }
        for path in saved_paths {
            self.note_recent(path);
        }
    }
}
