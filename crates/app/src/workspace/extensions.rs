// SPDX-License-Identifier: MPL-2.0
//! Captured original-byte authority; no file is opened on the UI thread.
use super::{Workspace, WorkspaceEditor};
use bareline_file_io::codecs::disk::DiskDecoded;
use std::sync::Arc;
pub enum OriginalSource {
    Resident(Arc<Vec<u8>>),
    Paged(DiskDecoded),
}
impl OriginalSource {
    pub fn len(&self) -> u64 {
        match self { Self::Resident(bytes) => bytes.len() as u64, Self::Paged(store) => store.raw_len }
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}
impl Workspace {
    pub fn raw_source_descriptor(&self, index: usize) -> Result<Option<OriginalSource>, String> {
        match self.editors.get(index).ok_or("Document closed")? {
            WorkspaceEditor::Paged(editor) => Ok(Some(OriginalSource::Paged(editor.read_handle().original_store()?))),
            WorkspaceEditor::Resident(_) => Ok(self.files.get(index).and_then(Option::as_ref)
                .and_then(|file| file.encoding.as_ref()).map(|encoding| OriginalSource::Resident(encoding.original_bytes()))),
        }
    }
    pub fn extension_edits_preserve_original(&self, index: usize) -> bool {
        self.editors.get(index).is_some_and(|editor| !editor.paged() && !editor.read_only())
            && !self.files.get(index).and_then(Option::as_ref).and_then(|file| file.encoding.as_ref())
                .is_some_and(|encoding| encoding.has_opaque_original())
    }
}
