// SPDX-License-Identifier: MPL-2.0
//! Bounded worker-side utility operations. None mutate a file or auto-save a document.
use bareline_diff::CancelToken;
use bareline_document::{DocumentSnapshot, Edit, EditTransaction, Revision, TextOffset};
use bareline_syntax::{StyleKind, StyleSpan};
use base64::Engine;
use sha2::Digest;
use std::{
    io::{Read, Write},
    ops::Range,
};
use unicode_segmentation::UnicodeSegmentation;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtilityError {
    Cancelled,
    InvalidRange,
    IncompleteSource,
    BudgetExceeded,
    InvalidDecode,
    InvalidUtf8,
    Io,
    StaleStyles,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}
impl HashAlgorithm {
    pub fn label(self) -> &'static str {
        match self {
            Self::Md5 => "MD5 (legacy integrity hash)",
            Self::Sha1 => "SHA-1 (legacy integrity hash)",
            Self::Sha256 => "SHA-256",
            Self::Sha512 => "SHA-512",
        }
    }
}
enum Hasher {
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
}
impl Hasher {
    fn new(a: HashAlgorithm) -> Self {
        match a {
            HashAlgorithm::Md5 => Self::Md5(md5::Md5::new()),
            HashAlgorithm::Sha1 => Self::Sha1(sha1::Sha1::new()),
            HashAlgorithm::Sha256 => Self::Sha256(sha2::Sha256::new()),
            HashAlgorithm::Sha512 => Self::Sha512(sha2::Sha512::new()),
        }
    }
    fn update(&mut self, b: &[u8]) {
        match self {
            Self::Md5(h) => h.update(b),
            Self::Sha1(h) => h.update(b),
            Self::Sha256(h) => h.update(b),
            Self::Sha512(h) => h.update(b),
        }
    }
    fn finish(self) -> String {
        let bytes: Vec<u8> = match self {
            Self::Md5(h) => h.finalize().to_vec(),
            Self::Sha1(h) => h.finalize().to_vec(),
            Self::Sha256(h) => h.finalize().to_vec(),
            Self::Sha512(h) => h.finalize().to_vec(),
        };
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
#[derive(Clone, Debug)]
pub struct HashResult {
    pub hexadecimal: String,
    pub bytes: u64,
    pub revision: Option<Revision>,
}
/// Reads at most 64 KiB between cancellation/progress checks. Reader belongs on I/O pool.
pub fn hash_reader(
    mut reader: impl Read,
    algorithm: HashAlgorithm,
    cancel: &CancelToken,
    mut progress: impl FnMut(u64) -> bool,
) -> Result<HashResult, UtilityError> {
    let mut hasher = Hasher::new(algorithm);
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check(cancel)?;
        let count = reader.read(&mut buffer).map_err(|_| UtilityError::Io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes = bytes
            .checked_add(count as u64)
            .ok_or(UtilityError::BudgetExceeded)?;
        if !progress(bytes) {
            return Err(UtilityError::Cancelled);
        }
    }
    Ok(HashResult {
        hexadecimal: hasher.finish(),
        bytes,
        revision: None,
    })
}
pub fn hash_snapshot(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    algorithm: HashAlgorithm,
    cancel: &CancelToken,
) -> Result<HashResult, UtilityError> {
    if !snapshot.is_complete() {
        return Err(UtilityError::IncompleteSource);
    }
    let chunks = snapshot
        .chunks(range)
        .map_err(|_| UtilityError::InvalidRange)?;
    let mut hasher = Hasher::new(algorithm);
    let mut bytes = 0u64;
    for chunk in chunks {
        for part in chunk.as_bytes().chunks(64 * 1024) {
            check(cancel)?;
            hasher.update(part);
            bytes += part.len() as u64;
        }
    }
    Ok(HashResult {
        hexadecimal: hasher.finish(),
        bytes,
        revision: Some(snapshot.revision),
    })
}
fn check(c: &CancelToken) -> Result<(), UtilityError> {
    if c.is_cancelled() {
        Err(UtilityError::Cancelled)
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
}
/// All validation and output staging completes before one undoable transaction exists.
pub fn transform(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    kind: Transform,
    max_bytes: usize,
    cancel: &CancelToken,
) -> Result<EditTransaction, UtilityError> {
    check(cancel)?;
    if !snapshot.is_complete() {
        return Err(UtilityError::IncompleteSource);
    }
    let raw = snapshot.read(range.clone(), max_bytes).map_err(|e| {
        if e == bareline_document::Error::BudgetExceeded {
            UtilityError::BudgetExceeded
        } else {
            UtilityError::InvalidRange
        }
    })?;
    let output = match kind {
        Transform::Base64Encode => {
            if raw.len().div_ceil(3).saturating_mul(4) > max_bytes {
                return Err(UtilityError::BudgetExceeded);
            }
            base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
        }
        Transform::Base64Decode => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.as_bytes())
                .map_err(|_| UtilityError::InvalidDecode)?;
            String::from_utf8(bytes).map_err(|_| UtilityError::InvalidUtf8)?
        }
        Transform::UrlEncode => {
            let mut s = String::new();
            for (i, &b) in raw.as_bytes().iter().enumerate() {
                if i % 1024 == 0 {
                    check(cancel)?
                }
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    s.push(char::from(b))
                } else {
                    s.push('%');
                    s.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                    s.push(char::from(b"0123456789ABCDEF"[(b & 15) as usize]));
                }
                if s.len() > max_bytes {
                    return Err(UtilityError::BudgetExceeded);
                }
            }
            s
        }
        Transform::UrlDecode => {
            let bytes = raw.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if i % 1024 == 0 {
                    check(cancel)?
                }
                if bytes[i] == b'%' {
                    let h = *bytes.get(i + 1).ok_or(UtilityError::InvalidDecode)?;
                    let l = *bytes.get(i + 2).ok_or(UtilityError::InvalidDecode)?;
                    out.push(hex(h)?.checked_mul(16).ok_or(UtilityError::InvalidDecode)? + hex(l)?);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).map_err(|_| UtilityError::InvalidUtf8)?
        }
    };
    check(cancel)?;
    if output.len() > max_bytes {
        return Err(UtilityError::BudgetExceeded);
    }
    Ok(EditTransaction {
        base_revision: snapshot.revision,
        edits: vec![Edit {
            range,
            insert: output,
        }],
    })
}
fn hex(b: u8) -> Result<u8, UtilityError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(UtilityError::InvalidDecode),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statistics {
    pub bytes: usize,
    pub characters: usize,
    pub graphemes: usize,
    pub words: usize,
    pub lines: usize,
    pub revision: Revision,
}
/// Exact Unicode statistics for bounded logical lines. Long-line cap returns an explicit
/// budget error; callers still have snapshot.len() immediately for the byte count.
pub fn statistics(
    snapshot: &DocumentSnapshot,
    max_line_bytes: usize,
    cancel: &CancelToken,
    mut progress: impl FnMut(usize) -> bool,
) -> Result<Statistics, UtilityError> {
    if !snapshot.is_complete() {
        return Err(UtilityError::IncompleteSource);
    }
    let mut stats = Statistics {
        bytes: snapshot.len(),
        characters: 0,
        graphemes: 0,
        words: 0,
        lines: snapshot.line_count(),
        revision: snapshot.revision,
    };
    for line in 0..snapshot.line_count() {
        check(cancel)?;
        let range = snapshot
            .line_range(line)
            .map_err(|_| UtilityError::InvalidRange)?;
        let end = range.end.0;
        let text = snapshot
            .read(range, max_line_bytes)
            .map_err(|_| UtilityError::BudgetExceeded)?;
        stats.characters += text.chars().count();
        stats.graphemes += text.graphemes(true).count();
        stats.words += text.unicode_words().count();
        if !progress(end) {
            return Err(UtilityError::Cancelled);
        }
    }
    Ok(stats)
}
#[derive(Clone, Copy, Debug)]
pub enum ExportFormat {
    Html,
    Rtf,
}
#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);
pub struct ExportStyles<'a> {
    pub revision: Revision,
    pub spans: &'a [StyleSpan],
    pub foreground: Rgb,
    pub colors: [Rgb; 5],
}
fn color_index(kind: StyleKind) -> usize {
    match kind {
        StyleKind::Keyword => 1,
        StyleKind::String => 2,
        StyleKind::Number => 3,
        StyleKind::Comment => 4,
        StyleKind::Operator => 5,
    }
}
/// Streams escaped syntax export to a caller-owned staging writer. The caller must use
/// safe atomic file publication; an I/O error may leave only this staging output partial.
pub fn export(
    snapshot: &DocumentSnapshot,
    styles: &ExportStyles<'_>,
    format: ExportFormat,
    mut writer: impl Write,
    cancel: &CancelToken,
) -> Result<(), UtilityError> {
    if !snapshot.is_complete() {
        return Err(UtilityError::IncompleteSource);
    }
    if styles.revision != snapshot.revision {
        return Err(UtilityError::StaleStyles);
    }
    let mut previous = TextOffset(0);
    for span in styles.spans {
        if span.range.start < previous || snapshot.chunks(span.range.clone()).is_err() {
            return Err(UtilityError::InvalidRange);
        }
        previous = span.range.end;
    }
    match format {
        ExportFormat::Html => writer
            .write_all(b"<!doctype html><meta charset=\"utf-8\"><pre>")
            .map_err(|_| UtilityError::Io)?,
        ExportFormat::Rtf => {
            writer
                .write_all(b"{\\rtf1\\ansi\\uc1{\\colortbl;")
                .map_err(|_| UtilityError::Io)?;
            for Rgb(r, g, b) in std::iter::once(styles.foreground).chain(styles.colors) {
                write!(writer, "\\red{r}\\green{g}\\blue{b};").map_err(|_| UtilityError::Io)?;
            }
            writer.write_all(b"}").map_err(|_| UtilityError::Io)?;
        }
    }
    let mut at = TextOffset(0);
    let mut previous_cr = false;
    for span in styles.spans {
        if span.range.start > at {
            export_range(
                snapshot,
                at..span.range.start,
                0,
                styles,
                (format, &mut previous_cr),
                &mut writer,
                cancel,
            )?
        }
        export_range(
            snapshot,
            span.range.clone(),
            color_index(span.kind),
            styles,
            (format, &mut previous_cr),
            &mut writer,
            cancel,
        )?;
        at = span.range.end;
    }
    if at.0 < snapshot.len() {
        export_range(
            snapshot,
            at..TextOffset(snapshot.len()),
            0,
            styles,
            (format, &mut previous_cr),
            &mut writer,
            cancel,
        )?
    }
    writer
        .write_all(match format {
            ExportFormat::Html => b"</pre>" as &[u8],
            ExportFormat::Rtf => b"}",
        })
        .map_err(|_| UtilityError::Io)?;
    Ok(())
}
fn export_range(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    color: usize,
    styles: &ExportStyles<'_>,
    state: (ExportFormat, &mut bool),
    writer: &mut impl Write,
    cancel: &CancelToken,
) -> Result<(), UtilityError> {
    let (format, previous_cr) = state;
    let Rgb(r, g, b) = if color == 0 {
        styles.foreground
    } else {
        styles.colors[color - 1]
    };
    match format {
        ExportFormat::Html => write!(writer, "<span style=\"color:#{r:02x}{g:02x}{b:02x}\">"),
        ExportFormat::Rtf => write!(writer, "\\cf{} ", color + 1),
    }
    .map_err(|_| UtilityError::Io)?;
    for chunk in snapshot
        .chunks(range)
        .map_err(|_| UtilityError::InvalidRange)?
    {
        for (i, c) in chunk.chars().enumerate() {
            if i % 1024 == 0 {
                check(cancel)?
            }
            let was_cr = *previous_cr;
            *previous_cr = c == '\r';
            match format {
                ExportFormat::Html => match c {
                    '&' => writer.write_all(b"&amp;"),
                    '<' => writer.write_all(b"&lt;"),
                    '>' => writer.write_all(b"&gt;"),
                    '"' => writer.write_all(b"&quot;"),
                    '\'' => writer.write_all(b"&#39;"),
                    _ => {
                        let mut buf = [0u8; 4];
                        writer.write_all(c.encode_utf8(&mut buf).as_bytes())
                    }
                },
                ExportFormat::Rtf => match c {
                    '\\' => writer.write_all(b"\\\\"),
                    '{' => writer.write_all(b"\\{"),
                    '}' => writer.write_all(b"\\}"),
                    '\n' if was_cr => Ok(()),
                    '\n' | '\r' => writer.write_all(b"\\line "),
                    '\t' => writer.write_all(b"\\tab "),
                    c if c.is_ascii() => write!(writer, "{c}"),
                    _ => {
                        let mut units = [0u16; 2];
                        for unit in c.encode_utf16(&mut units) {
                            write!(writer, "\\u{}?", *unit as i16).map_err(|_| UtilityError::Io)?;
                        }
                        Ok(())
                    }
                },
            }
            .map_err(|_| UtilityError::Io)?;
        }
    }
    if matches!(format, ExportFormat::Html) {
        writer.write_all(b"</span>").map_err(|_| UtilityError::Io)?;
    }
    Ok(())
}
pub fn register_commands(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::*;
    for (id, title) in [
        ("utilities.md5", "MD5 (legacy integrity hash)"),
        ("utilities.sha1", "SHA-1 (legacy integrity hash)"),
        ("utilities.sha256", "SHA-256"),
        ("utilities.sha512", "SHA-512"),
        ("utilities.base64Encode", "Base64 encode"),
        ("utilities.base64Decode", "Base64 decode"),
        ("utilities.urlEncode", "URL encode"),
        ("utilities.urlDecode", "URL decode"),
        ("utilities.statistics", "Document statistics"),
        ("utilities.exportHtml", "Export syntax-colored HTML"),
        ("utilities.exportRtf", "Export syntax-colored RTF"),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "Utilities",
            shortcut: "",
            action: Action::Contributed(id),
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    fn doc(s: &str) -> Document {
        Document::from_utf8(s, Budget::new(10000), Budget::new(10000)).unwrap()
    }
    #[test]
    fn standard_hash_vectors() {
        for (algorithm, expected) in [
            (HashAlgorithm::Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (
                HashAlgorithm::Sha1,
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                HashAlgorithm::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                HashAlgorithm::Sha512,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ] {
            assert_eq!(
                hash_reader(&b"abc"[..], algorithm, &CancelToken::default(), |_| true)
                    .unwrap()
                    .hexadecimal,
                expected
            );
        }
    }
    #[test]
    fn invalid_decode_never_creates_edit_and_roundtrip_undo() {
        let mut d = doc("Hello 🙂");
        let s = d.snapshot();
        d.apply(
            transform(
                &s,
                TextOffset(0)..TextOffset(s.len()),
                Transform::Base64Encode,
                1000,
                &CancelToken::default(),
            )
            .unwrap(),
        )
        .unwrap();
        let s = d.snapshot();
        d.apply(
            transform(
                &s,
                TextOffset(0)..TextOffset(s.len()),
                Transform::Base64Decode,
                1000,
                &CancelToken::default(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            d.snapshot()
                .read(TextOffset(0)..TextOffset(d.snapshot().len()), 1000)
                .unwrap(),
            "Hello 🙂"
        );
        for text in ["%", "%GG", "%FF"] {
            let s = doc(text).snapshot();
            assert!(
                transform(
                    &s,
                    TextOffset(0)..TextOffset(s.len()),
                    Transform::UrlDecode,
                    1000,
                    &CancelToken::default()
                )
                .is_err()
            );
        }
        let s = doc("not base64!").snapshot();
        assert!(matches!(
            transform(
                &s,
                TextOffset(0)..TextOffset(s.len()),
                Transform::Base64Decode,
                1000,
                &CancelToken::default()
            ),
            Err(UtilityError::InvalidDecode)
        ));
    }
    #[test]
    fn html_escapes_and_rtf_keeps_unicode() {
        let s = doc("<script>&🙂").snapshot();
        let styles = ExportStyles {
            revision: s.revision,
            spans: &[],
            foreground: Rgb(1, 2, 3),
            colors: [Rgb(4, 5, 6); 5],
        };
        let mut out = Vec::new();
        export(
            &s,
            &styles,
            ExportFormat::Html,
            &mut out,
            &CancelToken::default(),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("&lt;script&gt;&amp;🙂"));
        assert!(!text.contains("<script>"));
        let mut out = Vec::new();
        export(
            &s,
            &styles,
            ExportFormat::Rtf,
            &mut out,
            &CancelToken::default(),
        )
        .unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("\\u-10179?\\u-8638?")
        );
    }
    #[test]
    fn huge_generated_hash_cancels_after_one_chunk() {
        let reader = std::io::repeat(0).take(20 * 1024 * 1024 * 1024);
        let mut calls = 0;
        assert!(matches!(
            hash_reader(
                reader,
                HashAlgorithm::Sha256,
                &CancelToken::default(),
                |n| {
                    calls += 1;
                    assert_eq!(n, 64 * 1024);
                    false
                }
            ),
            Err(UtilityError::Cancelled)
        ));
        assert_eq!(calls, 1);
    }
}
