// SPDX-License-Identifier: MPL-2.0
use bareline_document::DocumentMetadata;
use bareline_file_io::codecs::{Confidence, Detection, Encoding, state::EncodingState};

pub(super) struct NewDocumentDefaults {
    pub encoding: String,
    pub eol: String,
    system_code_page: Option<u32>,
}

impl Default for NewDocumentDefaults {
    fn default() -> Self {
        // Neutral workspace callers retain the historical defaults. The shell
        // applies resolved user settings before publishing any new document.
        Self {
            encoding: "utf-8".into(),
            eol: "lf".into(),
            system_code_page: None,
        }
    }
}

impl NewDocumentDefaults {
    pub fn metadata(&self) -> Result<DocumentMetadata, String> {
        let (encoding, bom) = match self.encoding.as_str() {
            "utf-8" => (Encoding::Utf8, false),
            "utf-8-bom" => (Encoding::Utf8, true),
            "utf-16le" => (Encoding::Utf16Le, true),
            "utf-16be" => (Encoding::Utf16Be, true),
            "system" => {
                let code_page = self
                    .system_code_page
                    .ok_or("System encoding is unavailable on this host")?;
                let encoding = match code_page {
                    65001 => Some(Encoding::Utf8),
                    932 => Some(Encoding::ShiftJis),
                    936 => Some(Encoding::Gbk),
                    949 => Some(Encoding::EucKr),
                    950 => Some(Encoding::Big5),
                    1250..=1258 => Encoding::from_label(&format!("windows-{code_page}")),
                    _ => None,
                }
                .ok_or_else(|| {
                    format!("System code page {code_page} is not supported; select an explicit new-file encoding")
                })?;
                (encoding, false)
            }
            other => return Err(format!("Unsupported new-file encoding: {other}")),
        };
        if !matches!(self.eol.as_str(), "lf" | "crlf") {
            return Err(format!("Unsupported new-file line ending: {}", self.eol));
        }
        let metadata = DocumentMetadata::new(std::collections::BTreeMap::from([(
            "file.new_document_eol".into(),
            self.eol.clone(),
        )]))
        .map_err(|error| format!("new document policy: {error:?}"))?;
        let mut state = EncodingState::new(Detection {
            encoding: Encoding::Utf8,
            confidence: Confidence::Utf8Sample,
            bom: false,
            binary_warning: false,
        });
        state.convert_to(encoding);
        state.bom = bom;
        bareline_file_io::codecs::state::with_encoding(&metadata, &state)
            .map_err(|error| format!("new document encoding: {error:?}"))
    }
}

impl super::Workspace {
    /// Supply the platform's ANSI code page, not its console/OEM encoding.
    pub fn set_system_code_page(&mut self, code_page: u32) {
        self.new_document_defaults.system_code_page = Some(code_page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Input, Workspace, WorkspaceEditor, tests::PagedFileSystem};
    use std::sync::Arc;

    fn settle(workspace: &mut Workspace) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            workspace.pump();
            if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    fn settings(encoding: &str, eol: &str) -> bareline_settings::EffectiveSettings {
        bareline_settings::EffectiveSettings {
            default_encoding: encoding.into(),
            default_eol: eol.into(),
            ..Default::default()
        }
    }
    #[test]
    fn new_file_defaults_are_clean_and_changes_affect_only_later_documents() {
        let mut w = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        w.apply_resource_settings(&settings("utf-16le", "crlf"));
        w.new_document().unwrap();
        let initial = w.editors[0].snapshot().metadata().clone();
        assert!(!w.editors[0].dirty());
        w.editors[0].enqueue(Input::Undo);
        settle(&mut w);
        assert_eq!(w.editors[0].snapshot().metadata(), &initial);
        assert!(!w.editors[0].dirty());
        let newline = w.editors[0].snapshot().insertion_eol().to_owned();
        assert_eq!(newline, "\r\n");
        w.editors[0].enqueue(Input::Insert(newline));
        settle(&mut w);
        assert!(w.editors[0].dirty());
        w.editors[0].enqueue(Input::Undo);
        settle(&mut w);
        assert!(!w.editors[0].dirty());
        assert_eq!(w.editors[0].snapshot().insertion_eol(), "\r\n");
        w.editors[0].enqueue(Input::Redo);
        settle(&mut w);
        assert_eq!(
            w.editors[0]
                .snapshot()
                .read(bareline_document::TextOffset(0)..bareline_document::TextOffset(2), 2)
                .unwrap(),
            "\r\n"
        );
        w.apply_resource_settings(&settings("utf-8-bom", "lf"));
        w.new_document().unwrap();
        assert_eq!(w.editors[0].snapshot().metadata(), &initial);
        assert_eq!(w.encoding_state(0).unwrap().save_target, Encoding::Utf16Le);
        assert_eq!(w.encoding_state(1).unwrap().save_target, Encoding::Utf8);
        assert!(w.encoding_state(1).unwrap().bom);
        assert_eq!(w.editors[1].snapshot().insertion_eol(), "\n");
        assert!(!w.editors[1].dirty());
    }
    #[test]
    fn new_file_eol_conversion_before_first_enter_overrides_default_and_undo_restores_it() {
        use bareline_file_io::codecs::state::Eol;
        for prefix in ["", "first line"] {
            let mut w = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            w.apply_resource_settings(&settings("utf-8", "crlf"));
            w.new_document().unwrap();
            if !prefix.is_empty() {
                w.editors[0].enqueue(Input::Insert(prefix.into()));
                settle(&mut w);
            }
            w.encoding_eol(0, Eol::Lf, false).unwrap();
            settle(&mut w);
            assert_eq!(w.editors[0].snapshot().insertion_eol(), "\n");
            assert!(w.editors[0].dirty());
            w.editors[0].enqueue(Input::Undo);
            settle(&mut w);
            assert_eq!(w.editors[0].snapshot().insertion_eol(), "\r\n");
            assert_eq!(w.editors[0].dirty(), !prefix.is_empty());
            w.editors[0].enqueue(Input::Redo);
            settle(&mut w);
            assert_eq!(w.editors[0].snapshot().insertion_eol(), "\n");
            assert_eq!(w.editors[0].snapshot().len(), prefix.len());
        }
    }
    #[test]
    fn new_file_defaults_reject_unknown_system_code_page_without_publishing_a_tab() {
        let mut w = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        w.apply_resource_settings(&settings("system", "crlf"));
        w.set_system_code_page(874);
        assert!(w.new_document().unwrap_err().contains("874"));
        assert!(w.editors.is_empty());
        assert!(w.message.as_ref().unwrap().contains("select an explicit"));
        for (page, encoding) in [
            (1252, Encoding::Windows1252),
            (932, Encoding::ShiftJis),
            (65001, Encoding::Utf8),
        ] {
            w.set_system_code_page(page);
            w.new_document().unwrap();
            let state = w.encoding_state(w.editors.len() - 1).unwrap();
            assert_eq!(state.save_target, encoding);
            assert!(!state.bom);
        }
    }
    #[test]
    fn new_file_defaults_save_exact_encoding_bom_and_preserve_literal_mixed_eol() {
        for (name, expected) in [
            ("utf-8", vec![65, 13, 10, 66, 10, 67, 13]),
            ("utf-8-bom", vec![239, 187, 191, 65, 13, 10, 66, 10, 67, 13]),
            (
                "utf-16le",
                vec![255, 254, 65, 0, 13, 0, 10, 0, 66, 0, 10, 0, 67, 0, 13, 0],
            ),
            (
                "utf-16be",
                vec![254, 255, 0, 65, 0, 13, 0, 10, 0, 66, 0, 10, 0, 67, 0, 13],
            ),
        ] {
            let mut w = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            w.apply_resource_settings(&settings(name, "crlf"));
            w.new_document().unwrap();
            w.editors[0].enqueue(Input::Insert("A\r\nB\nC\r".into()));
            settle(&mut w);
            let path = std::env::temp_dir().join(format!(
                "bareline-new-defaults-{}-{}-{name}.txt",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            assert!(w.save(0, path.clone()));
            settle(&mut w);
            assert_eq!(std::fs::read(&path).unwrap(), expected, "{name}");
            assert!(!w.editors[0].dirty());
            // Open and save the existing mixed-EOL file under different defaults.
            let mut reopened = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
            reopened.apply_resource_settings(&settings("utf-16be", "lf"));
            reopened.open(path.clone());
            settle(&mut reopened);
            assert_eq!(reopened.editors[0].snapshot().eol_label(), "Mixed");
            assert!(reopened.save(0, path.clone()));
            settle(&mut reopened);
            assert_eq!(std::fs::read(&path).unwrap(), expected, "existing {name}");
            drop(reopened);
            drop(w);
            std::fs::remove_file(path).unwrap();
        }
    }
}
