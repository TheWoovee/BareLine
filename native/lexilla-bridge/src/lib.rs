// SPDX-License-Identifier: MPL-2.0
//! Bounded, synchronous Lexilla calls. Invoke only on a syntax worker.
//! Positions and styles are bytes in the caller's UTF-8 window, never raw-file offsets.
use std::{
    ffi::{CString, c_char, c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
};
pub const MAX_BYTES: usize = 256 * 1024;
/// Language-specific upstream options for the shared C-family lexer.
#[derive(Clone, Copy, Debug, Default)]
#[repr(u32)]
pub enum CppMode {
    #[default]
    Default = 0,
    JavaScript = 1,
    Go = 2,
    Java = 3,
    CSharp = 4,
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
    fn bareline_lexilla_session_create(name: *const c_char, keywords: *const c_char, mode: u32) -> *mut c_void;
    fn bareline_lexilla_session_destroy(handle: *mut c_void);
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
/// Worker-owned opaque Lexilla instance. Neither Send nor Sync: creation, calls
/// and destruction must occur on its owning thread. Retains at most two byte
/// windows during a call. Cancellation or missing lookbehind invalidates it.
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
/// As [`lex`], with a bounded, typed C-family option profile. Other lexers ignore it.
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
    fn unretained_comment_lookbehind_is_unavailable() {
        let mut session = LexerSession::new("cpp", "int", CppMode::Default).unwrap();
        session.advance("int f() {\n", 0, &|| false).unwrap();
        session.advance("/* comment\n", 10, &|| false).unwrap();
        assert_eq!(
            session.advance("continued */\n", 21, &|| false).unwrap_err(),
            Error::UnavailableContext
        );
    }
}
