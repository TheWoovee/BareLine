// SPDX-License-Identifier: MPL-2.0
//! OS-neutral description of a save dialog. Each platform maps it onto its own
//! native dialog; the policy for file types and extensions lives here (UI-11).
use std::path::{Path, PathBuf};

/// One entry in a save dialog's file-type list: a label and `;`-separated patterns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileTypeFilter {
    pub label: &'static str,
    pub patterns: &'static str,
}
pub const ALL_FILES: FileTypeFilter = FileTypeFilter {
    label: "All files",
    patterns: "*.*",
};
pub const TEXT_FILES: FileTypeFilter = FileTypeFilter {
    label: "Text files",
    patterns: "*.txt;*.md;*.markdown;*.log;*.json;*.xml;*.csv;*.ini;*.toml;*.yaml;*.yml",
};
const HTML_FILES: FileTypeFilter = FileTypeFilter {
    label: "HTML files",
    patterns: "*.html;*.htm",
};
const RTF_FILES: FileTypeFilter = FileTypeFilter {
    label: "Rich Text Format",
    patterns: "*.rtf",
};
const JSON_FILES: FileTypeFilter = FileTypeFilter {
    label: "JSON files",
    patterns: "*.json",
};
const TOML_FILES: FileTypeFilter = FileTypeFilter {
    label: "TOML files",
    patterns: "*.toml",
};

/// What a save dialog chooses a destination for. The kind decides the file-type
/// list, which type is selected first and the extension added to a bare name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveFileKind {
    /// Plain text, such as a document that has never been saved: text types
    /// first and `.txt` added to a name typed without an extension.
    Text,
    /// A document that already has a name: All files first and nothing added,
    /// so `Makefile` stays `Makefile`.
    Named,
    Html,
    Rtf,
    Json,
    Toml,
    /// Anything else: All files only and nothing added.
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDialogOptions {
    pub kind: SaveFileKind,
    /// Initial file name, without a directory.
    pub default_name: Option<String>,
    pub default_directory: Option<PathBuf>,
    /// The caller confirms replacing an existing file itself, after capturing
    /// its fingerprint, so the native dialog must not ask as well.
    pub app_confirms_overwrite: bool,
}
impl SaveDialogOptions {
    pub fn new(kind: SaveFileKind) -> Self {
        Self {
            kind,
            default_name: None,
            default_directory: None,
            app_confirms_overwrite: false,
        }
    }
    pub fn named(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.default_name = (!name.is_empty()).then_some(name);
        self
    }
    /// Suggest the document's name with this kind's extension, as exports do:
    /// `notes.txt` exported to HTML is offered as `notes.html`.
    pub fn named_after(self, document: &str) -> Self {
        let stem = Path::new(document)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .unwrap_or("Untitled");
        let name = match self.default_extension() {
            Some(extension) => format!("{stem}.{extension}"),
            None => stem.to_owned(),
        };
        self.named(name)
    }
    pub fn in_directory(mut self, directory: Option<PathBuf>) -> Self {
        self.default_directory = directory;
        self
    }
    pub fn app_confirms_overwrite(mut self) -> Self {
        self.app_confirms_overwrite = true;
        self
    }
    /// File types in display order; the first one is selected initially.
    pub fn filters(&self) -> Vec<FileTypeFilter> {
        match self.kind {
            SaveFileKind::Text => vec![TEXT_FILES, ALL_FILES],
            SaveFileKind::Named => vec![ALL_FILES, TEXT_FILES],
            SaveFileKind::Html => vec![HTML_FILES, ALL_FILES],
            SaveFileKind::Rtf => vec![RTF_FILES, ALL_FILES],
            SaveFileKind::Json => vec![JSON_FILES, ALL_FILES],
            SaveFileKind::Toml => vec![TOML_FILES, ALL_FILES],
            SaveFileKind::Any => vec![ALL_FILES],
        }
    }
    /// Extension (without the dot) the dialog adds to a name typed without one.
    pub fn default_extension(&self) -> Option<&'static str> {
        match self.kind {
            SaveFileKind::Text => Some("txt"),
            SaveFileKind::Html => Some("html"),
            SaveFileKind::Rtf => Some("rtf"),
            SaveFileKind::Json => Some("json"),
            SaveFileKind::Toml => Some("toml"),
            SaveFileKind::Named | SaveFileKind::Any => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_documents_keep_their_name_and_add_no_extension() {
        let options = SaveDialogOptions::new(SaveFileKind::Named).named("Makefile");
        assert_eq!(options.default_name.as_deref(), Some("Makefile"));
        assert_eq!(options.default_extension(), None);
        assert_eq!(options.filters().first(), Some(&ALL_FILES));
        assert!(!options.app_confirms_overwrite);
    }
    #[test]
    fn new_documents_default_to_text() {
        let options = SaveDialogOptions::new(SaveFileKind::Text)
            .named("Untitled 1.txt")
            .app_confirms_overwrite();
        assert_eq!(options.default_extension(), Some("txt"));
        assert_eq!(options.filters(), vec![TEXT_FILES, ALL_FILES]);
        assert!(options.app_confirms_overwrite);
    }
    #[test]
    fn exports_offer_their_own_type_and_extension() {
        let html = SaveDialogOptions::new(SaveFileKind::Html).named_after("notes.txt");
        assert_eq!(html.default_name.as_deref(), Some("notes.html"));
        assert_eq!(html.default_extension(), Some("html"));
        assert_eq!(html.filters()[0].patterns, "*.html;*.htm");
        let rtf = SaveDialogOptions::new(SaveFileKind::Rtf).named_after("Makefile");
        assert_eq!(rtf.default_name.as_deref(), Some("Makefile.rtf"));
        let json = SaveDialogOptions::new(SaveFileKind::Json);
        assert_eq!(json.default_extension(), Some("json"));
        assert_eq!(json.default_name, None);
        assert_eq!(
            SaveDialogOptions::new(SaveFileKind::Html)
                .named_after("")
                .default_name
                .as_deref(),
            Some("Untitled.html")
        );
    }
    #[test]
    fn unspecified_saves_never_force_an_extension() {
        let any = SaveDialogOptions::new(SaveFileKind::Any);
        assert_eq!(any.filters(), vec![ALL_FILES]);
        assert_eq!(any.default_extension(), None);
        assert_eq!(any.named("").default_name, None);
    }
}
