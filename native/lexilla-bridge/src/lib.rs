// SPDX-License-Identifier: MPL-2.0
//! Bounded, synchronous Lexilla calls. Invoke only on a syntax worker.
//! Positions and styles are bytes in the caller's UTF-8 window, never raw-file offsets.
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
};
pub const MAX_BYTES: usize = 256 * 1024;
/// Bytes one [`LexerSession`] covers from document start: `session_next` in
/// bridge.cpp refuses any window ending past this offset, so styling at or
/// after it always comes from the caller's fallback. Keep the two in step.
pub const SESSION_BYTES: usize = 8 * 1024 * 1024;
/// A session keeps restart data at every `RESTART_LINES`-th line (the native
/// fallback checkpoint lines), at least `RESTART_GAP` bytes apart, so at most
/// `SESSION_BYTES / RESTART_GAP` restart lines. Mirrored in bridge.cpp.
pub const RESTART_LINES: usize = 256;
pub const RESTART_GAP: usize = 8 * 1024;
/// Lexilla's `LexAccessor` fills a 4000-byte buffer ending at the end of the
/// text it is given, so a session keeps whole lines reaching this far before
/// each window (besides the last window, within a cap), and
/// [`LexerSession::restart`] rewinds far enough that the replay to the
/// requested offset reaches it. Mirrored in bridge.cpp.
pub const LOOKBEHIND: usize = 4096;
/// Language-specific upstream options for lexers that several languages share
/// (C family, CSS dialects) or that need a non-default property (logs).
#[derive(Clone, Copy, Debug, Default)]
#[repr(u32)]
pub enum CppMode {
    #[default]
    Default = 0,
    JavaScript = 1,
    Go = 2,
    Java = 3,
    CSharp = 4,
    Scss = 5,
    Less = 6,
    /// `errorlist` styles only a recognised location prefix, not the whole line.
    ErrorList = 7,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    UnsupportedLexer,
    NativeFailure,
    Cancelled,
    UnavailableContext,
}
#[derive(Debug)]
pub struct Output {
    pub styles: Vec<u8>,
    pub line_states: Vec<i32>,
    pub fold_levels: Vec<i32>,
}
unsafe extern "C" {
    fn bareline_lexilla_lexer_count() -> usize;
    fn bareline_lexilla_lexer_name(index: usize) -> *const c_char;
    fn bareline_lexilla_session_create(name: *const c_char, keywords: *const c_char, mode: u32) -> *mut c_void;
    fn bareline_lexilla_session_destroy(handle: *mut c_void);
    fn bareline_lexilla_session_restart(handle: *mut c_void, offset: usize, resumed: *mut usize) -> c_int;
    fn bareline_lexilla_session_next(
        handle: *mut c_void,
        data: *const u8,
        size: usize,
        start: usize,
        styles: *mut u8,
        states: *mut i32,
        levels: *mut i32,
        capacity: usize,
        count: *mut usize,
        cancel: extern "C" fn(*mut c_void) -> c_int,
        context: *mut c_void,
    ) -> c_int;
    fn bareline_lexilla_lex(
        data: *const u8,
        size: usize,
        name: *const c_char,
        keywords: *const c_char,
        style: u8,
        state: i32,
        mode: u32,
        styles: *mut u8,
        states: *mut i32,
        levels: *mut i32,
        capacity: usize,
        count: *mut usize,
        cancel: extern "C" fn(*mut c_void) -> c_int,
        context: *mut c_void,
    ) -> c_int;
}
/// Names of every bundled Lexilla lexer module, in build order.
pub fn lexer_names() -> Vec<&'static str> {
    // SAFETY: reads the length of the static, immutable module table.
    let count = unsafe { bareline_lexilla_lexer_count() };
    (0..count)
        .filter_map(|index| {
            // SAFETY: the index is bounds-checked natively; the result is null
            // or a static NUL-terminated literal compiled into an upstream lexer.
            let name = unsafe { bareline_lexilla_lexer_name(index) };
            if name.is_null() {
                return None;
            }
            // SAFETY: non-null names are valid for the program's lifetime.
            unsafe { CStr::from_ptr(name) }.to_str().ok()
        })
        .collect()
}
/// Worker-owned opaque Lexilla instance. Neither Send nor Sync: creation, calls
/// and destruction must occur on its owning thread. Retains at most two byte
/// windows during a call, plus bounded restart data (see [`RESTART_GAP`]).
/// Cancellation or missing lookbehind invalidates it until [`restart`](Self::restart).
pub struct LexerSession {
    handle: std::ptr::NonNull<c_void>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl LexerSession {
    pub fn new(lexer: &str, keywords: &str, mode: CppMode) -> Result<Self, Error> {
        if lexer.len() > 64 || keywords.len() > 64 * 1024 {
            return Err(Error::InvalidInput);
        }
        let name = CString::new(lexer).map_err(|_| Error::InvalidInput)?;
        let words = CString::new(keywords).map_err(|_| Error::InvalidInput)?;
        // SAFETY: native copies options and returns an exclusively owned handle.
        let handle = unsafe { bareline_lexilla_session_create(name.as_ptr(), words.as_ptr(), mode as u32) };
        Ok(Self {
            handle: std::ptr::NonNull::new(handle).ok_or(Error::UnsupportedLexer)?,
            _thread: std::marker::PhantomData,
        })
    }
    /// Advance from zero in contiguous newline-terminated windows. A terminal
    /// partial line may be styled but cannot be continued. Missing retained
    /// lookbehind is reported explicitly and never reconstructed from guesses.
    pub fn advance<F: Fn() -> bool>(&mut self, text: &str, origin: usize, cancel: &F) -> Result<Output, Error> {
        if text.len() > MAX_BYTES {
            return Err(Error::InvalidInput);
        }
        let mut output = Output {
            styles: vec![0; text.len()],
            line_states: vec![0; text.len() + 1],
            fold_levels: vec![0; text.len() + 1],
        };
        let mut count = 0;
        // SAFETY: the handle is exclusively owned, all arrays have validated
        // capacity, and callback/context pointers are used synchronously only.
        let result = unsafe {
            bareline_lexilla_session_next(
                self.handle.as_ptr(),
                text.as_ptr(),
                text.len(),
                origin,
                output.styles.as_mut_ptr(),
                output.line_states.as_mut_ptr(),
                output.fold_levels.as_mut_ptr(),
                output.line_states.len(),
                &mut count,
                cancelled::<F>,
                (cancel as *const F).cast_mut().cast(),
            )
        };
        match result {
            0 if count <= output.line_states.len() => {
                output.line_states.truncate(count);
                output.fold_levels.truncate(count);
                Ok(output)
            }
            1 => Err(Error::InvalidInput),
            4 => Err(Error::Cancelled),
            5 => Err(Error::UnavailableContext),
            _ => Err(Error::NativeFailure),
        }
    }
    /// Rewind to the latest retained restart line at or before `offset` whose
    /// retained text plus the bytes from it to `offset` reach [`LOOKBEHIND`],
    /// and return its offset, where the next [`advance`](Self::advance) must
    /// start; the caller replays from there to `offset`, so a short window at
    /// `offset` still has its lookbehind. Later restart data is dropped.
    /// Upstream lexers resume from a line start with the same instance, as in
    /// Scintilla, so the continuation matches a pass from zero, provided the
    /// caller only rewinds where the text before `offset`, and the byte at it,
    /// are what this session lexed. Without such a line this is
    /// [`Error::UnavailableContext`].
    pub fn restart(&mut self, offset: usize) -> Result<usize, Error> {
        let mut resumed = 0;
        // SAFETY: the handle is exclusively owned and `resumed` outlives the call.
        let result = unsafe { bareline_lexilla_session_restart(self.handle.as_ptr(), offset, &mut resumed) };
        match result {
            0 if resumed <= offset => Ok(resumed),
            1 => Err(Error::InvalidInput),
            5 => Err(Error::UnavailableContext),
            _ => Err(Error::NativeFailure),
        }
    }
}
impl Drop for LexerSession {
    fn drop(&mut self) {
        // SAFETY: exactly one owner releases the native handle on its thread.
        unsafe { bareline_lexilla_session_destroy(self.handle.as_ptr()) };
    }
}
extern "C" fn cancelled<F: Fn() -> bool>(context: *mut c_void) -> c_int {
    // SAFETY: lex supplies this exact F and the synchronous native call never retains it.
    let callback = unsafe { &*(context.cast::<F>()) };
    // Keeps unwinding out of the FFI frame in unwind builds (tests); release aborts on panic.
    i32::from(catch_unwind(AssertUnwindSafe(callback)).unwrap_or(true))
}
/// One owned lexer and bounded IDocument per invocation; no C++ objects escape.
/// Initial style/state alone do not encode every lexer's private state. Callers must
/// label nonzero-origin windows provisional until verified from document start.
pub fn lex<F: Fn() -> bool>(
    text: &str,
    lexer: &str,
    keywords: &str,
    initial_style: u8,
    initial_line_state: i32,
    cancel: &F,
) -> Result<Output, Error> {
    lex_with_mode(
        text,
        lexer,
        keywords,
        initial_style,
        initial_line_state,
        cancel,
        CppMode::Default,
    )
}
/// As [`lex`], with a bounded, typed option profile. Lexers it does not name ignore it.
pub fn lex_with_mode<F: Fn() -> bool>(
    text: &str,
    lexer: &str,
    keywords: &str,
    initial_style: u8,
    initial_line_state: i32,
    cancel: &F,
    mode: CppMode,
) -> Result<Output, Error> {
    if text.len() > MAX_BYTES || lexer.len() > 64 || keywords.len() > 64 * 1024 {
        return Err(Error::InvalidInput);
    }
    let name = CString::new(lexer).map_err(|_| Error::InvalidInput)?;
    let words = CString::new(keywords).map_err(|_| Error::InvalidInput)?;
    let mut output = Output {
        styles: vec![0; text.len()],
        line_states: vec![0; text.len() + 1],
        fold_levels: vec![0; text.len() + 1],
    };
    let mut count = 0;
    // SAFETY: all buffers are live for this synchronous call and sized to the validated
    // byte quota; C++ validates capacity, catches exceptions and never retains pointers.
    let result = unsafe {
        bareline_lexilla_lex(
            text.as_ptr(),
            text.len(),
            name.as_ptr(),
            words.as_ptr(),
            initial_style,
            initial_line_state,
            mode as u32,
            output.styles.as_mut_ptr(),
            output.line_states.as_mut_ptr(),
            output.fold_levels.as_mut_ptr(),
            output.line_states.len(),
            &mut count,
            cancelled::<F>,
            (cancel as *const F).cast_mut().cast(),
        )
    };
    match result {
        0 if count <= output.line_states.len() => {
            output.line_states.truncate(count);
            output.fold_levels.truncate(count);
            Ok(output)
        }
        1 => Err(Error::InvalidInput),
        2 => Err(Error::UnsupportedLexer),
        4 => Err(Error::Cancelled),
        _ => Err(Error::NativeFailure),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_lexers_style_multiline_fixtures() {
        for (name, text, words) in [
            (
                "cpp",
                "int main() { /* comment\ncontinued */ return 42; }\n",
                "int return",
            ),
            (
                "python",
                "def f():\n    s = '''multi\nline'''\n    return 42\n",
                "def return",
            ),
            ("rust", "fn main() { let s = r#\"text\"#; }\n", "fn let"),
            ("hypertext", "<html><body><!--comment--></body></html>\n", "html body"),
            ("xml", "<root attr=\"v\"><!--c--></root>\n", "root"),
            ("css", "body { color: red; /* comment */ }\n", "color"),
            ("json", "{\"key\": true, \"n\": 42}\n", "true false null"),
            (
                "sql",
                "SELECT col FROM table WHERE n=42; -- comment\n",
                "select from where",
            ),
            ("toml", "[section]\nkey = \"value\" # comment\n", "true false"),
        ] {
            let output = lex(text, name, words, 0, 0, &|| false).unwrap();
            assert_eq!(output.styles.len(), text.len());
            assert!(output.styles.iter().any(|&s| s != 0), "{name}");
            assert_eq!(output.fold_levels.len(), output.line_states.len());
        }
    }
    #[test]
    fn ffi_rejects_bad_names_quotas_and_cancellation() {
        assert_eq!(
            lex("x", "missing", "", 0, 0, &|| false).unwrap_err(),
            Error::UnsupportedLexer
        );
        assert_eq!(lex("x", "cpp\0", "", 0, 0, &|| false).unwrap_err(), Error::InvalidInput);
        assert_eq!(
            lex(&"x".repeat(MAX_BYTES + 1), "cpp", "", 0, 0, &|| false).unwrap_err(),
            Error::InvalidInput
        );
        assert_eq!(lex("/*abc*/", "cpp", "", 0, 0, &|| true).unwrap_err(), Error::Cancelled);
    }
    #[test]
    fn utf8_crlf_empty_and_nul_are_bounded() {
        for text in ["", "\r\n", "// 🦀 café\r\nconst x = \"東京\";\0"] {
            let output = lex(text, "cpp", "const", 0, 0, &|| false).unwrap();
            assert_eq!(output.styles.len(), text.len());
        }
    }
}

#[cfg(test)]
mod accessor_fuzz {
    unsafe extern "C" {
        fn bareline_lexilla_probe(data: *const u8, size: usize) -> i32;
    }
    #[test]
    fn every_idocument_accessor_handles_generated_and_boundary_inputs() {
        let mut seed = 0x55aa1234u32;
        for length in 0..512 {
            let bytes: Vec<u8> = (0..length)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed as u8
                })
                .collect();
            // SAFETY: synchronous probe owns its copy, length matches the live allocation.
            assert_eq!(
                unsafe { bareline_lexilla_probe(bytes.as_ptr(), bytes.len()) },
                0,
                "length {length}"
            );
        }
    }
}

#[cfg(test)]
mod cancellation_progress {
    #[test]
    fn interrupts_inside_each_real_lexer() {
        use std::cell::Cell;
        let text = "word = 42; /* comment */\n".repeat(100);
        for name in [
            "cpp",
            "python",
            "rust",
            "hypertext",
            "xml",
            "css",
            "json",
            "sql",
            "toml",
        ] {
            for threshold in [5, 30, 100] {
                let calls = Cell::new(0);
                let cancelled = || {
                    calls.set(calls.get() + 1);
                    calls.get() > threshold
                };
                assert_eq!(
                    super::lex(&text, name, "word", 0, 0, &cancelled).unwrap_err(),
                    super::Error::Cancelled,
                    "{name} at {threshold}"
                );
            }
        }
    }
}

#[cfg(test)]
mod cpp_profiles {
    use super::*;
    #[test]
    fn language_profiles_keep_raw_and_template_contents_in_strings() {
        for (mode, text, marker) in [
            (CppMode::JavaScript, "const s = `line\n// { raw }`;", "// { raw }"),
            (CppMode::Go, "var s = `line\n// { raw }`", "// { raw }"),
            (CppMode::Java, "String s = \"\"\"\n// { raw }\n\"\"\";", "// { raw }"),
            (CppMode::CSharp, "var s = \"\"\"\n// { raw }\n\"\"\";", "// { raw }"),
        ] {
            let output = lex_with_mode(text, "cpp", "const var String", 0, 0, &|| false, mode).unwrap();
            let start = text.find(marker).unwrap();
            assert!(
                output.styles[start..start + marker.len()]
                    .iter()
                    .all(|s| matches!(s, 20 | 21)),
                "{mode:?}"
            );
        }
    }
}

#[cfg(test)]
mod sessions {
    use super::*;
    #[test]
    fn every_session_binding_styles_and_cancels_through_absolute_accessors() {
        let text = "word = 42; /* comment */\n".repeat(100);
        for name in [
            "cpp",
            "python",
            "rust",
            "hypertext",
            "xml",
            "css",
            "json",
            "sql",
            "toml",
        ] {
            let mut session = LexerSession::new(name, "word", CppMode::Default).unwrap();
            assert_eq!(session.advance(&text, 0, &|| false).unwrap().styles.len(), text.len());
            let mut session = LexerSession::new(name, "word", CppMode::Default).unwrap();
            let calls = std::cell::Cell::new(0);
            assert_eq!(
                session
                    .advance(&text, 0, &|| {
                        calls.set(calls.get() + 1);
                        calls.get() > 30
                    })
                    .unwrap_err(),
                Error::Cancelled
            );
        }
    }
    #[test]
    fn rolling_absolute_windows_match_one_pass() {
        let padding = "int padding;\n".repeat(800);
        let chunks = [
            format!("int f() {{\n{padding}"),
            format!("{padding}  /* comment\n"),
            format!("  continued */ return 1;\n{padding}"),
            "}\n".to_owned(),
        ];
        let full = chunks.concat();
        let expected = lex(&full, "cpp", "int return", 0, 0, &|| false).unwrap();
        let mut session = LexerSession::new("cpp", "int return", CppMode::Default).unwrap();
        let mut actual = Vec::new();
        for chunk in chunks {
            let output = session.advance(&chunk, actual.len(), &|| false).unwrap();
            actual.extend(output.styles);
        }
        assert_eq!(actual, expected.styles);
    }
    #[test]
    fn cancellation_invalidates_opaque_state() {
        let mut session = LexerSession::new("rust", "fn", CppMode::Default).unwrap();
        assert_eq!(
            session.advance("fn f() {}\n", 0, &|| true).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(
            session.advance("fn f() {}\n", 0, &|| false).unwrap_err(),
            Error::InvalidInput
        );
    }
    #[test]
    fn restart_resumes_from_a_retained_line_like_a_pass_from_zero() {
        // SRC-14: a session rewinds to a retained restart line and continues
        // over edited text there exactly as a fresh session over that text.
        let line = "int value = 42; /* comment */ // trailing note for spacing\n";
        assert!(RESTART_LINES * line.len() >= RESTART_GAP);
        let old = format!("{}{}", line.repeat(1_200), "int tail;\n".repeat(50));
        // An unclosed comment changes everything after it.
        let new = format!("{}/* {}", line.repeat(1_200), "int tail;\n".repeat(50));
        let split = 600 * line.len();
        let mut session = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        assert_eq!(session.restart(0).unwrap_err(), Error::UnavailableContext);
        session.advance(&old[..split], 0, &|| false).unwrap();
        session.advance(&old[split..], split, &|| false).unwrap();
        let resumed = session.restart(1_100 * line.len()).unwrap();
        assert_eq!(resumed, 1_024 * line.len());
        let mut fresh = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        fresh.advance(&new[..split], 0, &|| false).unwrap();
        fresh.advance(&new[split..resumed], split, &|| false).unwrap();
        let expected = fresh.advance(&new[resumed..], resumed, &|| false).unwrap();
        let actual = session.advance(&new[resumed..], resumed, &|| false).unwrap();
        assert_eq!(actual.styles, expected.styles);
        assert_eq!(actual.line_states, expected.line_states);
        assert_eq!(actual.fold_levels, expected.fold_levels);
        assert!(expected.styles.ends_with(&[1; 10]), "the tail is a comment");
        // A cancelled window keeps the restart lines at or before its start.
        assert_eq!(session.restart(1_100 * line.len()).unwrap(), resumed);
        assert_eq!(
            session.advance(&new[resumed..], resumed, &|| true).unwrap_err(),
            Error::Cancelled
        );
        assert_eq!(session.restart(1_100 * line.len()).unwrap(), resumed);
        let again = session.advance(&new[resumed..], resumed, &|| false).unwrap();
        assert_eq!(again.styles, expected.styles);
        assert_eq!(again.fold_levels, expected.fold_levels);
        // SRC-14: just after a restart line the rewind goes one line further,
        // so the replay fills LexAccessor's buffer; short windows after it
        // then match a pass from zero instead of reporting missing lookbehind.
        let offset = 1_030 * line.len();
        let replay = session.restart(offset).unwrap();
        assert_eq!(replay, 768 * line.len());
        session.advance(&new[replay..offset], replay, &|| false).unwrap();
        let mut fresh = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        fresh.advance(&new[..split], 0, &|| false).unwrap();
        fresh.advance(&new[split..offset], split, &|| false).unwrap();
        for window in [offset..1_040 * line.len(), 1_040 * line.len()..1_045 * line.len()] {
            assert!(window.len() < LOOKBEHIND / 4);
            let expected = fresh.advance(&new[window.clone()], window.start, &|| false).unwrap();
            let actual = session.advance(&new[window.clone()], window.start, &|| false).unwrap();
            assert_eq!(actual.styles, expected.styles);
            assert_eq!(actual.line_states, expected.line_states);
            assert_eq!(actual.fold_levels, expected.fold_levels);
        }
        // Restart data before it survives a restart; nothing before line 256
        // exists, and line 256 serves only offsets that fill the buffer.
        assert_eq!(session.restart(330 * line.len()).unwrap(), 256 * line.len());
        for offset in [300 * line.len(), 255 * line.len()] {
            assert_eq!(session.restart(offset).unwrap_err(), Error::UnavailableContext);
        }
    }
    #[test]
    fn short_windows_keep_their_lookbehind() {
        // SRC-14: windows far shorter than LexAccessor's buffer continue from
        // the text retained before them, like one pass.
        let chunks = ["int f() {\n", "/* comment\n", "continued */\n", "int g;\n"];
        let full = chunks.concat();
        let expected = lex(&full, "cpp", "int", 0, 0, &|| false).unwrap();
        let mut session = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        let mut actual = Vec::new();
        for chunk in chunks {
            let output = session.advance(chunk, actual.len(), &|| false).unwrap();
            actual.extend(output.styles);
        }
        assert_eq!(actual, expected.styles);
    }
    #[test]
    fn lookbehind_past_a_line_longer_than_the_cap_is_unavailable() {
        // Retained lookbehind stops at whole lines within its cap; a lexer that
        // needs text before them reports it rather than guessing.
        let long = format!("/* {}\n", "x".repeat(70 * 1024));
        let mut session = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        session.advance(&long, 0, &|| false).unwrap();
        session.advance("a\n", long.len(), &|| false).unwrap();
        assert_eq!(
            session
                .advance("continued */\n", long.len() + 2, &|| false)
                .unwrap_err(),
            Error::UnavailableContext
        );
    }
}

#[cfg(test)]
mod full_set {
    use super::*;
    #[test]
    fn complete_upstream_lexer_set_is_registered_and_lexes() {
        let names = lexer_names();
        // Lexilla 5.5.3 defines 139 lexer modules in its 125 lexers/ sources; LexEDIFACT
        // (one module) is excluded because it reads past short documents (build.rs).
        assert_eq!(names.len(), 138);
        assert!(!names.contains(&"edifact"));
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        for name in [
            "a68k",
            "asm",
            "au3",
            "bash",
            "batch",
            "cmake",
            "dart",
            "diff",
            "erlang",
            "errorlist",
            "fortran",
            "haskell",
            "inno",
            "latex",
            "lua",
            "makefile",
            "markdown",
            "nsis",
            "pascal",
            "perl",
            "phpscript",
            "powershell",
            "props",
            "r",
            "registry",
            "ruby",
            "tcl",
            "tex",
            "vb",
            "vbscript",
            "yaml",
            "zig",
        ] {
            assert!(names.contains(&name), "{name}");
        }
        let text = "word 42 \"s\" 'c' # c // c -- c ; c\n(x) {y} <tag> $v\n";
        for name in names {
            let output = lex(text, name, "word", 0, 0, &|| false).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(output.styles.len(), text.len(), "{name}");
            let mut session = LexerSession::new(name, "word", CppMode::Default).unwrap();
            assert_eq!(
                session.advance(text, 0, &|| false).unwrap().styles.len(),
                text.len(),
                "{name}"
            );
        }
    }
    #[test]
    fn newline_separated_keyword_sets_reach_later_word_lists() {
        // SCE_LUA_WORD2: the second word list, "Basic functions".
        let output = lex("print(1)\n", "lua", "local\nprint", 0, 0, &|| false).unwrap();
        assert!(output.styles[..5].iter().all(|&s| s == 13), "{:?}", output.styles);
        let output = lex("print(1)\n", "lua", "local print", 0, 0, &|| false).unwrap();
        assert!(output.styles[..5].iter().all(|&s| s == 5), "{:?}", output.styles);
    }
    #[test]
    fn css_profiles_enable_scss_and_less_line_comments() {
        let text = "// note\na { color: red; }\n";
        for mode in [CppMode::Scss, CppMode::Less] {
            let output = lex_with_mode(text, "css", "color", 0, 0, &|| false, mode).unwrap();
            // SCE_CSS_COMMENT
            assert!(output.styles[..7].iter().all(|&s| s == 9), "{mode:?}");
        }
        let plain = lex(text, "css", "color", 0, 0, &|| false).unwrap();
        assert_ne!(plain.styles[2], 9);
    }
    #[test]
    fn errorlist_profile_separates_the_location_from_the_message() {
        let text = "src/main.c:12:5: error: oops\n";
        let message = text.find("error").unwrap();
        // SCE_ERR_GCC for the location, SCE_ERR_VALUE for the message.
        let output = lex_with_mode(text, "errorlist", "", 0, 0, &|| false, CppMode::ErrorList).unwrap();
        assert_eq!((output.styles[0], output.styles[message]), (2, 21));
        let plain = lex(text, "errorlist", "", 0, 0, &|| false).unwrap();
        assert_eq!((plain.styles[0], plain.styles[message]), (2, 2));
    }
}
