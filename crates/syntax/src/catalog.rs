// SPDX-License-Identifier: MPL-2.0
//! Version-one native language metadata. Lexilla names describe bindings, not availability.
use crate::Language;
use std::path::Path;
pub const CATALOG_VERSION: u32 = 1;
#[derive(Clone, Copy, Debug)]
pub struct LanguageMetadata {
    pub language: Language,
    pub id: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    pub lexilla: &'static str,
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
    pub keywords: &'static str,
    pub indent_after: &'static str,
}
impl LanguageMetadata {
    /// Versioned data definition consumed by the same bounded native engine.
    /// Built-in Rust/raw-string handling additionally uses its typed language ID.
    pub fn native_definition(&self) -> crate::udl::Definition {
        crate::udl::Definition {
            version: 1,
            id: self.id.into(),
            name: self.label.into(),
            extensions: self.extensions.iter().map(|s| (*s).into()).collect(),
            keywords: self
                .keywords
                .split_ascii_whitespace()
                .map(str::to_owned)
                .collect(),
            operators: "{}[]():;,.+-*/%=!<>|&^?~".into(),
            line_comment: self.line_comment.map(str::to_owned),
            block_comment: self.block_comment.map(|(a, b)| (a.into(), b.into())),
            strings: if self.language == Language::Json {
                vec!['"']
            } else if matches!(
                self.language,
                Language::JavaScript | Language::TypeScript | Language::Go
            ) {
                vec!['"', '\'', '`']
            } else {
                vec!['"', '\'']
            },
            fold_pairs: vec![('{', '}'), ('[', ']')],
        }
    }
}
macro_rules! entry {
    ($lang:ident,$id:literal,$label:literal,$ext:expr,$lexer:literal,$line:expr,$block:expr,$keys:expr,$indent:literal) => {
        LanguageMetadata {
            language: Language::$lang,
            id: $id,
            label: $label,
            extensions: $ext,
            lexilla: $lexer,
            line_comment: $line,
            block_comment: $block,
            keywords: $keys,
            indent_after: $indent,
        }
    };
}
const C_WORDS: &str = "auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while";
const JS_WORDS: &str = "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield true false null undefined";
pub static CATALOG: &[LanguageMetadata] = &[
    entry!(
        C,
        "c",
        "C",
        &["c", "h"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        C_WORDS,
        "{"
    ),
    entry!(
        Cpp,
        "cpp",
        "C++",
        &["cpp", "cxx", "cc", "hpp", "hxx"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "class namespace template typename public private protected virtual override nullptr constexpr using new delete true false auto const struct return if else for while",
        "{"
    ),
    entry!(
        CSharp,
        "csharp",
        "C#",
        &["cs"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "using namespace class interface public private protected internal static async await var string int bool new return if else foreach true false null",
        "{"
    ),
    entry!(
        Java,
        "java",
        "Java",
        &["java"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "package import class interface public private protected static final void int boolean new return if else for while try catch throw throws true false null",
        "{"
    ),
    entry!(
        JavaScript,
        "javascript",
        "JavaScript",
        &["js", "mjs", "cjs", "jsx"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        JS_WORDS,
        "{"
    ),
    entry!(
        TypeScript,
        "typescript",
        "TypeScript",
        &["ts", "tsx"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "async await class const declare enum export extends function implements import interface let namespace new private protected public readonly return static string number boolean type typeof undefined unknown never void true false null",
        "{"
    ),
    entry!(
        Python,
        "python",
        "Python",
        &["py", "pyw"],
        "python",
        Some("#"),
        None,
        "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield",
        ":"
    ),
    entry!(
        Rust,
        "rust",
        "Rust",
        &["rs"],
        "rust",
        Some("//"),
        Some(("/*", "*/")),
        crate::RUST_KEYWORDS,
        "{"
    ),
    entry!(
        Go,
        "go",
        "Go",
        &["go"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var true false nil",
        "{"
    ),
    entry!(
        Html,
        "html",
        "HTML",
        &["html", "htm"],
        "hypertext",
        None,
        Some(("<!--", "-->")),
        "html head body title script style div span a p input form button table tr td meta link",
        ">"
    ),
    entry!(
        Css,
        "css",
        "CSS",
        &["css"],
        "css",
        None,
        Some(("/*", "*/")),
        "important inherit initial unset none auto flex grid block inline relative absolute fixed sticky",
        "{"
    ),
    entry!(
        Json,
        "json",
        "JSON",
        &["json", "jsonl"],
        "json",
        None,
        None,
        "true false null",
        "{["
    ),
    entry!(
        Xml,
        "xml",
        "XML",
        &["xml", "svg", "xsd", "xsl"],
        "xml",
        None,
        Some(("<!--", "-->")),
        "xml version encoding",
        ">"
    ),
    entry!(
        Sql,
        "sql",
        "SQL",
        &["sql"],
        "sql",
        Some("--"),
        Some(("/*", "*/")),
        "SELECT FROM WHERE INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE DROP ALTER JOIN LEFT RIGHT INNER OUTER ON AS AND OR NOT NULL IS ORDER BY GROUP HAVING LIMIT DISTINCT UNION ALL true false select from where insert into values update set delete create table join on as and or not null",
        "("
    ),
    entry!(
        Toml,
        "toml",
        "TOML",
        &["toml"],
        "toml",
        Some("#"),
        None,
        "true false",
        "[{"
    ),
];
static PLAIN: LanguageMetadata = LanguageMetadata {
    language: Language::PlainText,
    id: "text",
    label: "Plain text",
    extensions: &[],
    lexilla: "null",
    line_comment: None,
    block_comment: None,
    keywords: "",
    indent_after: "",
};
impl Language {
    pub fn metadata(self) -> &'static LanguageMetadata {
        CATALOG
            .iter()
            .find(|m| m.language == self)
            .unwrap_or(&PLAIN)
    }
    pub fn label(self) -> &'static str {
        self.metadata().label
    }
    pub fn from_id(id: &str) -> Option<Self> {
        if id == "text" {
            return Some(Self::PlainText);
        }
        CATALOG.iter().find(|m| m.id == id).map(|m| m.language)
    }
    pub fn detect_with_context(
        path: &Path,
        prefix: &str,
        explicit: Option<Self>,
        association: Option<Self>,
    ) -> Self {
        Self::detect_with_regions(path, prefix, "", explicit, association)
    }
    /// Inputs are bounded before parsing; callers read prefix/suffix on a worker.
    pub fn detect_with_regions(
        path: &Path,
        prefix: &str,
        suffix: &str,
        explicit: Option<Self>,
        association: Option<Self>,
    ) -> Self {
        if let Some(language) = explicit.or(association) {
            return language;
        }
        let detected = Self::detect(path);
        if detected != Self::PlainText {
            return detected;
        }
        fn bounded(text: &str) -> &str {
            let mut end = text.len().min(8192);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        }
        let prefix = bounded(prefix);
        let suffix = bounded(suffix);
        let first = prefix.lines().next().unwrap_or("");
        if first.starts_with("#!") {
            if first.contains("python") {
                return Self::Python;
            }
            if first.contains("node") || first.contains("deno") {
                return Self::JavaScript;
            }
        }
        for line in prefix.lines().take(5).chain(suffix.lines().rev().take(5)) {
            for marker in ["mode:", "filetype=", "ft="] {
                if let Some((_, value)) = line.split_once(marker) {
                    let id = value
                        .trim_start()
                        .split(|c: char| {
                            !c.is_ascii_alphanumeric() && !matches!(c, '+' | '#' | '-')
                        })
                        .next()
                        .unwrap_or("");
                    let alias = match id {
                        "c++" => "cpp",
                        "c#" => "csharp",
                        "js" => "javascript",
                        "ts" => "typescript",
                        "py" => "python",
                        other => other,
                    };
                    if let Some(language) = Self::from_id(alias) {
                        return language;
                    }
                }
            }
        }
        let trimmed = prefix.trim_start();
        if trimmed.starts_with("<?xml") {
            Self::Xml
        } else if trimmed.to_ascii_lowercase().starts_with("<!doctype html") {
            Self::Html
        } else if (trimmed.starts_with('{') && trimmed.contains("\":"))
            || (trimmed.starts_with('[') && trimmed.contains("{\""))
        {
            Self::Json
        } else {
            Self::PlainText
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifteen_detect_with_comment_metadata() {
        assert_eq!(CATALOG.len(), 15);
        for entry in CATALOG {
            assert_eq!(
                Language::detect(Path::new(&format!("a.{}", entry.extensions[0]))),
                entry.language
            );
            assert!(!entry.lexilla.is_empty());
            assert!(!entry.keywords.is_empty());
        }
        assert_eq!(
            Language::detect_with_context(Path::new("tool"), "#!/usr/bin/env python3", None, None),
            Language::Python
        );
        assert_eq!(
            Language::detect_with_context(Path::new("a.rs"), "", Some(Language::Json), None),
            Language::Json
        );
    }
}
