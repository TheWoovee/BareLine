// SPDX-License-Identifier: MPL-2.0
//! Type identifiers and panel plans: the pasteboard types Bareline's private
//! clipboard data travels under, the Uniform Type Identifier of a path, the
//! configuration a save panel gets from `SaveDialogOptions`, and the user's
//! language as a BCP 47 tag. Plain Rust, tested on every system.
use bareline_platform::{
    FileTypeFilter, SaveDialogOptions,
    clipboard::{MULTISELECTION_CLIPBOARD_FORMAT, RECTANGLE_CLIPBOARD_FORMAT},
};
use std::path::{Path, PathBuf};

/// `CFBundleIdentifier` of Bareline.app; also the prefix of its private
/// pasteboard types. packaging/macos/Info.plist.in uses the same value.
pub const BUNDLE_IDENTIFIER: &str = "com.thewoovee.bareline";

/// The pasteboard type for one of Bareline's private clipboard formats, a
/// reverse-DNS dynamic type under the bundle identifier. Only the formats the
/// shared clipboard contract accepts have one.
pub fn pasteboard_type(format: &str) -> Option<&'static str> {
    match format {
        RECTANGLE_CLIPBOARD_FORMAT => Some("com.thewoovee.bareline.rectangle-v1"),
        MULTISELECTION_CLIPBOARD_FORMAT => Some("com.thewoovee.bareline.multiselection-v1"),
        _ => None,
    }
}

/// UTIs Bareline.app declares it opens (`CFBundleDocumentTypes`).
pub const PLAIN_TEXT: &str = "public.plain-text";
pub const SOURCE_CODE: &str = "public.source-code";

/// The Uniform Type Identifier macOS gives a file name, by extension, for
/// the text and source types Bareline opens: a system UTI where one exists,
/// `public.source-code` for other known source extensions, `public.plain-text`
/// for other text, and `public.data` for anything else.
pub fn content_type(path: &Path) -> &'static str {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return "public.data";
    };
    match extension.to_ascii_lowercase().as_str() {
        "txt" | "text" => PLAIN_TEXT,
        "md" | "markdown" => "net.daringfireball.markdown",
        "log" => "com.apple.log",
        "csv" => "public.comma-separated-values-text",
        "tsv" => "public.tab-separated-values-text",
        "json" => "public.json",
        "xml" => "public.xml",
        "yaml" | "yml" => "public.yaml",
        "html" | "htm" => "public.html",
        "rtf" => "public.rtf",
        "c" => "public.c-source",
        "h" => "public.c-header",
        "cc" | "cpp" | "cxx" | "c++" => "public.c-plus-plus-source",
        "hh" | "hpp" | "hxx" => "public.c-plus-plus-header",
        "m" => "public.objective-c-source",
        "mm" => "public.objective-c-plus-plus-source",
        "swift" => "public.swift-source",
        "py" => "public.python-script",
        "rb" => "public.ruby-script",
        "pl" | "pm" => "public.perl-script",
        "php" => "public.php-script",
        "sh" | "bash" | "zsh" | "command" => "public.shell-script",
        "js" | "mjs" | "cjs" => "com.netscape.javascript-source",
        "java" => "com.sun.java-source",
        "rs" | "go" | "ts" | "tsx" | "jsx" | "kt" | "kts" | "cs" | "fs" | "lua" | "sql" | "ps1" | "psm1" | "bat"
        | "cmd" | "vb" | "scala" | "dart" | "r" | "zig" | "toml" | "ini" | "cfg" | "conf" | "css" | "scss" | "less"
        | "vue" | "svelte" | "cmake" | "mk" | "gradle" => SOURCE_CODE,
        _ => "public.data",
    }
}
/// Whether a file's type conforms to one Bareline.app declares, so Finder's
/// Open With lists Bareline for it.
pub fn opens_as_text(path: &Path) -> bool {
    content_type(path) != "public.data"
}

/// The file extensions of a filter (`*.txt;*.md` gives `txt`, `md`), or
/// `None` for "All files".
pub fn filter_extensions(filter: &FileTypeFilter) -> Option<Vec<String>> {
    let mut extensions = Vec::new();
    for pattern in filter.patterns.split(';') {
        match pattern.trim().strip_prefix("*.") {
            Some("*") | None => return None,
            Some(extension) => extensions.push(extension.to_owned()),
        }
    }
    (!extensions.is_empty()).then_some(extensions)
}

/// How an `NSSavePanel` is configured for a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavePanelPlan {
    /// `allowedFileTypes`: the panel appends the first one to a name typed
    /// without an extension. `None` allows any name as typed, so `Makefile`
    /// stays `Makefile`.
    pub allowed_extensions: Option<Vec<String>>,
    /// `allowsOtherFileTypes`: a name with another extension is kept.
    pub allows_other_types: bool,
    /// `nameFieldStringValue`.
    pub name: Option<String>,
    /// `directoryURL`. The caller checked it on a worker (APP-19).
    pub directory: Option<PathBuf>,
    /// Start in ~/Documents when the caller gave no directory and confirms
    /// overwrites itself, as the Windows dialog does.
    pub documents_fallback: bool,
}

/// The save panel for `options`. The type list collapses to the extensions
/// of the first filter, with the default extension first; an `NSSavePanel`
/// has no type menu without an accessory view.
///
/// `app_confirms_overwrite` cannot turn off the panel's own replace question
/// (there is no public API for it), so on macOS the person is asked by the
/// panel; the shell should skip its second question after this dialog (see
/// [`NATIVE_OVERWRITE_PROMPT`]) and rely on the save pipeline's identity check.
pub fn save_panel_plan(options: &SaveDialogOptions) -> SavePanelPlan {
    let allowed_extensions = options.default_extension().map(|default| {
        let mut extensions = options
            .filters()
            .first()
            .and_then(filter_extensions)
            .unwrap_or_default();
        extensions.retain(|extension| extension != default);
        extensions.insert(0, default.to_owned());
        extensions
    });
    SavePanelPlan {
        allows_other_types: true,
        allowed_extensions,
        name: options.default_name.clone(),
        directory: options.default_directory.clone(),
        documents_fallback: options.app_confirms_overwrite && options.default_directory.is_none(),
    }
}
/// `NSSavePanel` always asks before replacing an existing file.
pub const NATIVE_OVERWRITE_PROMPT: bool = true;

/// An entry of `NSLocale.preferredLanguages` as a BCP 47 tag: `en-US` and
/// `zh-Hans-CN` stay as they are and `de_DE` becomes `de-DE`. `None` for an
/// empty entry.
pub fn language_tag(preferred: &str) -> Option<String> {
    let tag = preferred.trim().replace('_', "-");
    let valid = !tag.is_empty()
        && tag
            .split('-')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()));
    valid.then_some(tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_platform::{SaveFileKind, dialogs::ALL_FILES, dialogs::TEXT_FILES};

    #[test]
    fn private_clipboard_formats_have_reverse_dns_types() {
        let rectangle = pasteboard_type(RECTANGLE_CLIPBOARD_FORMAT).unwrap();
        let multiselection = pasteboard_type(MULTISELECTION_CLIPBOARD_FORMAT).unwrap();
        for kind in [rectangle, multiselection] {
            assert!(kind.starts_with(BUNDLE_IDENTIFIER));
            assert!(kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
        }
        assert_ne!(rectangle, multiselection);
        assert_eq!(pasteboard_type("public.utf8-plain-text"), None);
        assert_eq!(pasteboard_type("MSDEVColumnSelect"), None);
    }

    #[test]
    fn paths_classify_by_extension() {
        assert_eq!(content_type(Path::new("/tmp/notes.TXT")), PLAIN_TEXT);
        assert_eq!(content_type(Path::new("README.md")), "net.daringfireball.markdown");
        assert_eq!(content_type(Path::new("main.c")), "public.c-source");
        assert_eq!(content_type(Path::new("lib.rs")), SOURCE_CODE);
        assert_eq!(content_type(Path::new("Cargo.toml")), SOURCE_CODE);
        assert_eq!(content_type(Path::new("build.sh")), "public.shell-script");
        assert_eq!(content_type(Path::new("Makefile")), "public.data");
        assert_eq!(content_type(Path::new("photo.png")), "public.data");
        assert!(opens_as_text(Path::new("a.json")));
        assert!(!opens_as_text(Path::new("archive.zip")));
    }

    #[test]
    fn filters_become_extension_lists() {
        assert_eq!(filter_extensions(&ALL_FILES), None);
        let text = filter_extensions(&TEXT_FILES).unwrap();
        assert_eq!(text.first().map(String::as_str), Some("txt"));
        assert!(text.contains(&"yml".to_owned()));
    }

    #[test]
    fn save_panels_append_only_the_kinds_default_extension() {
        let text = save_panel_plan(&SaveDialogOptions::new(SaveFileKind::Text).named("Untitled 1"));
        let allowed = text.allowed_extensions.unwrap();
        assert_eq!(allowed[0], "txt");
        assert!(allowed.contains(&"md".to_owned()));
        assert!(text.allows_other_types);
        assert_eq!(text.name.as_deref(), Some("Untitled 1"));
        // A named document or an unspecified save keeps the name as typed.
        for kind in [SaveFileKind::Named, SaveFileKind::Any] {
            assert_eq!(save_panel_plan(&SaveDialogOptions::new(kind)).allowed_extensions, None);
        }
        let html = save_panel_plan(&SaveDialogOptions::new(SaveFileKind::Html).named_after("notes.txt"));
        assert_eq!(html.allowed_extensions, Some(vec!["html".into(), "htm".into()]));
        assert_eq!(html.name.as_deref(), Some("notes.html"));
    }

    #[test]
    fn documents_is_the_fallback_only_when_the_app_confirms_overwrite() {
        let confirmed = SaveDialogOptions::new(SaveFileKind::Text).app_confirms_overwrite();
        assert!(save_panel_plan(&confirmed).documents_fallback);
        let placed = confirmed.in_directory(Some(PathBuf::from("/Users/me/Projects")));
        let plan = save_panel_plan(&placed);
        assert!(!plan.documents_fallback);
        assert_eq!(plan.directory, Some(PathBuf::from("/Users/me/Projects")));
        assert!(!save_panel_plan(&SaveDialogOptions::new(SaveFileKind::Text)).documents_fallback);
    }

    #[test]
    fn preferred_languages_become_language_tags() {
        assert_eq!(language_tag("en-US").as_deref(), Some("en-US"));
        assert_eq!(language_tag("zh-Hans-CN").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(language_tag("de_DE").as_deref(), Some("de-DE"));
        assert_eq!(language_tag("fr").as_deref(), Some("fr"));
        assert_eq!(language_tag(""), None);
        assert_eq!(language_tag("en--US"), None);
        assert_eq!(language_tag("en US"), None);
    }
}
