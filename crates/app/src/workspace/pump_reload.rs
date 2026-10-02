// SPDX-License-Identifier: MPL-2.0
//! Reload and Interpret As completions (ARC-01).
use super::*;
impl Workspace {
    /// Whether the tab at `index` may take a file's text in place of its own:
    /// text with unsaved edits only once its recovery was durably discarded,
    /// which `gate_reload` does first (REC-04, P6-02).
    fn may_replace_text(&self, index: usize, discarding: bool) -> bool {
        let unretired_edits = self.editors[index].dirty() && !discarding;
        let replaced = TabFacts {
            bound: true,
            dirty: false,
        };
        lifecycle::transition(
            self.tabs[index].lifecycle,
            LifecycleEvent::Opened { unretired_edits },
            replaced,
        ) != Err(LifecycleRefusal::UnretiredEdits)
    }
    /// A resident reload replaces its unchanged tab with a fresh editor.
    pub(super) fn complete_resident_reload(
        &mut self,
        reload: &PendingReload,
        opened: bareline_file_io::lifecycle::Opened,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        if let Some(index) = self.reload_index(&reload.target)
            && matches!(self.editors[index], WorkspaceEditor::Resident(_))
            && !self.editors[index].busy()
            && self.may_replace_text(index, reload.discarding)
        {
            // Like close plus reopen, the reloaded text gets a fresh
            // editor: undo history, bookmarks, folds and marks belonged
            // to the replaced text (WSP-11), and the next pump binds a
            // fresh recovery owner to the new document (REC-04).
            let snapshot = opened.document.snapshot();
            let mut editor = EditorSurface::new(
                self.scheduler.document(opened.document, 32),
                snapshot,
                self.notify.clone(),
            );
            self.editors[index].viewport().copy_view_settings_to(&mut editor);
            editor.user_read_only = self.editors[index].viewport().user_read_only;
            let old = std::mem::replace(&mut self.editors[index], editor.into());
            // The tab keeps its position, pin and colour for the
            // reloaded document; only its view starts over (PED-23).
            self.note_tab_replaced(index, old.document_identity());
            self.retired.push(old);
            self.tabs[index].file = Some(FileState {
                binary_accepted: false,
                _lease: admission.take(),
                path: opened.path,
                fingerprint: opened.fingerprint,
                bom: opened.bom,
                encoding: opened.encoding,
            });
            let _ = self.apply_lifecycle(index, LifecycleEvent::Opened { unretired_edits: false });
            self.refresh_encoding_open(index);
            self.find.clear_source();
            self.message = Some("Reloaded from disk.".into());
        } else {
            self.resume_abandoned_reload(Some(reload));
            self.message = Some("Document changed while reloading; current edits were preserved.".into());
        }
    }
    /// A paged Interpret As replaces the unchanged paged tab it was started for.
    pub(super) fn complete_interpret(
        &mut self,
        reload: Option<&PendingReload>,
        opened: Box<bareline_file_io::lifecycle::PagedOpened>,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        let (captured, _) = self.interpreting_paged.take().unwrap();
        let index = self.editors.iter().position(|editor| matches!(editor, WorkspaceEditor::Paged(editor) if editor.snapshot().same_document(&captured) && editor.snapshot().revision == captured.revision));
        let discarding = reload.is_some_and(|reload| reload.discarding);
        if let Some(index) = index.filter(|index| self.may_replace_text(*index, discarding)) {
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
                    let old = std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(editor));
                    // The tab stays where it is for the reinterpreted text (PED-23).
                    self.note_tab_replaced(index, old.document_identity());
                    self.retired.push(old);
                    self.tabs[index].file = Some(file);
                    let _ = self.apply_lifecycle(index, LifecycleEvent::Opened { unretired_edits: false });
                    self.find.clear_source();
                    self.message = Some("Original bytes reinterpreted.".into());
                }
                Err(error) => {
                    self.resume_abandoned_reload(reload);
                    self.message = Some(error);
                }
            }
        } else {
            self.resume_abandoned_reload(reload);
            self.message = Some("Document changed while interpreting; current edits retained.".into());
        }
    }
    /// A paged reload, or a resident Interpret As beyond the resident limits,
    /// replaces its unchanged tab with a fresh paged editor.
    pub(super) fn complete_paged_reload(
        &mut self,
        reload: &PendingReload,
        opened: Box<bareline_file_io::lifecycle::PagedOpened>,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        if let Some(index) = self.reload_index(&reload.target)
            && !self.editors[index].busy()
            && self.may_replace_text(index, reload.discarding)
            // A paged Interpret As rereads the file, which must
            // still hold the bytes that were opened (FIO-01).
            && (reload.interpret.is_none()
                || self.tabs[index]
                    .file
                    .as_ref()
                    .is_some_and(|file| file.fingerprint.sha256 == opened.fingerprint.sha256))
        {
            let read_only = self.editors[index].viewport().user_read_only;
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
                    // Only view preferences carry over; folds and marks
                    // belonged to the replaced text (WSP-11).
                    self.editors[index]
                        .viewport()
                        .copy_view_settings_to(editor.viewport_mut());
                    // A paged view cannot overwrite, so it returns to Insert (UI-07).
                    editor.viewport_mut().overwrite = false;
                    editor.set_user_read_only(read_only);
                    if let Some(root) = &self.recovery_root {
                        editor.enable_recovery(root.clone(), self.file_system.clone());
                    }
                    let old = std::mem::replace(&mut self.editors[index], WorkspaceEditor::Paged(editor));
                    // The tab stays where it is for the reloaded text (PED-23).
                    self.note_tab_replaced(index, old.document_identity());
                    self.retired.push(old);
                    self.tabs[index].file = Some(file);
                    let _ = self.apply_lifecycle(index, LifecycleEvent::Opened { unretired_edits: false });
                    self.refresh_encoding_open(index);
                    self.find.clear_source();
                    self.message = Some("Reloaded from disk.".into());
                }
                Err(error) => {
                    self.resume_abandoned_reload(Some(reload));
                    self.message = Some(error);
                }
            }
        } else {
            self.resume_abandoned_reload(Some(reload));
            self.message = Some(if reload.interpret.is_some() {
                "The file or document changed before Interpret As finished; nothing was replaced. Reload, then choose the encoding again.".into()
            } else {
                "Document changed while reloading; current edits were preserved.".into()
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::PagedFileSystem;

    fn opened(workspace: &Workspace, path: &std::path::Path) -> bareline_file_io::lifecycle::Opened {
        bareline_file_io::lifecycle::Opened {
            encoding: None,
            document: Document::from_utf8("reloaded", workspace.bytes.clone(), workspace.history.clone()).unwrap(),
            path: path.to_path_buf(),
            fingerprint: Fingerprint {
                identity: bareline_platform::FileIdentity {
                    volume: 1,
                    file: 3,
                    length: 8,
                    modified: 0,
                },
                sha256: [0; 32],
            },
            bom: false,
        }
    }

    #[test]
    fn a_resident_reload_replaces_its_unchanged_tab_in_place() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].set_read_only(true);
        let before = workspace.editors[0].document_identity();
        let reload = PendingReload::capture(&workspace.editors[0]);
        let path = std::env::temp_dir().join("bareline-handler-reload.txt");
        let opened = opened(&workspace, &path);
        workspace.complete_resident_reload(&reload, opened, None);
        assert_eq!(workspace.message.as_deref(), Some("Reloaded from disk."));
        assert_eq!(workspace.editors.len(), 1);
        let after = workspace.editors[0].document_identity();
        assert_ne!(after, before);
        assert_eq!(
            workspace.replacement_document(before.0),
            after.0,
            "the tab keeps its place"
        );
        assert!(workspace.editors[0].viewport().user_read_only);
        assert_eq!(workspace.path(0), Some(path.as_path()));
    }

    /// REC-04/P6-02: text with unsaved edits is replaced only after its
    /// recovery discard started; otherwise the reload is abandoned.
    #[test]
    fn a_reload_never_replaces_edits_whose_recovery_was_kept() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("unsaved draft".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(workspace.editors[0].dirty());
        let before = workspace.editors[0].document_identity();
        let mut reload = PendingReload::capture(&workspace.editors[0]);
        let path = std::env::temp_dir().join("bareline-handler-reload-unretired.txt");
        let opened_text = opened(&workspace, &path);
        workspace.complete_resident_reload(&reload, opened_text, None);
        assert_eq!(
            workspace.message.as_deref(),
            Some("Document changed while reloading; current edits were preserved.")
        );
        assert_eq!(workspace.editors[0].document_identity(), before);
        assert!(workspace.editors[0].dirty());
        // Once the discard has started (`gate_reload`), the text is replaced.
        reload.discarding = true;
        let opened_text = opened(&workspace, &path);
        workspace.complete_resident_reload(&reload, opened_text, None);
        assert_eq!(workspace.message.as_deref(), Some("Reloaded from disk."));
        assert_ne!(workspace.editors[0].document_identity(), before);
        assert_eq!(workspace.lifecycle(0), Some(FileLifecycle::Loaded));
    }

    #[test]
    fn a_reload_whose_document_closed_replaces_nothing() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        workspace.new_document().unwrap();
        let reload = PendingReload::capture(&workspace.editors[0]);
        workspace.new_document().unwrap();
        workspace.editors.remove(0);
        workspace.tabs.remove(0);
        let survivor = workspace.editors[0].document_identity();
        let path = std::env::temp_dir().join("bareline-handler-reload-closed.txt");
        let opened = opened(&workspace, &path);
        workspace.complete_resident_reload(&reload, opened, None);
        assert_eq!(
            workspace.message.as_deref(),
            Some("Document changed while reloading; current edits were preserved.")
        );
        assert_eq!(workspace.editors.len(), 1);
        assert_eq!(workspace.editors[0].document_identity(), survivor);
        assert_eq!(workspace.path(0), None);
    }
}
