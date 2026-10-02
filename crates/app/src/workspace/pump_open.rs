// SPDX-License-Identifier: MPL-2.0
//! Resident open completions (ARC-01).
use super::*;
impl Workspace {
    /// A resident open that finished: a reload replaces its tab, a duplicate
    /// resolves to the tab already holding the file, and otherwise the loading
    /// tab becomes the document.
    pub(super) fn complete_open(
        &mut self,
        pending: PendingIo,
        opened: bareline_file_io::lifecycle::Opened,
        mut admission: Option<bareline_search::replace_disk::OpenFileLease>,
    ) {
        let launch_request = pending.launch_request;
        self.note_recent(opened.path.clone());
        if let Some(reload) = &pending.reload {
            self.complete_resident_reload(reload, opened, admission);
            return;
        }
        if !pending.allow_duplicate
            && let Some(existing) = self.open_file_index(&opened.fingerprint.identity)
        {
            self.settle_duplicate_open(existing, pending.preview.as_ref(), launch_request);
            return;
        }
        let snapshot = opened.document.snapshot();
        let file = Some(FileState {
            binary_accepted: false,
            _lease: admission.take(),
            path: opened.path,
            fingerprint: opened.fingerprint,
            bom: opened.bom,
            encoding: opened.encoding,
        });
        let preview = pending.preview.as_ref().and_then(|source| {
            self.editors
                .iter()
                .position(|editor| editor.snapshot().same_document(source))
        });
        let index = if let Some(index) = preview {
            let loading = self.editors[index].document_identity();
            self.editors[index].finish_loading(self.scheduler.document(opened.document, 32), snapshot);
            self.note_tab_replaced(index, loading);
            self.tabs[index].file = file;
            self.tabs[index].label.clear();
            index
        } else {
            let editor = EditorSurface::new(
                self.scheduler.document(opened.document, 32),
                snapshot,
                self.notify.clone(),
            );
            self.push_tab(editor.into(), file, String::new())
        };
        self.refresh_encoding_open(index);
        self.message = self.encoding_hint(index);
        let document = self.editors[index].document_identity();
        self.record_launch_open(launch_request, Ok(document));
    }
}
