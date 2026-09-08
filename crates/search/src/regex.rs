// SPDX-License-Identifier: MPL-2.0
//! PCRE2 hard-partial streaming with an exact bounded fallback for contextual syntax.
use super::*;
use pcre2_sys::*;
use std::{
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};

pub const SUBJECT_LIMIT: usize = 16 * 1024 * 1024;
/// Complete context budget; no source prefix is discarded and all subject anchors stay exact.
pub const CONTEXT_LIMIT: usize = 64 * 1024 * 1024;
pub(super) type Captures = Vec<Option<Range<TextOffset>>>;
pub(super) fn capture_names(query:&SearchQuery)->Result<Vec<(String,usize)>,Completeness>{
    if query.pattern.len()>MAX_PATTERN_BYTES{return Err(Completeness::InvalidQuery);}
    Engine::new(query)?.names()
}
pub(super) const STREAM_WINDOW:usize=256*1024;
const PARTIAL_CONTEXT:usize=8*1024*1024;
/// These constructs can inspect discarded subject context or change start semantics.
/// Keep them on the exact subject path rather than guessing an overlap distance.
pub(super) fn streamable(query:&SearchQuery)->bool {
    if query.whole_word{return false;}
    let mut pattern=query.pattern.as_str();
    for flag in ["(?s)","(?m)","(?i)","(?is)","(?si)"]{pattern=pattern.strip_prefix(flag).unwrap_or(pattern);}
    !pattern.contains("(?")&&!pattern.contains("(*")&&!pattern.contains('^')&&!pattern.contains('$')&&!pattern.contains("[:<:]")&&!pattern.contains("[:>:]")
        &&!pattern.as_bytes().windows(2).any(|pair|pair[0]==b'\\'&&(pair[1].is_ascii_digit()||b"AbBGKkzgZQEX".contains(&pair[1])))
}
enum PartialMatch { Match(Captures), Partial(usize), None }

// pcre2-sys omits callout bindings. The block is opaque: the callback never dereferences it.
// Signature is from the bundled pcre2.h; PCRE2 calls synchronously on the matching thread.
unsafe extern "C" {
    fn pcre2_set_callout_8(
        context: *mut pcre2_match_context_8,
        callback: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32>,
        data: *mut c_void,
    ) -> i32;
}
struct Interrupt<'a> {
    job: &'a SearchJob,
    deadline: Instant,
}
unsafe extern "C" fn interrupt(_: *mut c_void, data: *mut c_void) -> i32 {
    // SAFETY: Engine::run keeps this stack value alive for the synchronous match call.
    let state = unsafe { &*(data as *const Interrupt<'_>) };
    if state.job.is_cancelled() || Instant::now() >= state.deadline {
        PCRE2_ERROR_CALLOUT
    } else {
        0
    }
}
struct Engine {
    code: *mut pcre2_code_8,
    data: *mut pcre2_match_data_8,
    context: *mut pcre2_match_context_8,
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
    fn partial(&mut self,text:&str,start:usize,state:&mut Interrupt<'_>,eof:bool)->Result<PartialMatch,Completeness>{
        // SAFETY: allocations are owned by Engine; the UTF-8 subject and synchronous
        // callout state remain live throughout the call and ovector copy.
        unsafe{
            pcre2_set_callout_8(self.context,Some(interrupt),(state as *mut Interrupt<'_>).cast());
            let code=pcre2_match_8(self.code,text.as_ptr(),text.len(),start,if eof{0}else{PCRE2_PARTIAL_HARD},self.data,self.context);
            if state.job.is_cancelled(){return Err(Completeness::Cancelled);}
            if Instant::now()>=state.deadline{return Err(Completeness::RegexLimit);}
            if code==PCRE2_ERROR_NOMATCH{return Ok(PartialMatch::None);}
            let vector=pcre2_get_ovector_pointer_8(self.data);
            if code==PCRE2_ERROR_PARTIAL{
                let at=*vector;
                if at>text.len()||!text.is_char_boundary(at){return Err(Completeness::UnsupportedStreaming);}
                return Ok(PartialMatch::Partial(at));
            }
            if code<0{return Err(Completeness::RegexLimit);}
            let values=std::slice::from_raw_parts(vector,pcre2_get_ovector_count_8(self.data) as usize*2);
            let mut captures=Vec::new();
            for pair in values.as_chunks::<2>().0{
                captures.push(if pair[0]==usize::MAX{None}else{
                    if pair[0]>pair[1]||pair[1]>text.len()||!text.is_char_boundary(pair[0])||!text.is_char_boundary(pair[1]){return Err(Completeness::UnsupportedStreaming);}
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
            pcre2_pattern_info_8(
                self.code,
                PCRE2_INFO_NAMECOUNT,
                (&mut count as *mut u32).cast(),
            );
            pcre2_pattern_info_8(
                self.code,
                PCRE2_INFO_NAMEENTRYSIZE,
                (&mut size as *mut u32).cast(),
            );
            pcre2_pattern_info_8(
                self.code,
                PCRE2_INFO_NAMETABLE,
                (&mut table as *mut *const u8).cast(),
            );
            let mut names = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let entry =
                    std::slice::from_raw_parts(table.add(index * size as usize), size as usize);
                let number = u16::from_be_bytes([entry[0], entry[1]]) as usize;
                let bytes = entry[2..].split(|b| *b == 0).next().unwrap();
                let name = std::str::from_utf8(bytes).map_err(|_| Completeness::InvalidQuery)?;
                names.push((name.into(), number));
            }
            Ok(names)
        }
    }
    fn new(query: &SearchQuery) -> Result<Self, Completeness> {
        // SAFETY: All inputs are valid slices for the call duration. PCRE2 ownership is
        // transferred to Engine immediately; all failure paths free their allocations.
        unsafe {
            let compile = pcre2_compile_context_create_8(ptr::null_mut());
            if compile.is_null() {
                return Err(Completeness::RegexLimit);
            }
            pcre2_set_max_pattern_compiled_length_8(compile, 1024 * 1024);
            pcre2_set_parens_nest_limit_8(compile, 250);
            let mut error = 0;
            let mut offset = 0;
            // Keep PCRE2's literal-prefix/start optimizations: disabling them invokes
            // a callout at every candidate byte and exhausts the deadline on an
            // ordinary 20 MiB search. Optimized subject scans are bounded by the
            // 64 MiB context cap; matching still has automatic callouts and limits,
            // and cancellation is checked before/after each engine invocation.
            let options = PCRE2_UTF
                | PCRE2_UCP
                | PCRE2_AUTO_CALLOUT
                | PCRE2_NEVER_BACKSLASH_C
                | if query.case == Case::Folded {
                    PCRE2_CASELESS
                } else {
                    0
                };
            let code = pcre2_compile_8(
                query.pattern.as_ptr(),
                query.pattern.len(),
                options,
                &mut error,
                &mut offset,
                compile,
            );
            pcre2_compile_context_free_8(compile);
            if code.is_null() {
                return Err(Completeness::InvalidQuery);
            }
            let engine = Self {
                code,
                data: pcre2_match_data_create_from_pattern_8(code, ptr::null_mut()),
                context: pcre2_match_context_create_8(ptr::null_mut()),
            };
            if engine.data.is_null() || engine.context.is_null() {
                return Err(Completeness::RegexLimit);
            }
            pcre2_set_match_limit_8(engine.context, 1_000_000);
            pcre2_set_depth_limit_8(engine.context, 1_000);
            pcre2_set_heap_limit_8(engine.context, 8 * 1024); // KiB
            Ok(engine)
        }
    }
    fn run(
        &mut self,
        text: &str,
        start: usize,
        state: &mut Interrupt<'_>,
    ) -> Result<Option<Captures>, Completeness> {
        // SAFETY: Engine owns live allocations; subject and state outlive this synchronous
        // non-JIT call. The returned ovector belongs to data and is copied before reuse.
        unsafe {
            pcre2_set_callout_8(
                self.context,
                Some(interrupt),
                (state as *mut Interrupt<'_>).cast(),
            );
            let code = pcre2_match_8(
                self.code,
                text.as_ptr(),
                text.len(),
                start,
                0,
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
                return Err(Completeness::RegexLimit);
            }
            let count = pcre2_get_ovector_count_8(self.data) as usize;
            let values =
                std::slice::from_raw_parts(pcre2_get_ovector_pointer_8(self.data), count * 2);
            let mut captures = Vec::with_capacity(count);
            for pair in values.as_chunks::<2>().0 {
                captures.push(if pair[0] == usize::MAX {
                    None
                } else {
                    if pair[0] > pair[1]
                        || !text.is_char_boundary(pair[0])
                        || !text.is_char_boundary(pair[1])
                    {
                        return Err(Completeness::UnsupportedStreaming);
                    }
                    Some(TextOffset(pair[0])..TextOffset(pair[1]))
                });
            }
            Ok(Some(captures))
        }
    }
}
/// Retain PCRE2's earliest hard-partial candidate, not an arbitrary fixed overlap.
/// A candidate exceeding the bounded context fails explicitly; no matches are skipped.
pub(super) fn scan_stream(
    length:usize,query:&SearchQuery,job:&SearchJob,
    mut read:impl FnMut(usize,usize)->Result<String,Completeness>,
    mut emit:impl FnMut(Captures)->Result<(),Completeness>,
)->Result<(),Completeness>{
    if query.pattern.len()>MAX_PATTERN_BYTES{return Err(Completeness::InvalidQuery);}
    if !streamable(query){return Err(Completeness::UnsupportedStreaming);}
    let mut engine=Engine::new(query)?;
    let selection=query.selection.clone().unwrap_or(TextOffset(0)..TextOffset(length));
    if selection.start>selection.end||selection.end.0>length{return Err(Completeness::InvalidQuery);}
    let mut text=String::new();let mut base=selection.start.0;let mut next=base;let mut start=0usize;
    loop{
        if job.is_cancelled(){return Err(Completeness::Cancelled);}
        if next<length{
            let part=read(next,STREAM_WINDOW.min(length-next))?;
            if part.is_empty()||part.len()>length-next{return Err(Completeness::Unsupported);}
            if text.len().saturating_add(part.len())>PARTIAL_CONTEXT{return Err(Completeness::UnsupportedStreaming);}
            next+=part.len();text.push_str(&part);
        }
        let eof=next==length;
        let mut state=Interrupt{job,deadline:Instant::now()+Duration::from_secs(2)};
        let retain=loop{
            if base+start>selection.end.0{return Ok(());}
            match engine.partial(&text,start,&mut state,eof)?{
                PartialMatch::None=>break text.len(),
                PartialMatch::Partial(at)=>break at,
                PartialMatch::Match(mut captures)=>{
                    let range=captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
                    // An empty match at a temporary edge must wait for the next scalar.
                    if range.is_empty()&&range.end.0==text.len()&&!eof{break range.start.0;}
                    for capture in captures.iter_mut().flatten(){capture.start.0+=base;capture.end.0+=base;}
                    emit(captures)?;
                    start=range.end.0;
                    if range.is_empty(){if let Some(c)=text[start..].chars().next(){start+=c.len_utf8();}else{break text.len();}}
                }
            }
        };
        if eof{return Ok(());}
        text.drain(..retain);base+=retain;start=0;
    }
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
        if snapshot.len()>SUBJECT_LIMIT&&streamable(query){
            result.capture_names=Engine::new(query)?.names()?;
            let mut used=result.capture_names.iter().map(|(name,_)|name.len()+std::mem::size_of::<(String,usize)>()).sum::<usize>();
            return scan_stream(snapshot.len(),query,job,|start,size|{
                let mut end=start.saturating_add(size).min(snapshot.len());
                while end>start&&!snapshot.is_boundary(TextOffset(end)){end-=1;}
                snapshot.read(TextOffset(start)..TextOffset(end),size).map_err(|_|Completeness::Unsupported)
            },|captures|{
                let range=captures[0].clone().ok_or(Completeness::UnsupportedStreaming)?;
                if range.end>selection.end{return Ok(());}
                used=used.saturating_add(std::mem::size_of::<SearchMatch>()+std::mem::size_of::<Captures>()+captures.len()*std::mem::size_of::<Option<Range<TextOffset>>>());
                if used>query.results_ram_bytes.min(MAX_RESULT_BYTES){return Err(Completeness::ResultLimit);}
                result.matches.push(SearchMatch{range});result.captures.as_mut().unwrap().push(captures);
                if result.matches.len()-emitted==BATCH_SIZE{
                    emit(SearchBatch{job:job.id,revision:snapshot.revision,source:snapshot,matches:&result.matches[emitted..]});emitted=result.matches.len();
                }
                Ok(())
            });
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
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let mut start = selection.start.0;
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
            let Some(captures) = engine.run(&text, start, &mut state)? else {
                break;
            };
            let range = captures[0]
                .clone()
                .ok_or(Completeness::UnsupportedStreaming)?;
            if range.start > selection.end {
                break;
            }
            if range.end <= selection.end
                && (!query.whole_word
                    || word::boundaries(snapshot, range.start.0, range.end.0)
                        .ok_or(Completeness::Unsupported)?)
            {
                used = used.saturating_add(
                    std::mem::size_of::<SearchMatch>()
                        + std::mem::size_of::<Captures>()
                        + captures.len() * std::mem::size_of::<Option<Range<TextOffset>>>(),
                );
                if used > query.results_ram_bytes.min(MAX_RESULT_BYTES) {
                    return Err(Completeness::ResultLimit);
                }
                result.matches.reserve_exact(1);
                result.captures.as_mut().unwrap().reserve_exact(1);
                result.matches.push(SearchMatch {
                    range: range.clone(),
                });
                result.captures.as_mut().unwrap().push(captures);
                if result.matches.len() - emitted == BATCH_SIZE {
                    emit(SearchBatch {
                        job: job.id,
                        revision: snapshot.revision,
                        source: snapshot,
                        matches: &result.matches[emitted..],
                    });
                    emitted = result.matches.len();
                }
            }
            start = range.end.0;
            if range.is_empty() {
                let Some(c) = text[start..].chars().next() else {
                    break;
                };
                start += c.len_utf8();
            }
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
    result.completeness = if job.is_cancelled() {
        Completeness::Cancelled
    } else {
        status.err().unwrap_or(Completeness::Complete)
    };
    result.total_count = result.matches.len();
    result.count_complete = result.completeness == Completeness::Complete;
    result
}

/// Numeric capture templates: $0, $1, ${1}, \1, $$, and \n/\r/\t/\\.
/// Unknown syntax is rejected so unsupported replacement dialects cannot alter text.
pub(super) fn expand(
    template: &str,
    captures: &[Option<Range<TextOffset>>],
    names: &[(String, usize)],
    snapshot: &DocumentSnapshot,
    limit: usize,
) -> Result<String, ReplaceError> {
    expand_ranges(template,captures,names,limit,|range,size|snapshot.read(range,size).map_err(|_|ReplaceError::Stale))
}
/// Expand only referenced capture ranges through the caller's bounded source reader.
/// Global offsets are preserved; no full document or full match is materialized.
pub(super) fn expand_ranges(
    template:&str,captures:&[Option<Range<TextOffset>>],names:&[(String,usize)],limit:usize,
    mut read:impl FnMut(Range<TextOffset>,usize)->Result<String,ReplaceError>,
)->Result<String,ReplaceError>{
    let mut output = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        let mut literal = None;
        let mut capture = None;
        match c {
            '$' | '\\' => {
                let next = chars.next().ok_or(ReplaceError::InvalidReplacement)?;
                if c == '$' && (next == '{' || next == '+') {
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
                    capture = Some(
                        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                            name.parse::<usize>()
                                .map_err(|_| ReplaceError::InvalidReplacement)?
                        } else {
                            names
                                .iter()
                                .find(|(candidate, index)| {
                                    candidate == &name && captures[*index].is_some()
                                })
                                .or_else(|| names.iter().find(|(candidate, _)| candidate == &name))
                                .map(|(_, index)| *index)
                                .ok_or(ReplaceError::InvalidReplacement)?
                        },
                    );
                } else if next.is_ascii_digit() {
                    let braced = next == '{';
                    let mut number = if braced {
                        0
                    } else {
                        next as usize - '0' as usize
                    };
                    let mut digits = usize::from(!braced);
                    while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                        chars.next();
                        digits += 1;
                        number = number
                            .checked_mul(10)
                            .and_then(|n| n.checked_add(d as usize - '0' as usize))
                            .ok_or(ReplaceError::InvalidReplacement)?;
                    }
                    if digits == 0 || (braced && chars.next() != Some('}')) {
                        return Err(ReplaceError::InvalidReplacement);
                    }
                    capture = Some(number);
                } else {
                    literal = Some(match (c, next) {
                        ('$', '$') => '$',
                        ('\\', '\\') => '\\',
                        ('\\', 'n') => '\n',
                        ('\\', 'r') => '\r',
                        ('\\', 't') => '\t',
                        _ => return Err(ReplaceError::InvalidReplacement),
                    });
                }
            }
            _ => literal = Some(c),
        }
        if let Some(index) = capture
            && let Some(range) = captures
                .get(index)
                .ok_or(ReplaceError::InvalidReplacement)?
        {
            let size = range.end.0 - range.start.0;
            if size > limit.saturating_sub(output.len()) {
                return Err(ReplaceError::StagingLimit);
            }
            let text=read(range.clone(),size)?;
            if text.len()!=size{return Err(ReplaceError::Stale);}
            output.push_str(&text);
        }
        if let Some(c) = literal {
            if c.len_utf8() > limit.saturating_sub(output.len()) {
                return Err(ReplaceError::StagingLimit);
            }
            output.push(c);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn range_expansion_reads_only_referenced_global_capture_and_checks_budget_first(){
        let names=capture_names(&query("(?<word>abc)")).unwrap();
        let at=CONTEXT_LIMIT+17;
        let captures=vec![Some(TextOffset(at)..TextOffset(at+3)),Some(TextOffset(at)..TextOffset(at+3))];
        let mut reads=0;
        let output=expand_ranges("${word}/$1",&captures,&names,7,|range,size|{
            assert_eq!(range,TextOffset(at)..TextOffset(at+3));assert_eq!(size,3);reads+=1;Ok("abc".into())
        }).unwrap();
        assert_eq!(output,"abc/abc");assert_eq!(reads,2);
        assert!(matches!(expand_ranges("$1",&captures,&names,2,|_,_|panic!("over-budget capture must not read")),Err(ReplaceError::StagingLimit)));
    }
    #[test]
    fn hard_partial_multiline_crosses_windows_and_preserves_capture_offsets(){
        let text=format!("{}BEGIN\n{}\nEND tail BEGIN\nlast\nEND","x".repeat(STREAM_WINDOW-3),"α\n".repeat(STREAM_WINDOW/3));
        let mut query=SearchQuery::literal("(?s)BEGIN\\n(.*?)\\nEND");query.mode=SearchMode::Regex;
        let document=Document::from_utf8(&text,Budget::new(text.len()*4),Budget::new(0)).unwrap();
        let job=SearchJob::default();let expected=scan(&document.snapshot(),&query,&job,|_|{});
        assert_eq!(expected.completeness(),Completeness::Complete);
        let mut actual=Vec::new();let mut reads=0;
        scan_stream(text.len(),&query,&job,|start,size|{
            assert!(size<=STREAM_WINDOW);reads+=1;let mut end=(start+size).min(text.len());while !text.is_char_boundary(end){end-=1;}Ok(text[start..end].into())
        },|captures|{actual.push(captures);Ok(())}).unwrap();
        assert!(reads>=3);assert_eq!(actual.len(),2);
        assert_eq!(actual,expected.captures.unwrap());
        assert_eq!(actual[0][0].as_ref().unwrap().start,TextOffset(STREAM_WINDOW-3));
    }
    #[test]
    fn partial_candidate_cap_is_explicit_and_cancellation_is_not_complete(){
        let mut query=SearchQuery::literal("(?s)BEGIN.*END");query.mode=SearchMode::Regex;
        let mut first=true;
        let status=scan_stream(PARTIAL_CONTEXT+STREAM_WINDOW,&query,&SearchJob::default(),|_,size|{let mut part="x".repeat(size);if first{part.replace_range(..5,"BEGIN");first=false;}Ok(part)},|_|panic!("unterminated candidate cannot emit"));
        assert!(matches!(status,Err(Completeness::UnsupportedStreaming|Completeness::RegexLimit)));
        let job=SearchJob::default();job.cancel();
        assert_eq!(scan_stream(1,&query,&job,|_,_|panic!("cancelled read"),|_|Ok(())),Err(Completeness::Cancelled));
        query.pattern="(?<=prefix)match".into();assert!(!streamable(&query));
    }
    fn document(text: &str) -> Document {
        Document::from_utf8(
            text,
            Budget::new(128 * 1024 * 1024),
            Budget::new(128 * 1024 * 1024),
        )
        .unwrap()
    }
    fn query(pattern: &str) -> SearchQuery {
        let mut q = SearchQuery::literal(pattern);
        q.mode = SearchMode::Regex;
        q
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
        assert_eq!(
            super::scan(&snapshot, &q, &SearchJob::default(), |_| {}).count(),
            1
        );
    }
    #[test]
    fn captures_expand_before_atomic_transaction_and_obey_budget() {
        let mut doc = document("ab12 ab34");
        let snapshot = doc.snapshot();
        let result = super::scan(
            &snapshot,
            &query(r"(ab)(\d+)"),
            &SearchJob::default(),
            |_| {},
        );
        let tx = result
            .prepare_replace(&snapshot, r"$2-${1}-\1-$$", 4096)
            .unwrap();
        doc.apply(tx).unwrap();
        assert_eq!(
            doc.snapshot()
                .read(TextOffset(0)..TextOffset(doc.snapshot().len()), 4096)
                .unwrap(),
            "12-ab-ab-$ 34-ab-ab-$"
        );
        assert!(matches!(
            result.prepare_replace(&snapshot, "$9", 4096),
            Err(ReplaceError::InvalidReplacement)
        ));
        assert!(matches!(
            result.prepare_replace(&snapshot, "$0$0", 2),
            Err(ReplaceError::StagingLimit)
        ));
        let named = super::scan(
            &snapshot,
            &query(r"(?<word>ab)(?<number>\d+)"),
            &SearchJob::default(),
            |_| {},
        );
        let tx = named
            .prepare_replace(&snapshot, "${number}:$+{word}", 4096)
            .unwrap();
        assert_eq!(tx.edits[0].insert, "12:ab");
        assert!(matches!(
            named.prepare_replace(&snapshot, "${missing}", 4096),
            Err(ReplaceError::InvalidReplacement)
        ));
    }
    #[test]
    fn unsupported_subject_limits_and_invalid_patterns_never_replace() {
        let snapshot = document(&"x".repeat(CONTEXT_LIMIT + 1)).snapshot();
        let result = super::scan(&snapshot, &query(r"\Ax"), &SearchJob::default(), |_| {});
        assert_eq!(result.completeness(), Completeness::UnsupportedStreaming);
        assert!(matches!(
            result.prepare_replace(&snapshot, "y", 4096),
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
            engine.run("aaaa!", 0, &mut state),
            Err(Completeness::RegexLimit)
        );
        job.cancel();
        state.deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            engine.run("aaaa!", 0, &mut state),
            Err(Completeness::Cancelled)
        );
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
        assert_eq!(result.completeness(), Completeness::ResultLimit);
        assert!(matches!(
            result.prepare_replace(&snapshot, "b", 4096),
            Err(ReplaceError::Incomplete)
        ));
    }
}
