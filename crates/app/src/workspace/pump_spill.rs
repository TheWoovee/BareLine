// SPDX-License-Identifier: MPL-2.0
//! Memory-pressure spill completions (ARC-01).
use super::*;
impl Workspace {
    /// A resident document moved to large-file storage replaces its tab with a
    /// paged editor, unless the document, its bytes or its view changed.
    pub(super) fn complete_spill(
        &mut self,
        captured: bareline_document::DocumentSnapshot,
        result: Result<
            (
                bareline_file_io::codecs::disk::PagedTranscoded,
                Option<bareline_document::spill::PreparedSpill>,
            ),
            FileError,
        >,
    ) {
        self.spill_pending = false;
        self.spill_document = None;
        // A spill whose document was closed meanwhile is not a storage
        // failure, so it must not pause automatic spilling.
        let target_open = self
            .editors
            .iter()
            .any(|editor| editor.snapshot().same_document(&captured));
        match result {
            Err(_) if !target_open => {}
            Err(error) => {
                self.spill_paused = true;
                self.message = Some(format!("Memory spill paused; document retained: {}", file_error(error)));
            }
            Ok((mut transcoded, prepared)) => {
                let preserving_history = prepared.is_some();
                let index = self.editors.iter().position(|editor| {
                    matches!(editor, WorkspaceEditor::Resident(_)) && editor.snapshot().same_document(&captured)
                });
                if let Some(index) = index
                    && !self.document_busy(index)
                    && self.editors[index].snapshot().revision == captured.revision
                    && self
                        .editors
                        .iter()
                        .filter(|editor| editor.snapshot().same_document(&captured))
                        .count()
                        == 1
                    && (preserving_history
                        || self.files[index]
                            .as_ref()
                            .is_some_and(|file| transcoded.store.fingerprint.sha256 == file.fingerprint.sha256))
                {
                    let selection = self.editors[index].viewport().selection;
                    let migration = match prepared {
                        Some(prepared) => self.editors[index]
                            .document_service()
                            .ok_or(bareline_document::Error::ActorBusy)
                            .and_then(|service| service.migrate_spill(prepared)),
                        None => self.editors[index].migrate_clean_spill(&captured, transcoded.source.source()),
                    };
                    match migration {
                        Err(error) => {
                            self.spill_paused = true;
                            self.message = Some(format!(
                                "Moving the document out of memory stopped: {error}. The document stays open in memory."
                            ));
                        }
                        Ok(document) => {
                            transcoded.document = document;
                            let opened = Box::new(bareline_file_io::lifecycle::PagedOpened {
                                recovery_origin: None,
                                recovered_resident: None,
                                unrestored_revision: None,
                                path: self.files[index]
                                    .as_ref()
                                    .map_or_else(|| PathBuf::from("Untitled"), |file| file.path.clone()),
                                fingerprint: self.files[index].as_ref().map_or_else(
                                    || transcoded.store.fingerprint.clone(),
                                    |file| file.fingerprint.clone(),
                                ),
                                transcoded,
                            });
                            match self.new_paged_editor(opened) {
                                Err(error) => {
                                    let _ = self.editors[index].cancel_clean_spill(&captured);
                                    self.spill_paused = true;
                                    self.message = Some(error);
                                }
                                Ok(mut paged) => {
                                    if self.files[index].is_none() {
                                        paged.require_save_as();
                                    }
                                    paged.set_streaming_quota(self.transcode_quota_bytes);
                                    self.editors[index].copy_presentation_to(paged.viewport_mut());
                                    if let Some(root) = &self.recovery_root {
                                        paged.enable_recovery(root.clone(), self.file_system.clone());
                                    }
                                    self.spill_selection =
                                        Some((paged.snapshot().clone(), selection.anchor, selection.caret, None));
                                    let old =
                                        std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(paged));
                                    self.retired.push(old);
                                    if let Some(file) = self.files[index].as_mut() {
                                        file.encoding = None;
                                    }
                                    self.refresh_encoding_open(index);
                                    self.find.clear_source();
                                    self.last_drawn = None;
                                    // The move to large-file storage is an internal
                                    // detail; clear the transient status instead of
                                    // showing a banner about it (UX-04/UX-60).
                                    self.message = None;
                                    // The promoted editor now answers for itself.
                                    self.promotion_target = None;
                                }
                            }
                        }
                    }
                } else if target_open {
                    self.spill_paused = true;
                    self.message = Some("Memory spill was not attached because the document, original bytes, or view ownership changed; current data was retained.".into());
                }
            }
        }
    }
    /// Restore a spilled document's selection in its new paged editor.
    pub(super) fn restore_spill_selection(&mut self) {
        if let Some((snapshot, anchor, caret, token)) = self.spill_selection.take()
            && let Some(WorkspaceEditor::Paged(editor)) = self.editors.iter_mut().find(
                |editor| matches!(editor, WorkspaceEditor::Paged(paged) if paged.snapshot().same_document(&snapshot)),
            )
        {
            if editor.snapshot().content_state != snapshot.content_state {
                self.message = Some("Document changed before its promoted selection was restored.".into());
            } else if editor.busy() {
                self.spill_selection = Some((snapshot, anchor, caret, token));
            } else if let Some(token) = token {
                use bareline_editor_surface::paged_view::SelectionRestoreStatus;
                match editor.selection_restore_status(token) {
                    SelectionRestoreStatus::Pending => {
                        self.spill_selection = Some((snapshot, anchor, caret, Some(token)))
                    }
                    SelectionRestoreStatus::Applied => {}
                    SelectionRestoreStatus::Failed(error) => self.message = Some(error),
                    SelectionRestoreStatus::Superseded => {
                        self.message = Some("Promoted selection restoration was superseded.".into())
                    }
                }
            } else {
                match editor.restore_global_selection(
                    bareline_document::TextOffset(anchor),
                    bareline_document::TextOffset(caret),
                    false,
                ) {
                    Ok(token) => self.spill_selection = Some((snapshot, anchor, caret, Some(token))),
                    Err(error) => self.message = Some(error),
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::PagedFileSystem;

    #[test]
    fn a_failed_spill_pauses_spilling_only_while_its_document_is_open() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let captured = workspace.editors[0].snapshot().clone();
        workspace.spill_pending = true;
        workspace.spill_document = Some(captured.identity_token().0);
        workspace.complete_spill(captured.clone(), Err(FileError::Cancelled));
        assert!(!workspace.spill_pending);
        assert_eq!(workspace.spill_document, None);
        assert!(workspace.spill_paused);
        assert_eq!(
            workspace.message,
            Some(format!(
                "Memory spill paused; document retained: {}",
                file_error(FileError::Cancelled)
            ))
        );
        assert!(matches!(workspace.editors.as_slice(), [WorkspaceEditor::Resident(_)]));

        // A spill whose document closed meanwhile is not a storage failure.
        let mut closed = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        closed.spill_pending = true;
        closed.complete_spill(captured, Err(FileError::Cancelled));
        assert!(!closed.spill_pending);
        assert!(!closed.spill_paused);
        assert_eq!(closed.message, None);
    }
}
