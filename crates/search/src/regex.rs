// SPDX-License-Identifier: MPL-2.0
//! PCRE2 hard-partial streaming with an exact bounded fallback for contextual syntax.
use super::*;
use pcre2_sys::*;
use std::{
    cell::Cell,
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};

pub const SUBJECT_LIMIT: usize = 16 * 1024 * 1024;
/// Complete context budget; no source prefix is discarded and all subject anchors stay exact.
pub const CONTEXT_LIMIT: usize = 64 * 1024 * 1024;
pub(super) type Captures = Vec<Option<Range<TextOffset>>>;
pub(super) fn capture_names(query: &SearchQuery) -> Result<Vec<(String, usize)>, Completeness> {
    if query.pattern.len() > MAX_PATTERN_BYTES {
        return Err(Completeness::InvalidQuery);
    }
    Engine::new(query)?.names()
}
pub(super) const STREAM_WINDOW: usize = 256 * 1024;
const PARTIAL_CONTEXT: usize = 8 * 1024 * 1024;
/// Every engine call gets its own deadline: a fixed allowance plus time to scan the bytes
/// it may inspect, so many quick matches never share one scan-wide budget (SRC-03).
const ATTEMPT_BUDGET: Duration = Duration::from_secs(2);
/// About 15 MiB/s, far below PCRE2's scan rate with automatic callouts.
const ATTEMPT_NANOS_PER_BYTE: u64 = 64;
/// Callouts between clock and cancellation reads; the first callout of a call reads both.
const CLOCK_STRIDE: u32 = 256;
fn attempt_deadline(bytes: usize) -> Instant {
    Instant::now() + ATTEMPT_BUDGET + Duration::from_nanos((bytes as u64).saturating_mul(ATTEMPT_NANOS_PER_BYTE))
}
/// Streaming drops the text before each resume point, so a pattern streams only when its
/// compiled form never inspects that text (SRC-12). The decision comes from PCRE2's view of
/// the compiled pattern, not its spelling: `[^,]+` and named groups stream, while lookbehind,
/// `\b`, `\B` and `\A` (PCRE2_INFO_MAXLOOKBEHIND) and `^`, `$`, `\G`, `\X` and verb items
/// (the auto-callout item list) keep the exact subject path.
pub(super) fn streamable(query: &SearchQuery) -> Result<bool, Completeness> {
    if query.pattern.len() > MAX_PATTERN_BYTES {
        return Err(Completeness::InvalidQuery);
    }
    Ok(Engine::new(query)?.streamable())
}
enum PartialMatch {
    Match(Captures),
    Partial(usize),
    None,
}

// pcre2-sys omits callout bindings. Signatures and the version-0 prefixes of
// pcre2_callout_block and pcre2_callout_enumerate_block are from the bundled pcre2.h
// (10.46); PCRE2 owns the blocks and calls synchronously on the calling thread.
unsafe extern "C" {
    fn pcre2_set_callout_8(
        context: *mut pcre2_match_context_8,
        callback: Option<unsafe extern "C" fn(*const CalloutBlock, *mut c_void) -> i32>,
        data: *mut c_void,
    ) -> i32;
    fn pcre2_callout_enumerate_8(
        code: *const pcre2_code_8,
        callback: Option<unsafe extern "C" fn(*const CalloutItem, *mut c_void) -> i32>,
        data: *mut c_void,
    ) -> i32;
}
/// Read-only prefix of the C layout; only the subject and position fields are read.
#[allow(dead_code)]
#[repr(C)]
struct CalloutBlock {
    version: u32,
    callout_number: u32,
    capture_top: u32,
    capture_last: u32,
    offset_vector: *mut usize,
    mark: *const u8,
    subject: *const u8,
    subject_length: usize,
    start_match: usize,
    current_position: usize,
    pattern_position: usize,
    next_item_length: usize,
}
/// Read-only C layout of one enumerated callout; only the item offset is read.
#[allow(dead_code)]
#[repr(C)]
struct CalloutItem {
    version: u32,
    pattern_position: usize,
    next_item_length: usize,
    callout_number: u32,
    callout_string_offset: usize,
    callout_string_length: usize,
    callout_string: *const u8,
}
struct Interrupt<'a> {
    job: &'a SearchJob,
    deadline: Instant,
}
struct Callout<'a, 'b> {
    state: &'a Interrupt<'b>,
    pattern: &'a [u8],
    /// Whole-word boundaries are checked here as well as by the caller.
    word_callout: bool,
    /// Callouts seen during this engine call.
    ticks: Cell<u32>,
    /// The attempt start of the last rejected whole-word candidate, and the rejections there.
    rejected: Cell<(usize, u32)>,
}
/// Whole-word candidates the final callout rejects at one attempt start before it lets the
/// next one through for the caller to reject, so a long word run cannot backtrack into
/// the match limit.
const WORD_REJECTIONS: u32 = 1024;
unsafe extern "C" fn interrupt(block: *const CalloutBlock, data: *mut c_void) -> i32 {
    // SAFETY: Engine::run/partial keep this stack value alive for the synchronous match call.
    let callout = unsafe { &*(data as *const Callout<'_, '_>) };
    let ticks = callout.ticks.get();
    callout.ticks.set(ticks.wrapping_add(1));
    if ticks.is_multiple_of(CLOCK_STRIDE)
        && (callout.state.job.is_cancelled() || Instant::now() >= callout.state.deadline)
    {
        return PCRE2_ERROR_CALLOUT;
    }
    // SAFETY: PCRE2 passes a live block whose subject spans subject_length bytes.
    let block = unsafe { &*block };
    let at = block.current_position;
    if callout.word_callout {
        // SAFETY: the subject is the caller's &str, so these bytes are valid UTF-8.
        let subject =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(block.subject, block.subject_length)) };
        // No candidate from a start that follows a word character can pass, so the attempt
        // fails at its first callout and a word run costs one step per character.
        if !word::text_start_boundary(subject, block.start_match) {
            return 1;
        }
        if block.pattern_position == callout.pattern.len() {
            // The final callout follows the group around the whole pattern (see
            // `whole_word_groups`), so it sees every complete candidate. Failing it makes
            // PCRE2 backtrack into the pattern's other choices (`xy` in `x|xy`) rather than
            // the caller discarding the only candidate tried at this start (SRC-16).
            if word::text_boundaries(subject, block.start_match, at) {
                return 0;
            }
            let (start, count) = callout.rejected.get();
            let count = if start == block.start_match {
                count.saturating_add(1)
            } else {
                1
            };
            callout.rejected.set((block.start_match, count));
            return i32::from(count <= WORD_REJECTIONS);
        }
    }
    // ANYCRLF also accepts a lone CR or LF as a newline, so PCRE2 lets `^` and `$`
    // match between the CR and LF of one CRLF. Notepad++ never does; a positive
    // return fails this item and backtracks, so `\s+$` stops before the CR.
    let anchor = matches!(callout.pattern.get(block.pattern_position), Some(b'^' | b'$'));
    // SAFETY: 0 < at < subject_length, so both bytes are inside the subject.
    let inside_crlf = anchor
        && at > 0
        && at < block.subject_length
        && unsafe { *block.subject.add(at - 1) == b'\r' && *block.subject.add(at) == b'\n' };
    i32::from(inside_crlf)
}
/// Items that read text before the start offset: `^`, `$` and the CRLF guard read the
/// previous character, `\G` is the start offset itself, `\X` looks back over regional
/// indicators, and verbs such as (*COMMIT) depend on where an engine call began.
unsafe extern "C" fn stream_hazard(item: *const CalloutItem, data: *mut c_void) -> i32 {
    // SAFETY: Engine::streamable passes its live pattern slice, and PCRE2 a live block,
    // for this synchronous enumeration only.
    let (pattern, item) = unsafe { (*(data as *const &[u8]), &*item) };
    let next = pattern.get(item.pattern_position..).unwrap_or_default();
    i32::from(
        matches!(next.first(), Some(b'^' | b'$'))
            || [b"\\G".as_slice(), b"\\X".as_slice(), b"(*".as_slice()]
                .iter()
                .any(|prefix| next.starts_with(prefix)),
    )
}
/// Name the bound behind a failed engine call (SRC-06). The callout fails a call only for
/// cancellation, which callers check first, or for the attempt deadline.
fn limit(code: i32) -> Completeness {
    Completeness::RegexLimit(match code {
        PCRE2_ERROR_CALLOUT => RegexLimitKind::Time,
        PCRE2_ERROR_MATCHLIMIT => RegexLimitKind::Backtracking,
        PCRE2_ERROR_DEPTHLIMIT => RegexLimitKind::Depth,
        PCRE2_ERROR_HEAPLIMIT | PCRE2_ERROR_NOMEMORY => RegexLimitKind::Memory,
        _ => RegexLimitKind::Engine,
    })
}
/// Compile with the document newline settings; the caller owns the returned code.
fn compile(pattern: &[u8], options: u32) -> Result<*mut pcre2_code_8, Completeness> {
    // SAFETY: The pattern slice is valid for the call, and the context is freed on
    // every path after its creation.
    unsafe {
        let compile = pcre2_compile_context_create_8(ptr::null_mut());
        if compile.is_null() {
            return Err(Completeness::RegexLimit(RegexLimitKind::Memory));
        }
        pcre2_set_max_pattern_compiled_length_8(compile, 1024 * 1024);
        pcre2_set_parens_nest_limit_8(compile, 250);
        // Documents keep CRLF, LF and CR line breaks; `.` and `$` must treat each as
        // one newline, and `\R` matches exactly those breaks (SRC-01).
        pcre2_set_newline_8(compile, PCRE2_NEWLINE_ANYCRLF);
        pcre2_set_bsr_8(compile, PCRE2_BSR_ANYCRLF);
        let mut error = 0;
        let mut offset = 0;
        let code = pcre2_compile_8(
            pattern.as_ptr(),
            pattern.len(),
            options,
            &mut error,
            &mut offset,
            compile,
        );
        pcre2_compile_context_free_8(compile);
        if code.is_null() {
            return Err(Completeness::InvalidQuery);
        }
        Ok(code)
    }
}
/// Byte length of the leading start-of-pattern options such as (*UCP) or
/// (*LIMIT_MATCH=9), which PCRE2 accepts only at the very start. Backtracking verbs
/// such as (*FAIL) are pattern items, not options.
fn start_options(pattern: &str) -> usize {
    let mut end = 0;
    while let Some(rest) = pattern[end..].strip_prefix("(*") {
        let Some(close) = rest.find(')') else {
            break;
        };
        let name = &rest[..close];
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'=')
            || matches!(name, "ACCEPT" | "COMMIT" | "F" | "FAIL" | "PRUNE" | "SKIP" | "THEN")
        {
            break;
        }
        end += close + 3;
    }
    end
}
/// Whether the callouts may reject whole-word candidates. They see the attempt start,
/// not a `\K` start; they take the end of a (?R) recursion for the end of the pattern;
/// and after (*COMMIT) and similar verbs a rejection fails the whole engine call. Such
/// patterns keep only the caller's check. The test reads the spelling and is
/// deliberately broad: a false positive only costs the in-engine backtracking.
fn words_in_callout(pattern: &str) -> bool {
    let body = &pattern.as_bytes()[start_options(pattern)..];
    !(0..body.len()).any(|i| {
        let rest = &body[i..];
        if rest.starts_with(b"\\K") || rest.starts_with(b"(*") {
            return true;
        }
        // (?R), (?0), (?+0), \g<0>, \g'0' and the like call the whole pattern.
        let Some(call) = [b"(?".as_slice(), b"\\g<".as_slice(), b"\\g'".as_slice()]
            .into_iter()
            .find_map(|prefix| rest.strip_prefix(prefix))
        else {
            return false;
        };
        let call = call
            .strip_prefix(b"+".as_slice())
            .or_else(|| call.strip_prefix(b"-".as_slice()))
            .unwrap_or(call);
        let zeros = call.iter().take_while(|b| **b == b'0').count();
        call.first() == Some(&b'R') || (zeros > 0 && !call.get(zeros).is_some_and(u8::is_ascii_digit))
    })
}
/// PCRE2 ends every top-level alternative but the last at the automatic callout before
/// its `|`, so only one group around the whole pattern gives every candidate the same
/// final callout (SRC-16). Start-of-pattern options stay in front, `\E` closes a
/// trailing `\Q` (an orphan `\E` is ignored), and the second form, tried only when the
/// first fails to compile, ends a trailing (?x) comment that swallowed the `)`.
fn whole_word_groups(pattern: &str) -> [String; 2] {
    let (options, body) = pattern.split_at(start_options(pattern));
    [format!("{options}(?:{body}\\E)"), format!("{options}(?:{body}\\E\r\n)")]
}
struct Engine {
    code: *mut pcre2_code_8,
    data: *mut pcre2_match_data_8,
    context: *mut pcre2_match_context_8,
    /// Pattern bytes for the callout; PCRE2 reports item offsets into them.
    pattern: Box<[u8]>,
    /// Whole-word query: never streamed, and candidates are checked by the caller.
    whole_word: bool,
    /// Whole-word candidates are also checked by the callouts (see [`words_in_callout`]).
    word_callout: bool,
    /// CRLF is a newline, so an empty match advances over it as one unit (pcre2demo).
    crlf_newline: bool,
}
impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: These are exclusively owned PCRE2 allocations; free accepts null.
        unsafe {
            pcre2_match_context_free_8(self.context);
            pcre2_match_data_free_8(self.data);
            pcre2_code_free_8(self.code);
        }
    }
}
impl Engine {
    fn partial(
        &mut self,
        text: &str,
        start: usize,
        options: u32,
        state: &mut Interrupt<'_>,
        eof: bool,
    ) -> Result<PartialMatch, Completeness> {
        // PCRE2_NO_UTF_CHECK requires a character-boundary start offset.
        if !text.is_char_boundary(start) {
            return Err(Completeness::UnsupportedStreaming);
        }
        let callout = Callout {
            state: &*state,
            pattern: &self.pattern,
            word_callout: self.word_callout,
            ticks: Cell::new(0),
            rejected: Cell::new((0, 0)),
        };
        // SAFETY: allocations are owned by Engine; the UTF-8 subject and synchronous
        // callout state remain live throughout the call and ovector copy. The subject is
        // a &str and start a character boundary, so PCRE2's UTF check, which rescans the
        // rest of the subject on every call, is skipped (SRC-03).
        unsafe {
            pcre2_set_callout_8(self.context, Some(interrupt), ptr::from_ref(&callout).cast_mut().cast());
            let code = pcre2_match_8(
                self.code,
                text.as_ptr(),
                text.len(),
                start,
                options | PCRE2_NO_UTF_CHECK | if eof { 0 } else { PCRE2_PARTIAL_HARD },
                self.data,
                self.context,
            );
            if state.job.is_cancelled() {
                return Err(Completeness::Cancelled);
            }
            if Instant::now() >= state.deadline {
                return Err(Completeness::RegexLimit(RegexLimitKind::Time));
            }
            if code == PCRE2_ERROR_NOMATCH {
                return Ok(PartialMatch::None);
            }
            let vector = pcre2_get_ovector_pointer_8(self.data);
            if code == PCRE2_ERROR_PARTIAL {
                let at = *vector;
                if at > text.len() || !text.is_char_boundary(at) {
                    return Err(Completeness::UnsupportedStreaming);
                }
                return Ok(PartialMatch::Partial(at));
            }
            if code < 0 {
                return Err(limit(code));
            }
            let values = std::slice::from_raw_parts(vector, pcre2_get_ovector_count_8(self.data) as usize * 2);
            let mut captures = Vec::new();
            for pair in values.as_chunks::<2>().0 {
                captures.push(if pair[0] == usize::MAX {
                    None
                } else {
                    if pair[0] > pair[1]
                        || pair[1] > text.len()
                        || !text.is_char_boundary(pair[0])
                        || !text.is_char_boundary(pair[1])
                    {
                        return Err(Completeness::UnsupportedStreaming);
                    }
                    Some(TextOffset(pair[0])..TextOffset(pair[1]))
                });
            }
            Ok(PartialMatch::Match(captures))
        }
    }
    fn names(&self) -> Result<Vec<(String, usize)>, Completeness> {
        // SAFETY: PCRE2 returns a table owned by code, still live through this copy.
        unsafe {
            let mut count = 0u32;
            let mut size = 0u32;
            let mut table: *const u8 = ptr::null();
            pcre2_pattern_info_8(self.code, PCRE2_INFO_NAMECOUNT, (&mut count as *mut u32).cast());
            pcre2_pattern_info_8(self.code, PCRE2_INFO_NAMEENTRYSIZE, (&mut size as *mut u32).cast());
            pcre2_pattern_info_8(self.code, PCRE2_INFO_NAMETABLE, (&mut table as *mut *const u8).cast());
            let mut names = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let entry = std::slice::from_raw_parts(table.add(index * size as usize), size as usize);
                let number = u16::from_be_bytes([entry[0], entry[1]]) as usize;
                let bytes = entry[2..].split(|b| *b == 0).next().unwrap();
                let name = std::str::from_utf8(bytes).map_err(|_| Completeness::InvalidQuery)?;
                names.push((name.into(), number));
            }
            Ok(names)
        }
    }
    /// See [`streamable`].
    fn streamable(&self) -> bool {
        if self.whole_word {
            return false;
        }
        let pattern: &[u8] = &self.pattern;
        let mut lookbehind = 0u32;
        // SAFETY: code is live; the callback reads `pattern` only during this synchronous call.
        unsafe {
            pcre2_pattern_info_8(
                self.code,
                PCRE2_INFO_MAXLOOKBEHIND,
                (&mut lookbehind as *mut u32).cast(),
            ) == 0
                && lookbehind == 0
                && pcre2_callout_enumerate_8(
                    self.code,
                    Some(stream_hazard),
                    ptr::from_ref(&pattern).cast_mut().cast(),
                ) == 0
        }
    }
    /// Where iteration resumes after an empty match that no non-empty match at the same
    /// position replaced: one character on, or past a whole CRLF when CRLF is a newline.
    fn advance(&self, text: &str, start: usize) -> Option<usize> {
        let rest = text.get(start..)?;
        if self.crlf_newline && rest.starts_with("\r\n") {
            return Some(start + 2);
        }
        rest.chars().next().map(|c| start + c.len_utf8())
    }
    fn new(query: &SearchQuery) -> Result<Self, Completeness> {
        // Keep PCRE2's literal-prefix/start optimizations: disabling them invokes
        // a callout at every candidate byte and exhausts the deadline on an
        // ordinary 20 MiB search. Optimized subject scans are bounded by the
        // 64 MiB context cap; matching still has automatic callouts and limits,
        // and cancellation is checked before/after each engine invocation.
        // `^`/`$` match at line boundaries like Notepad++ (decision D1, SRC-02);
        // `(?-m)` restores document anchors.
        // PCRE2_CASELESS folds one character to one character (Σ/σ/ς match), unlike
        // literal search's full folding: `strasse` does not match `Straße` (SRC-17).
        let options = PCRE2_UTF
            | PCRE2_UCP
            | PCRE2_AUTO_CALLOUT
            | PCRE2_NEVER_BACKSLASH_C
            | PCRE2_MULTILINE
            | if query.case == Case::Folded { PCRE2_CASELESS } else { 0 }
            | if query.dot_matches_newline { PCRE2_DOTALL } else { 0 };
        let mut code = compile(query.pattern.as_bytes(), options)?;
        let mut pattern: Box<[u8]> = query.pattern.as_bytes().into();
        let word_callout = query.whole_word && words_in_callout(&query.pattern);
        if word_callout {
            // Auto-possession skips callouts, so `[a-z ]+` at the end would never give
            // back characters when the final callout rejects its longest candidate.
            let grouped = whole_word_groups(&query.pattern)
                .into_iter()
                .find_map(|group| Some((compile(group.as_bytes(), options | PCRE2_NO_AUTO_POSSESS).ok()?, group)));
            if let Some((grouped, group)) = grouped {
                // SAFETY: code is the exclusively owned compile result above, now replaced.
                unsafe { pcre2_code_free_8(code) };
                code = grouped;
                pattern = group.into_bytes().into();
            }
        }
        // SAFETY: code is a live compiled pattern. PCRE2 ownership is transferred to
        // Engine immediately; all failure paths free their allocations.
        unsafe {
            let mut newline = 0u32;
            pcre2_pattern_info_8(code, PCRE2_INFO_NEWLINE, (&mut newline as *mut u32).cast());
            let engine = Self {
                code,
                data: pcre2_match_data_create_from_pattern_8(code, ptr::null_mut()),
                context: pcre2_match_context_create_8(ptr::null_mut()),
                pattern,
                whole_word: query.whole_word,
                word_callout,
                crlf_newline: matches!(newline, PCRE2_NEWLINE_CRLF | PCRE2_NEWLINE_ANY | PCRE2_NEWLINE_ANYCRLF),
            };
            if engine.data.is_null() || engine.context.is_null() {
                return Err(Completeness::RegexLimit(RegexLimitKind::Memory));
            }
            pcre2_set_match_limit_8(engine.context, 1_000_000);
            // Each nested backtracking point is a heap frame, so the heap limit below
            // already bounds depth. A small depth limit cut off ordinary group
            // repetitions such as a 2,000-character JSON string (SRC-06).
            pcre2_set_depth_limit_8(engine.context, 1_000_000);
            pcre2_set_heap_limit_8(engine.context, 8 * 1024); // KiB
            Ok(engine)
        }
    }
    fn run(
        &mut self,
        text: &str,
        start: usize,
        options: u32,
        state: &mut Interrupt<'_>,
    ) -> Result<Option<Captures>, Completeness> {
        // PCRE2_NO_UTF_CHECK requires a character-boundary start offset.
        if !text.is_char_boundary(start) {
            return Err(Completeness::UnsupportedStreaming);
        }
        let callout = Callout {
            state: &*state,
            pattern: &self.pattern,
            word_callout: self.word_callout,
            ticks: Cell::new(0),
            rejected: Cell::new((0, 0)),
        };
        // SAFETY: Engine owns live allocations; subject and state outlive this synchronous
        // non-JIT call. The returned ovector belongs to data and is copied before reuse.
        // The subject is a &str, so the per-call UTF rescan is skipped (SRC-03).
        unsafe {
            pcre2_set_callout_8(self.context, Some(interrupt), ptr::from_ref(&callout).cast_mut().cast());
            let code = pcre2_match_8(
                self.code,
                text.as_ptr(),
                text.len(),
                start,
                options | PCRE2_NO_UTF_CHECK,
                self.data,
                self.context,
            );
            if state.job.is_cancelled() {
                return Err(Completeness::Cancelled);
            }
            if code == PCRE2_ERROR_NOMATCH {
                return Ok(None);
            }
            if code < 0 {
                return Err(limit(code));
            }
            let count = pcre2_get_ovector_count_8(self.data) as usize;
            let values = std::slice::from_raw_parts(pcre2_get_ovector_pointer_8(self.data), count * 2);
            let mut captures = Vec::with_capacity(count);
            for pair in values.as_chunks::<2>().0 {
                captures.push(if pair[0] == usize::MAX {
                    None
                } else {
                    if pair[0] > pair[1] || !text.is_char_boundary(pair[0]) || !text.is_char_boundary(pair[1]) {
                        return Err(Completeness::UnsupportedStreaming);
                    }
                    Some(TextOffset(pair[0])..TextOffset(pair[1]))
                });
            }
            Ok(Some(captures))
        }
    }
}
/// pcre2demo's options for the next attempt: after an empty match, first look for a
/// non-empty match anchored at the same position (`|a` on "a" finds "", "a", "") (SRC-16).
fn next_options(after_empty: bool) -> u32 {
    if after_empty {
        PCRE2_NOTEMPTY_ATSTART | PCRE2_ANCHORED
    } else {
        0
    }
}
/// Retain PCRE2's earliest hard-partial candidate, not an arbitrary fixed overlap.
/// A candidate exceeding the bounded context fails explicitly; no matches are skipped.
pub(super) fn scan_stream(
    length: usize,
    query: &SearchQuery,
    job: &SearchJob,
    mut read: impl FnMut(usize, usize) -> Result<String, Completeness>,
    mut emit: impl FnMut(Captures) -> Result<(), Completeness>,
) -> Result<(), Completeness> {
    if query.pattern.len() > MAX_PATTERN_BYTES {
        return Err(Completeness::InvalidQuery);
    }
    let mut engine = Engine::new(query)?;
    if !engine.streamable() {
        return Err(Completeness::UnsupportedStreaming);
    }
    let selection = query.selection.clone().unwrap_or(TextOffset(0)..TextOffset(length));
    if selection.start > selection.end || selection.end.0 > length {
        return Err(Completeness::InvalidQuery);
    }
    let mut text = String::new();
    let mut base = selection.start.0;
    let mut next = base;
    let mut start = 0usize;
    let mut after_empty = false;
    let mut state = Interrupt {
        job,
        deadline: Instant::now(),
    };
    loop {
        if job.is_cancelled() {
            return Err(Completeness::Cancelled);
        }
        if next < length {
            let part = read(next, STREAM_WINDOW.min(length - next))?;
            if part.is_empty() || part.len() > length - next {
                return Err(Completeness::Unsupported);
            }
            if text.len().saturating_add(part.len()) > PARTIAL_CONTEXT {
                return Err(Completeness::UnsupportedStreaming);
            }
            next += part.len();
            text.push_str(&part);
        }
        let eof = next == length;
        let retain = loop {
            if base + start > selection.end.0 {
                return Ok(());
            }
            state.deadline = attempt_deadline(text.len() - start);
            match engine.partial(&text, start, next_options(after_empty), &mut state, eof)? {
                PartialMatch::None if after_empty => {
                    // A CR at a temporary edge may begin a CRLF; wait for the next window.
                    if !eof && start + 1 == text.len() && text.ends_with('\r') {
                        break start;
                    }
                    after_empty = false;
                    match engine.advance(&text, start) {
                        Some(advanced) => start = advanced,
                        None => break text.len(),
                    }
                }
                // Keep a trailing CR so the next call starts before it: PCRE2's own
                // bumpalong then skips the LF of a CRLF as it does on the whole subject.
                PartialMatch::None if !eof && start < text.len() && text.ends_with('\r') => {
                    break text.len() - 1;
                }
                PartialMatch::None => break text.len(),
                PartialMatch::Partial(at) => break at,
                PartialMatch::Match(mut captures) => {
                    let range = captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
                    // An empty match at a temporary edge must wait for the next scalar.
                    if range.is_empty() && range.end.0 == text.len() && !eof {
                        break range.start.0;
                    }
                    for capture in captures.iter_mut().flatten() {
                        capture.start.0 += base;
                        capture.end.0 += base;
                    }
                    emit(captures)?;
                    start = range.end.0;
                    after_empty = range.is_empty();
                }
            }
        };
        if eof {
            return Ok(());
        }
        text.drain(..retain);
        base += retain;
        start = 0;
    }
}
/// Count one match and retain it within the result budget. Past the budget a match is only
/// counted, and only when the query asks for a complete count (SRC-21). Ok(true) = retained.
fn record(
    result: &mut SearchResults,
    captures: Captures,
    used: &mut usize,
    limited: &mut bool,
    query: &SearchQuery,
) -> Result<bool, Completeness> {
    let range = captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
    result.total_count += 1;
    if !*limited {
        *used = used.saturating_add(
            std::mem::size_of::<SearchMatch>()
                + std::mem::size_of::<Captures>()
                + captures.len() * std::mem::size_of::<Option<Range<TextOffset>>>(),
        );
        *limited = *used > query.results_ram_bytes.min(MAX_RESULT_BYTES);
        if !*limited {
            let retained = result.captures.get_or_insert_with(Vec::new);
            if result.matches.len() == result.matches.capacity() {
                // Grow geometrically; one slot at a time copies the list once per match.
                let grow = result.matches.len().max(BATCH_SIZE);
                result.matches.reserve_exact(grow);
                retained.reserve_exact(grow);
            }
            result.matches.push(SearchMatch { range });
            retained.push(captures);
            return Ok(true);
        }
    }
    if !query.count_beyond_limit {
        result.total_count -= 1;
        return Err(Completeness::ResultLimit);
    }
    Ok(false)
}
pub(super) fn scan(
    snapshot: &DocumentSnapshot,
    query: &SearchQuery,
    job: &SearchJob,
    mut emit: impl FnMut(SearchBatch<'_>),
) -> SearchResults {
    let mut result = SearchResults {
        job: job.id,
        source: snapshot.clone(),
        matches: Vec::new(),
        completeness: Completeness::Complete,
        captures: Some(Vec::new()),
        capture_names: Vec::new(),
        total_count: 0,
        count_complete: false,
    };
    let mut emitted = 0;
    // Retained results filled the budget; later matches are only counted.
    let mut limited = false;
    let status = (|| {
        if job.is_cancelled() {
            return Err(Completeness::Cancelled);
        }
        if query.pattern.len() > MAX_PATTERN_BYTES {
            return Err(Completeness::InvalidQuery);
        }
        let selection = query
            .selection
            .clone()
            .unwrap_or(TextOffset(0)..TextOffset(snapshot.len()));
        if selection.start > selection.end
            || selection.end.0 > snapshot.len()
            || !snapshot.is_boundary(selection.start)
            || !snapshot.is_boundary(selection.end)
        {
            return Err(Completeness::InvalidQuery);
        }
        if snapshot.len() > SUBJECT_LIMIT && streamable(query)? {
            result.capture_names = capture_names(query)?;
            let mut used = result
                .capture_names
                .iter()
                .map(|(name, _)| name.len() + std::mem::size_of::<(String, usize)>())
                .sum::<usize>();
            return scan_stream(
                snapshot.len(),
                query,
                job,
                |start, size| {
                    let mut end = start.saturating_add(size).min(snapshot.len());
                    while end > start && !snapshot.is_boundary(TextOffset(end)) {
                        end -= 1;
                    }
                    snapshot
                        .read(TextOffset(start)..TextOffset(end), size)
                        .map_err(|_| Completeness::Unsupported)
                },
                |captures| {
                    let range = captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
                    if range.end > selection.end {
                        return Ok(());
                    }
                    if record(&mut result, captures, &mut used, &mut limited, query)?
                        && result.matches.len() - emitted == BATCH_SIZE
                    {
                        emit(SearchBatch {
                            job: job.id,
                            revision: snapshot.revision,
                            source: snapshot,
                            matches: &result.matches[emitted..],
                        });
                        emitted = result.matches.len();
                    }
                    Ok(())
                },
            );
        }
        if snapshot.len() > CONTEXT_LIMIT {
            return Err(Completeness::UnsupportedStreaming);
        }
        let mut engine = Engine::new(query)?;
        result.capture_names = engine.names()?;
        let mut text = String::with_capacity(snapshot.len());
        for chunk in snapshot
            .chunks(TextOffset(0)..TextOffset(snapshot.len()))
            .map_err(|_| Completeness::Unsupported)?
        {
            if job.is_cancelled() {
                return Err(Completeness::Cancelled);
            }
            text.push_str(chunk);
        }
        let mut state = Interrupt {
            job,
            deadline: Instant::now(),
        };
        let mut start = selection.start.0;
        let mut after_empty = false;
        let mut used = result
            .capture_names
            .iter()
            .map(|(name, _)| name.len() + std::mem::size_of::<(String, usize)>())
            .sum::<usize>();
        if used > query.results_ram_bytes.min(MAX_RESULT_BYTES) {
            return Err(Completeness::ResultLimit);
        }
        while start <= selection.end.0 {
            if job.is_cancelled() {
                return Err(Completeness::Cancelled);
            }
            state.deadline = attempt_deadline(text.len() - start);
            let Some(captures) = engine.run(&text, start, next_options(after_empty), &mut state)? else {
                if !after_empty {
                    break;
                }
                after_empty = false;
                let Some(advanced) = engine.advance(&text, start) else {
                    break;
                };
                start = advanced;
                continue;
            };
            let range = captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
            if range.end > selection.end {
                break;
            }
            if query.whole_word
                && !word::boundaries(snapshot, range.start.0, range.end.0).ok_or(Completeness::Unsupported)?
            {
                // A rejected candidate can hide a whole word starting inside it (SRC-16).
                let Some(c) = text[range.start.0..].chars().next() else {
                    break;
                };
                start = range.start.0 + c.len_utf8();
                after_empty = false;
                continue;
            }
            if record(&mut result, captures, &mut used, &mut limited, query)?
                && result.matches.len() - emitted == BATCH_SIZE
            {
                emit(SearchBatch {
                    job: job.id,
                    revision: snapshot.revision,
                    source: snapshot,
                    matches: &result.matches[emitted..],
                });
                emitted = result.matches.len();
            }
            start = range.end.0;
            after_empty = range.is_empty();
        }
        Ok(())
    })();
    if emitted < result.matches.len() {
        emit(SearchBatch {
            job: job.id,
            revision: snapshot.revision,
            source: snapshot,
            matches: &result.matches[emitted..],
        });
    }
    // Retained results account for capacity; drop the geometric-growth slack.
    result.matches.shrink_to_fit();
    if let Some(captures) = result.captures.as_mut() {
        captures.shrink_to_fit();
    }
    let cancelled = job.is_cancelled();
    result.count_complete = status.is_ok() && !cancelled;
    result.completeness = match status {
        _ if cancelled => Completeness::Cancelled,
        Err(error) => error,
        Ok(()) if limited => Completeness::ResultLimit,
        Ok(()) => Completeness::Complete,
    };
    result
}

/// Parse a regex replacement once: $0, $1, ${1}, \1, ${name}, $+{name}, $$, and
/// \n/\r/\t/\\. Unknown syntax is rejected so unsupported replacement dialects cannot
/// alter text.
pub(super) fn parse_template(template: &str) -> Result<ReplacementTemplate, ReplaceError> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' && c != '\\' {
            text.push(c);
            continue;
        }
        let next = chars.next().ok_or(ReplaceError::InvalidReplacement)?;
        let piece = if c == '$' && (next == '{' || next == '+') {
            if next == '+' && chars.next() != Some('{') {
                return Err(ReplaceError::InvalidReplacement);
            }
            let mut name = String::new();
            loop {
                match chars.next() {
                    Some('}') => break,
                    Some(c) if name.len() < MAX_PATTERN_BYTES => name.push(c),
                    _ => return Err(ReplaceError::InvalidReplacement),
                }
            }
            if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                TemplatePiece::Group(name.parse::<usize>().map_err(|_| ReplaceError::InvalidReplacement)?)
            } else {
                TemplatePiece::Named(name)
            }
        } else if next.is_ascii_digit() {
            let mut number = next as usize - '0' as usize;
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                chars.next();
                number = number
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(d as usize - '0' as usize))
                    .ok_or(ReplaceError::InvalidReplacement)?;
            }
            TemplatePiece::Group(number)
        } else {
            text.push(match (c, next) {
                ('$', '$') => '$',
                ('\\', '\\') => '\\',
                ('\\', 'n') => '\n',
                ('\\', 'r') => '\r',
                ('\\', 't') => '\t',
                _ => return Err(ReplaceError::InvalidReplacement),
            });
            continue;
        };
        if !text.is_empty() {
            pieces.push(TemplatePiece::Text(std::mem::take(&mut text)));
        }
        pieces.push(piece);
    }
    if !text.is_empty() {
        pieces.push(TemplatePiece::Text(text));
    }
    Ok(ReplacementTemplate { pieces })
}
pub(super) fn expand(
    template: &ReplacementTemplate,
    captures: &[Option<Range<TextOffset>>],
    names: &[(String, usize)],
    snapshot: &DocumentSnapshot,
    limit: usize,
) -> Result<String, ReplaceError> {
    expand_ranges(template, captures, names, limit, |range, size| {
        snapshot.read(range, size).map_err(|_| ReplaceError::Stale)
    })
}
/// Expand only referenced capture ranges through the caller's bounded source reader.
/// Global offsets are preserved; no full document or full match is materialized.
pub(super) fn expand_ranges(
    template: &ReplacementTemplate,
    captures: &[Option<Range<TextOffset>>],
    names: &[(String, usize)],
    limit: usize,
    mut read: impl FnMut(Range<TextOffset>, usize) -> Result<String, ReplaceError>,
) -> Result<String, ReplaceError> {
    let mut output = String::new();
    for piece in &template.pieces {
        let index = match piece {
            TemplatePiece::Text(text) => {
                if text.len() > limit.saturating_sub(output.len()) {
                    return Err(ReplaceError::StagingLimit);
                }
                output.push_str(text);
                continue;
            }
            TemplatePiece::Group(index) => *index,
            // A duplicate name refers to the group that participated in this match.
            TemplatePiece::Named(name) => names
                .iter()
                .find(|(candidate, index)| candidate == name && captures.get(*index).is_some_and(Option::is_some))
                .or_else(|| names.iter().find(|(candidate, _)| candidate == name))
                .map(|(_, index)| *index)
                .ok_or(ReplaceError::InvalidReplacement)?,
        };
        if let Some(range) = captures.get(index).ok_or(ReplaceError::InvalidReplacement)? {
            let size = range.end.0 - range.start.0;
            if size > limit.saturating_sub(output.len()) {
                return Err(ReplaceError::StagingLimit);
            }
            let text = read(range.clone(), size)?;
            if text.len() != size {
                return Err(ReplaceError::Stale);
            }
            output.push_str(&text);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn range_expansion_reads_only_referenced_global_capture_and_checks_budget_first() {
        let names = capture_names(&query("(?<word>abc)")).unwrap();
        let at = CONTEXT_LIMIT + 17;
        let captures = vec![
            Some(TextOffset(at)..TextOffset(at + 3)),
            Some(TextOffset(at)..TextOffset(at + 3)),
        ];
        let mut reads = 0;
        let output = expand_ranges(&template("${word}/$1"), &captures, &names, 7, |range, size| {
            assert_eq!(range, TextOffset(at)..TextOffset(at + 3));
            assert_eq!(size, 3);
            reads += 1;
            Ok("abc".into())
        })
        .unwrap();
        assert_eq!(output, "abc/abc");
        assert_eq!(reads, 2);
        assert!(matches!(
            expand_ranges(&template("$1"), &captures, &names, 2, |_, _| panic!(
                "over-budget capture must not read"
            )),
            Err(ReplaceError::StagingLimit)
        ));
    }
    #[test]
    fn hard_partial_multiline_crosses_windows_and_preserves_capture_offsets() {
        let text = format!(
            "{}BEGIN\n{}\nEND tail BEGIN\nlast\nEND",
            "x".repeat(STREAM_WINDOW - 3),
            "α\n".repeat(STREAM_WINDOW / 3)
        );
        let mut query = SearchQuery::literal("(?s)BEGIN\\n(.*?)\\nEND");
        query.mode = SearchMode::Regex;
        let document = Document::from_utf8(&text, Budget::new(text.len() * 4), Budget::new(0)).unwrap();
        let job = SearchJob::default();
        let expected = scan(&document.snapshot(), &query, &job, |_| {});
        assert_eq!(expected.completeness(), Completeness::Complete);
        let mut actual = Vec::new();
        let mut reads = 0;
        scan_stream(
            text.len(),
            &query,
            &job,
            |start, size| {
                assert!(size <= STREAM_WINDOW);
                reads += 1;
                let mut end = (start + size).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Ok(text[start..end].into())
            },
            |captures| {
                actual.push(captures);
                Ok(())
            },
        )
        .unwrap();
        assert!(reads >= 3);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual, expected.captures.unwrap());
        assert_eq!(actual[0][0].as_ref().unwrap().start, TextOffset(STREAM_WINDOW - 3));
    }
    #[test]
    fn partial_candidate_cap_is_explicit_and_cancellation_is_not_complete() {
        let mut query = SearchQuery::literal("(?s)BEGIN.*END");
        query.mode = SearchMode::Regex;
        let mut first = true;
        let status = scan_stream(
            PARTIAL_CONTEXT + STREAM_WINDOW,
            &query,
            &SearchJob::default(),
            |_, size| {
                let mut part = "x".repeat(size);
                if first {
                    part.replace_range(..5, "BEGIN");
                    first = false;
                }
                Ok(part)
            },
            |_| panic!("unterminated candidate cannot emit"),
        );
        assert!(matches!(
            status,
            Err(Completeness::UnsupportedStreaming | Completeness::RegexLimit(_))
        ));
        let job = SearchJob::default();
        job.cancel();
        assert_eq!(
            scan_stream(1, &query, &job, |_, _| panic!("cancelled read"), |_| Ok(())),
            Err(Completeness::Cancelled)
        );
        query.pattern = "(?<=prefix)match".into();
        assert_eq!(streamable(&query), Ok(false));
    }
    fn document(text: &str) -> Document {
        Document::from_utf8(text, Budget::new(128 * 1024 * 1024), Budget::new(128 * 1024 * 1024)).unwrap()
    }
    fn query(pattern: &str) -> SearchQuery {
        let mut q = SearchQuery::literal(pattern);
        q.mode = SearchMode::Regex;
        q
    }
    fn template(value: &str) -> ReplacementTemplate {
        ReplacementTemplate::decode(value, SearchMode::Regex).unwrap()
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn context_catalog_and_utf8_empty_progress() {
        for (text, pattern, expected) in [
            ("xab ab", r"(?<=x)(ab)\b", vec![1..3]),
            ("ab ab", r"\A(ab) \1\z", vec![0..5]),
            ("a\nb", r"(?m)^b$", vec![2..3]),
            ("a\nb", r"a\nb\z", vec![0..3]),
            ("éx", r"(?=.)|\z", vec![0..0, 2..2, 3..3]),
            ("ab", r"a.*\z|a", vec![0..2]),
            ("aa x", r"\Ga", vec![0..1, 1..2]),
        ] {
            let snapshot = document(text).snapshot();
            let result = super::scan(&snapshot, &query(pattern), &SearchJob::default(), |_| {});
            assert_eq!(result.completeness(), Completeness::Complete, "{pattern}");
            assert_eq!(
                result
                    .matches()
                    .iter()
                    .map(|m| m.range.start.0..m.range.end.0)
                    .collect::<Vec<_>>(),
                expected,
                "{pattern}"
            );
        }
        let snapshot = document("xab").snapshot();
        let mut q = query(r"(?<=x)ab");
        q.selection = Some(TextOffset(1)..TextOffset(3));
        assert_eq!(super::scan(&snapshot, &q, &SearchJob::default(), |_| {}).count(), 1);
    }
    fn ranges(text: &str, q: &SearchQuery) -> Vec<Range<usize>> {
        let result = super::scan(&document(text).snapshot(), q, &SearchJob::default(), |_| {});
        assert_eq!(result.completeness(), Completeness::Complete, "{:?}", q.pattern);
        result
            .matches()
            .iter()
            .map(|m| m.range.start.0..m.range.end.0)
            .collect()
    }
    fn replace_all(text: &str, pattern: &str, replacement: &str) -> String {
        let mut doc = document(text);
        let snapshot = doc.snapshot();
        let result = super::scan(&snapshot, &query(pattern), &SearchJob::default(), |_| {});
        doc.apply(result.prepare_replace(&snapshot, &template(replacement), 4096).unwrap())
            .unwrap();
        let after = doc.snapshot();
        after.read(TextOffset(0)..TextOffset(after.len()), 4096).unwrap()
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn line_anchors_dot_and_bsr_follow_crlf_lf_and_cr_breaks() {
        for eol in ["\r\n", "\n", "\r"] {
            let text = ["one", "two", "three"].join(eol);
            let n = eol.len();
            let starts = [0, 3 + n, 6 + 2 * n];
            let ends = [3, 6 + n, 11 + 2 * n];
            assert_eq!(ranges(&text, &query("^.")), starts.map(|s| s..s + 1), "{eol:?}");
            assert_eq!(ranges(&text, &query("$")), ends.map(|e| e..e), "{eol:?}");
            assert_eq!(
                ranges(&text, &query("^.*$")),
                [0, 1, 2].map(|i| starts[i]..ends[i]),
                "{eol:?}"
            );
            assert_eq!(ranges(&text, &query(r"\R")), [3..3 + n, 6 + n..6 + 2 * n], "{eol:?}");
            let dots = ranges(&text, &query("."));
            assert_eq!(dots.len(), 11, "{eol:?}");
            assert!(dots.iter().all(|r| !text[r.clone()].contains(['\r', '\n'])), "{eol:?}");
            assert_eq!(ranges(&text, &query(r"(?-m)^.")), [0..1], "{eol:?}");
        }
        // Mixed CRLF, LF and CR lines.
        let mixed = "one\r\ntwo\nthree\rfour";
        assert_eq!(ranges(mixed, &query("^.")), [0..1, 5..6, 9..10, 15..16]);
        assert_eq!(ranges(mixed, &query("$")), [3..3, 8..8, 14..14, 19..19]);
        assert_eq!(ranges(mixed, &query("^.*$")), [0..3, 5..8, 9..14, 15..19]);
        assert_eq!(ranges(mixed, &query(r"\R")), [3..5, 8..9, 14..15]);
        assert_eq!(ranges("a end\r\nb end\r\n", &query("(?m)end$")), [2..5, 9..12]);
        // `$` never splits a CRLF, so trailing whitespace stops before the CR.
        assert_eq!(ranges("a \r\nb\t\nc  \rd", &query(r"\s+$")), [1..2, 5..6, 8..10]);
        assert_eq!(ranges("a\r\n\r\nb", &query("^$")), [3..3]);
    }
    #[test]
    fn regex_replace_all_keeps_every_line_break() {
        let crlf = "alpha end\r\nbeta end\r\ngamma end\r\n";
        assert_eq!(replace_all(crlf, r"[ \t]*end$", ""), "alpha\r\nbeta\r\ngamma\r\n");
        assert_eq!(replace_all(crlf, r"(?m) end.*$", ""), "alpha\r\nbeta\r\ngamma\r\n");
        assert_eq!(
            replace_all(crlf, "^", "> "),
            "> alpha end\r\n> beta end\r\n> gamma end\r\n"
        );
        assert_eq!(
            replace_all("alpha  \r\nbeta\t\r\ngamma", r"\s+$", ""),
            "alpha\r\nbeta\r\ngamma"
        );
        assert_eq!(
            replace_all("a end \r\nb end\nc end\rd", r"[ \t]*end\s*$", ""),
            "a\r\nb\nc\rd"
        );
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn dot_matches_newline_option_maps_to_dotall() {
        let text = "BEGIN\r\nbody\nEND";
        let mut q = query("BEGIN.*END");
        assert!(ranges(text, &q).is_empty());
        q.dot_matches_newline = true;
        assert_eq!(ranges(text, &q), [0..text.len()]);
        q.pattern = "(?-s)BEGIN.*END".into();
        assert!(ranges(text, &q).is_empty());
    }
    #[test]
    fn captures_expand_before_atomic_transaction_and_obey_budget() {
        let mut doc = document("ab12 ab34");
        let snapshot = doc.snapshot();
        let result = super::scan(&snapshot, &query(r"(ab)(\d+)"), &SearchJob::default(), |_| {});
        let tx = result
            .prepare_replace(&snapshot, &template(r"$2-${1}-\1-$$"), 4096)
            .unwrap();
        doc.apply(tx).unwrap();
        assert_eq!(
            doc.snapshot()
                .read(TextOffset(0)..TextOffset(doc.snapshot().len()), 4096)
                .unwrap(),
            "12-ab-ab-$ 34-ab-ab-$"
        );
        assert!(matches!(
            result.prepare_replace(&snapshot, &template("$9"), 4096),
            Err(ReplaceError::InvalidReplacement)
        ));
        assert!(matches!(
            result.prepare_replace(&snapshot, &template("$0$0"), 2),
            Err(ReplaceError::StagingLimit)
        ));
        let named = super::scan(
            &snapshot,
            &query(r"(?<word>ab)(?<number>\d+)"),
            &SearchJob::default(),
            |_| {},
        );
        let tx = named
            .prepare_replace(&snapshot, &template("${number}:$+{word}"), 4096)
            .unwrap();
        assert_eq!(tx.edits[0].insert, "12:ab");
        assert!(matches!(
            named.prepare_replace(&snapshot, &template("${missing}"), 4096),
            Err(ReplaceError::InvalidReplacement)
        ));
        for invalid in ["$", r"\q", "${1", "$+x"] {
            assert_eq!(
                ReplacementTemplate::decode(invalid, SearchMode::Regex),
                Err(ReplaceError::InvalidReplacement),
                "{invalid}"
            );
        }
    }
    #[test]
    fn unsupported_subject_limits_and_invalid_patterns_never_replace() {
        let snapshot = document(&"x".repeat(CONTEXT_LIMIT + 1)).snapshot();
        let result = super::scan(&snapshot, &query(r"\Ax"), &SearchJob::default(), |_| {});
        assert_eq!(result.completeness(), Completeness::UnsupportedStreaming);
        assert!(matches!(
            result.prepare_replace(&snapshot, &template("y"), 4096),
            Err(ReplaceError::Incomplete)
        ));
        let snapshot = document("aaaaa").snapshot();
        assert_eq!(
            super::scan(&snapshot, &query("["), &SearchJob::default(), |_| {}).completeness(),
            Completeness::InvalidQuery
        );
        let mut q = query("a");
        q.results_ram_bytes = 1;
        assert_eq!(
            super::scan(&snapshot, &q, &SearchJob::default(), |_| {}).completeness(),
            Completeness::ResultLimit
        );
        let job = SearchJob::default();
        job.cancel();
        assert_eq!(
            super::scan(&snapshot, &query("a"), &job, |_| {}).completeness(),
            Completeness::Cancelled
        );
    }
    #[test]
    fn multiline_regex_crosses_sixteen_mib_with_full_anchor_context() {
        let mut text = "x".repeat(SUBJECT_LIMIT - 2);
        text.push_str("AB\nCD");
        text.push_str(&"x".repeat(4 * 1024 * 1024));
        let snapshot = document(&text).snapshot();
        let result = super::scan(&snapshot, &query("AB\nCD"), &SearchJob::default(), |_| {});
        assert_eq!(result.completeness(), Completeness::Complete);
        assert_eq!(
            result.matches()[0].range,
            TextOffset(SUBJECT_LIMIT - 2)..TextOffset(SUBJECT_LIMIT + 3)
        );
    }
    #[test]
    fn engine_callout_stops_cancelled_and_expired_matching() {
        let q = query("(a+)+$");
        let mut engine = Engine::new(&q).unwrap();
        let job = SearchJob::default();
        let mut state = Interrupt {
            job: &job,
            deadline: Instant::now() - Duration::from_secs(1),
        };
        assert_eq!(
            engine.run("aaaa!", 0, 0, &mut state),
            Err(Completeness::RegexLimit(RegexLimitKind::Time))
        );
        job.cancel();
        state.deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(engine.run("aaaa!", 0, 0, &mut state), Err(Completeness::Cancelled));
    }
    #[test]
    fn capped_batches_emit_retained_tail_and_disable_replacement() {
        let snapshot = document(&"a".repeat(400)).snapshot();
        let mut q = query("a");
        let record = std::mem::size_of::<SearchMatch>()
            + std::mem::size_of::<Captures>()
            + std::mem::size_of::<Option<Range<TextOffset>>>();
        q.results_ram_bytes = record * 150;
        let mut sizes = Vec::new();
        let result = super::scan(&snapshot, &q, &SearchJob::default(), |batch| {
            sizes.push(batch.matches.len())
        });
        assert_eq!(sizes, [128, 22]);
        assert_eq!(result.count(), 150);
        assert!(!result.count_complete());
        assert_eq!(result.completeness(), Completeness::ResultLimit);
        assert!(matches!(
            result.prepare_replace(&snapshot, &template("b"), 4096),
            Err(ReplaceError::Incomplete)
        ));
        // Asked for a full count, the scan keeps counting without retaining (SRC-21).
        q.count_beyond_limit = true;
        let mut sizes = Vec::new();
        let result = super::scan(&snapshot, &q, &SearchJob::default(), |batch| {
            sizes.push(batch.matches.len())
        });
        assert_eq!(sizes, [128, 22]);
        assert_eq!(result.matches().len(), 150);
        assert_eq!(result.count(), 400);
        assert!(result.count_complete());
        assert_eq!(result.completeness(), Completeness::ResultLimit);
        assert!(matches!(
            result.prepare_replace(&snapshot, &template("b"), 4096),
            Err(ReplaceError::Incomplete)
        ));
    }
    #[test]
    fn hundred_thousand_matches_in_a_megabyte_complete() {
        // Without PCRE2_NO_UTF_CHECK every call revalidated the rest of the subject, and
        // one deadline covered the whole scan, so this stopped at the regex limit (SRC-03).
        let text = "1234567 ab\n".repeat(100_000);
        assert!(text.len() > 1024 * 1024);
        let result = super::scan(
            &document(&text).snapshot(),
            &query(r"\d+"),
            &SearchJob::default(),
            |_| {},
        );
        assert_eq!(result.completeness(), Completeness::Complete);
        assert_eq!(result.count(), 100_000);
        assert_eq!(
            result.matches()[99_999].range,
            TextOffset(11 * 99_999)..TextOffset(11 * 99_999 + 7)
        );
    }
    #[test]
    fn long_group_repetition_is_not_cut_off_and_limits_are_named() {
        // A 2,000+ character JSON string repeats one group per character (SRC-06).
        let body = "abcdefghi\\\"".repeat(200);
        let text = format!(r#"{{"key": "{body}", "n": 1}}"#);
        let length = body.len();
        assert_eq!(
            ranges(&text, &query(r#""(?:[^"\\]|\\.)*""#)),
            [1..6, 8..length + 10, length + 12..length + 15]
        );
        let runaway = format!("{}!", "a".repeat(40));
        assert_eq!(
            super::scan(
                &document(&runaway).snapshot(),
                &query("(a+)+$"),
                &SearchJob::default(),
                |_| {}
            )
            .completeness(),
            Completeness::RegexLimit(RegexLimitKind::Backtracking)
        );
    }
    #[test]
    fn empty_matches_iterate_like_pcre2demo() {
        // After an empty match, a non-empty match at the same position is still found,
        // and advancing never starts between the CR and LF of a CRLF (SRC-16).
        assert_eq!(ranges("a", &query("|a")), [0..0, 0..1, 1..1]);
        assert_eq!(replace_all("a", "|a", "X"), "XXX");
        assert_eq!(ranges("a\r\nb", &query("x*")), [0..0, 1..1, 3..3, 4..4]);
        let snapshot = document("a").snapshot();
        let result = super::scan(&snapshot, &query("|a"), &SearchJob::default(), |_| {});
        let one = result
            .prepare_replace_scoped(
                &snapshot,
                &template("<$0>"),
                4096,
                ReplaceScope::One(TextOffset(0)..TextOffset(1)),
                &SearchJob::default(),
            )
            .unwrap();
        assert_eq!(one.edits[0].insert, "<a>");
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn whole_word_regex_backtracks_into_longer_alternatives() {
        let mut q = query("x|xy");
        q.whole_word = true;
        assert_eq!(ranges("xy", &q), [0..2]);
        assert_eq!(ranges("x xy xyz", &q), [0..1, 2..4]);
        q.pattern = "foo".into();
        assert_eq!(ranges("foo food _foo foo\u{301} foo-foo", &q), [0..3, 20..23, 24..27]);
        // Every top-level alternative, not just the last, reaches the final callout.
        q.pattern = "a|ab|abc".into();
        assert_eq!(ranges("abc ab a abcd", &q), [0..3, 4..6, 7..8]);
        q.pattern = "a|ab|abc|abcd|b".into();
        assert_eq!(ranges("abcd b", &q), [0..4, 5..6]);
        // Start options stay first, a trailing \Q and a trailing (?x) comment stay closed.
        for pattern in ["(*UCP)(*NO_JIT)x|xy", r"x|\Qxy", "(?x) x | xy  # either"] {
            q.pattern = pattern.into();
            assert_eq!(ranges("xy", &q), [0..2], "{pattern}");
        }
        // Repetitions give back characters too; auto-possession would keep `ab cd`.
        q.pattern = "[a-z ]+".into();
        assert_eq!(ranges("ab cd9", &q), [0..2]);
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn whole_word_regex_keeps_the_caller_check_for_recursion_keep_and_verbs() {
        let mut q = query(r"\((?:[^()]++|(?R))*\)");
        q.whole_word = true;
        // The inner (b) recursion ends before `c`; only the outermost end is a candidate end.
        assert_eq!(ranges("(a(b)c)", &q), [0..7]);
        // The match starts at \K, after the comma, not at the attempt start after `a`.
        q.pattern = r",\Kfoo".into();
        assert_eq!(ranges("a,foo", &q), [2..5]);
        // A rejection after (*COMMIT) would end the whole engine call at `foox`.
        q.pattern = "foo(*COMMIT)".into();
        assert_eq!(ranges("foox foo", &q), [5..8]);
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn whole_word_regex_rejections_stay_linear_on_a_long_word_run() {
        // Rejecting every shorter `a+` at the first start would exceed the match limit,
        // and retrying each later start inside the run would scan it again each time.
        let run = 1_000_000;
        let text = format!("{}b a", "a".repeat(run));
        let mut q = query("a+");
        q.whole_word = true;
        assert_eq!(ranges(&text, &q), [run + 2..run + 3]);
    }
    #[test]
    fn streaming_is_classified_from_the_compiled_pattern() {
        for (pattern, expected) in [
            ("[^,]+", true),
            (r"(?<cell>[^,\r\n]+),", true),
            (r"a(?=b)", true),
            (r"(?s)BEGIN.*?END", true),
            ("^a", false),
            ("a$", false),
            (r"(?<=,)a", false),
            (r"\ba", false),
            (r"\Aa", false),
            (r"\Ga", false),
            ("(*COMMIT)a", false),
        ] {
            assert_eq!(streamable(&query(pattern)), Ok(expected), "{pattern}");
        }
        let mut whole = query("a");
        whole.whole_word = true;
        assert_eq!(streamable(&whole), Ok(false));
        assert_eq!(streamable(&query("[")), Err(Completeness::InvalidQuery));
        // `[^,]+` streams across windows with the same captures as the exact subject path.
        let text = "alpha,beta,γάμμα\r\n".repeat(40_000);
        let q = query(r"(?<cell>[^,]+)");
        let job = SearchJob::default();
        let expected = super::scan(&document(&text).snapshot(), &q, &job, |_| {});
        assert_eq!(expected.completeness(), Completeness::Complete);
        let mut actual = Vec::new();
        scan_stream(
            text.len(),
            &q,
            &job,
            |start, size| {
                let mut end = (start + size).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Ok(text[start..end].into())
            },
            |captures| {
                actual.push(captures);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(actual, expected.captures.unwrap());
    }
    #[test]
    #[allow(clippy::single_range_in_vec_init)] // Expected match ranges, not range contents.
    fn caseless_regex_folds_single_characters_only() {
        // Documented difference from literal search's full folding (SRC-17).
        let text = "Straße STRASSE ςΣσ";
        let mut q = query("strasse");
        q.case = Case::Folded;
        assert_eq!(ranges(text, &q), [8..15]);
        q.pattern = "σ".into();
        assert_eq!(ranges(text, &q), [16..18, 18..20, 20..22]);
    }
    #[test]
    fn regex_template_is_parsed_once() {
        // `\\n` is one escaped backslash and `n`; nothing decodes it again (SRC-21).
        assert_eq!(template(r"\\n"), ReplacementTemplate::plain(r"\n"));
        assert_eq!(replace_all("a", "a", r"\\n"), r"\n");
        assert_eq!(replace_all("a", "a", r"\n"), "\n");
    }
}
