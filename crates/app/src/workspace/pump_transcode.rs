// SPDX-License-Identifier: MPL-2.0
//! Paged open (transcode) completions (ARC-01).
use super::*;
impl Workspace {
    /// A paged open that finished: Interpret As, reload and recovery restore
    /// take their own paths; otherwise the loading tab becomes the document.
    pub(super) fn complete_transcode(
        &mut self,
        pending: PendingIo,
        mut opened: Box<bareline_file_io::lifecycle::PagedOpened>,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        let launch_request = pending.launch_request;
        let recovery_restore_request = pending.recovery_restore_request;
        if self
            .interpreting_paged
            .as_ref()
            .is_some_and(|(_, path)| *path == opened.path)
        {
            self.complete_interpret(pending.reload.as_ref(), opened, admission);
            return;
        }
        if let Some(reload) = &pending.reload {
            self.complete_paged_reload(reload, opened, admission);
            return;
        }
        if opened.recovery_origin.is_none() {
            self.note_recent(opened.path.clone());
        }
        // The resident open's canonical identity check, so drag-drop,
        // Recent or another spelling cannot open a large file twice (PED-24).
        if !pending.allow_duplicate
            && opened.recovery_origin.is_none()
            && let Some(existing) = self.open_file_index(&opened.fingerprint.identity)
        {
            self.settle_duplicate_open(existing, pending.preview.as_ref(), launch_request);
            return;
        }
        // REC-07: restore fell back to an older checkpoint; say what was lost.
        let unrestored = opened.unrestored_revision.map(|revision| {
            format!(
                "Recovered an earlier checkpoint: the last protected change (revision {revision}) could not be restored."
            )
        });
        if opened.recovery_origin.is_some()
            && self.complete_recovered_resident(&pending, &mut opened, unrestored.as_deref())
        {
            return;
        }
        let preview = self.preview_index(pending.preview.as_ref());
        let path = opened.path.clone();
        let file = FileState {
            binary_accepted: false,
            _lease: admission.take(),
            path: opened.path.clone(),
            fingerprint: opened.fingerprint.clone(),
            bom: opened.transcoded.store.state.bom,
            encoding: None,
        };
        match self.new_paged_editor(opened) {
            Ok(mut editor) => {
                if let Some(root) = &self.recovery_root {
                    editor.enable_recovery(root.clone(), self.file_system.clone());
                }
                // The loading tab becomes the document in place (FIO-01).
                let index = match preview {
                    Some(index) => {
                        // Keep the binary guard; add the placeholder's choice.
                        let keep = self.editors[index].viewport().user_read_only;
                        editor.set_user_read_only(editor.user_read_only() || keep);
                        let old = std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(editor));
                        self.note_replaced(old.document_identity(), self.editors[index].document_identity());
                        self.retired.push(old);
                        self.files[index] = Some(file);
                        self.untitled_labels[index].clear();
                        self.find.clear_source();
                        index
                    }
                    None => {
                        self.editors.push(WorkspaceEditor::Paged(editor));
                        self.files.push(Some(file));
                        self.untitled_labels.push(String::new());
                        self.editors.len() - 1
                    }
                };
                // A restore fallback (REC-07) outranks, but never hides, the encoding hint (FIO-05).
                self.message = match (unrestored, self.encoding_hint(index)) {
                    (Some(unrestored), Some(hint)) => Some(format!("{unrestored} {hint}")),
                    (unrestored, hint) => unrestored.or(hint),
                };
                let document = self.editors[index].document_identity();
                self.record_launch_open(launch_request, Ok(document));
                self.record_recovery_restore(recovery_restore_request, Ok(document));
            }
            Err(error) => {
                self.settle_failed_open(pending.preview.as_ref(), Some(path), pending.keep_failed_tab, &error);
                self.message = Some(error.clone());
                self.record_launch_open(launch_request, Err(error.clone()));
                self.record_recovery_restore(recovery_restore_request, Err(error));
            }
        }
    }
    /// A paged open paused at the temporary disk limit keeps its tab for Resume.
    pub(super) fn complete_transcode_paused(
        &mut self,
        pending: PendingIo,
        paused: Box<bareline_file_io::lifecycle::PausedTranscode>,
    ) {
        let launch_request = pending.launch_request;
        let recovery_restore_request = pending.recovery_restore_request;
        let error = format!(
            "Opening paused: {}. Your file is unchanged; resume after increasing the temporary disk limit (Settings).",
            paused.error
        );
        // The kept tab shows the pause; Resume reopens into it in place.
        self.paused_tab = self.settle_failed_open(
            pending.preview.as_ref(),
            pending.open_path.clone(),
            pending.keep_failed_tab,
            &error,
        );
        self.message = Some(error.clone());
        self.record_launch_open(launch_request, Err(error.clone()));
        self.record_recovery_restore(recovery_restore_request, Err(error));
        self.paused_reload = pending.reload.clone();
        self.paused_transcode = Some(paused);
    }
    /// A paged open that failed or was cancelled.
    pub(super) fn complete_transcode_failed(&mut self, pending: PendingIo, error: FileError) {
        let launch_request = pending.launch_request;
        let recovery_restore_request = pending.recovery_restore_request;
        self.interpreting_paged = None;
        self.resume_abandoned_reload(pending.reload.as_ref());
        let cancelled = matches!(error, FileError::Cancelled);
        let error = file_error(error);
        self.settle_failed_open(
            pending.preview.as_ref(),
            pending.open_path.clone(),
            pending.keep_failed_tab && !cancelled,
            &error,
        );
        self.message = Some(error.clone());
        self.record_launch_open(launch_request, Err(error.clone()));
        self.record_recovery_restore(recovery_restore_request, Err(error));
    }
}
