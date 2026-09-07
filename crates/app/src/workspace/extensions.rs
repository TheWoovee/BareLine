// SPDX-License-Identifier: MPL-2.0
//! Captured original-byte authority; no file is opened on the UI thread.
use super::{Workspace, WorkspaceEditor};
use bareline_file_io::codecs::disk::DiskDecoded;
use std::sync::Arc;
pub enum OriginalSource {
    Resident(Arc<Vec<u8>>),
    Paged(DiskDecoded),
    File {
        path: std::path::PathBuf,
        fingerprint: bareline_file_io::lifecycle::Fingerprint,
        platform: Arc<dyn bareline_platform::LocalFileSystem>,
    },
}
impl OriginalSource {
    pub fn len(&self) -> u64 {
        match self {
            Self::Resident(bytes) => bytes.len() as u64,
            Self::Paged(store) => store.raw_len,
            Self::File { fingerprint, .. } => fingerprint.identity.length,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Worker-only: retain the exact opened disk generation, never edited text.
    pub fn verified_file(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<std::fs::File>, String> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Seek, SeekFrom};
        let Self::File {
            path,
            fingerprint,
            platform,
        } = self
        else {
            return Ok(None);
        };
        let mut file = platform.open_sealed_read(path).map_err(|e| e.to_string())?;
        if platform.identity(&file).map_err(|e| e.to_string())? != fingerprint.identity {
            return Err("Original disk generation changed".into());
        }
        let mut hash = Sha256::new();
        let mut bytes = [0; 65536];
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err("Original read cancelled".into());
            }
            let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        if <[u8; 32]>::from(hash.finalize()) != fingerprint.sha256
            || platform.identity(&file).map_err(|e| e.to_string())? != fingerprint.identity
        {
            return Err("Original disk generation changed".into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        Ok(Some(file))
    }
}
impl Workspace {
    pub fn raw_source_descriptor(&self, index: usize) -> Result<Option<OriginalSource>, String> {
        match self.editors.get(index).ok_or("Document closed")? {
            WorkspaceEditor::Paged(editor) => Ok(Some(OriginalSource::Paged(
                editor.read_handle().original_store()?,
            ))),
            WorkspaceEditor::Resident(_) => {
                Ok(self.files.get(index).and_then(Option::as_ref).map(|file| {
                    file.encoding.as_ref().map_or_else(
                        || OriginalSource::File {
                            path: file.path.clone(),
                            fingerprint: file.fingerprint.clone(),
                            platform: self.file_system.clone(),
                        },
                        |encoding| OriginalSource::Resident(encoding.original_bytes()),
                    )
                }))
            }
        }
    }
    pub fn extension_edits_preserve_original(&self, index: usize) -> bool {
        self.editors
            .get(index)
            .is_some_and(|editor| !editor.paged() && !editor.read_only())
            && !self
                .files
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|file| file.encoding.as_ref())
                .is_some_and(|encoding| encoding.has_opaque_original())
    }
}
