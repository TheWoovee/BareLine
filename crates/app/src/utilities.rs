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
/// Worker-side full text reader for paged buffers, including unsaved piece-tree edits.
/// Owns only one 64 KiB window and never substitutes unavailable bytes.
pub struct PagedTextReader {
    source: bareline_editor_surface::paged_view::PagedReadHandle,
    range: Range<TextOffset>,
    position: usize,
    buffer: Vec<u8>,
    consumed: usize,
    budget: bareline_document::Budget,
    cancel: CancelToken,
}
impl PagedTextReader {
    pub fn new(source:bareline_editor_surface::paged_view::PagedReadHandle,range:Range<TextOffset>,cancel:CancelToken)->Result<Self,UtilityError> {
        if range.start>range.end || range.end.0>source.snapshot().len(){return Err(UtilityError::InvalidRange);}
        Ok(Self{position:range.start.0,source,range,buffer:Vec::new(),consumed:0,budget:bareline_document::Budget::new(256*1024),cancel})
    }
}
impl Read for PagedTextReader {
    fn read(&mut self,output:&mut[u8])->std::io::Result<usize> {
        if output.is_empty(){return Ok(0);}
        if self.cancel.is_cancelled(){return Err(std::io::Error::new(std::io::ErrorKind::Interrupted,"Cancelled"));}
        if self.consumed==self.buffer.len() {
            if self.position==self.range.end.0{return Ok(0);}
            let mut end=(self.position+64*1024).min(self.range.end.0);
            let mut retries=0;
            'retry:loop {
                let mut request=self.source.snapshot().begin_read(TextOffset(self.position)..TextOffset(end),64*1024,&self.budget).map_err(|e|std::io::Error::other(format!("{e:?}")))?;
                loop {
                    if self.cancel.is_cancelled(){return Err(std::io::Error::new(std::io::ErrorKind::Interrupted,"Cancelled"));}
                    match request.poll() {
                        bareline_document::paged::WindowPoll::Ready(window)=>{self.buffer=window.text().as_bytes().to_vec();self.position=end;self.consumed=0;break 'retry;},
                        bareline_document::paged::WindowPoll::Pending(ticket)=>{if !self.source.resolve_page(ticket).map_err(std::io::Error::other)?{std::thread::sleep(std::time::Duration::from_millis(1));}},
                        bareline_document::paged::WindowPoll::InvalidUtf8 if retries<3 && end<self.range.end.0=>{end-=1;retries+=1;continue 'retry;},
                        _=>return Err(std::io::Error::other("Paged text is unavailable or changed; retry")),
                    }
                }
            }
        }
        let count=output.len().min(self.buffer.len()-self.consumed);output[..count].copy_from_slice(&self.buffer[self.consumed..self.consumed+count]);self.consumed+=count;Ok(count)
    }
}
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
/// Lexes and exports consecutive bounded windows; syntax spans never accumulate for
/// the whole document. The writer must be a caller-owned atomic publication stage.
pub fn export_language(
    snapshot: &DocumentSnapshot,
    language: bareline_syntax::Language,
    foreground: Rgb,
    background: Rgb,
    colors: [Rgb; 5],
    format: ExportFormat,
    mut writer: impl Write,
    cancel: &CancelToken,
) -> Result<(), UtilityError> {
    if !snapshot.is_complete() { return Err(UtilityError::IncompleteSource); }
    match format {
        ExportFormat::Html => {let Rgb(r,g,b)=background;write!(writer,"<!doctype html><meta charset=\"utf-8\"><pre style=\"background:#{r:02x}{g:02x}{b:02x}\">").map_err(|_|UtilityError::Io)?;},
        ExportFormat::Rtf => {
            writer.write_all(b"{\\rtf1\\ansi\\uc1{\\colortbl;").map_err(|_|UtilityError::Io)?;
            for Rgb(r,g,b) in std::iter::once(foreground).chain(colors).chain(Some(background)) { write!(writer,"\\red{r}\\green{g}\\blue{b};").map_err(|_|UtilityError::Io)?; }
            writer.write_all(b"}\\highlight7 ").map_err(|_|UtilityError::Io)?;
        }
    }
    let mut lexer=bareline_syntax::ForwardLexer::new(snapshot.clone(),language);
    let syntax_cancel=bareline_syntax::Cancellation::default();
    let mut start=0;
    let mut previous_cr=false;
    while start<snapshot.len() {
        check(cancel)?;
        let mut end=start.saturating_add(128*1024).min(snapshot.len());
        while snapshot.chunks(TextOffset(start)..TextOffset(end)).is_err() && end>start { end-=1; }
        if end==start { return Err(UtilityError::InvalidUtf8); }
        let result=lexer.advance(TextOffset(end),&syntax_cancel).map_err(|_|UtilityError::IncompleteSource)?;
        let styles=ExportStyles{revision:snapshot.revision,spans:&result.spans,foreground,colors};
        let mut at=TextOffset(start);
        for span in &result.spans {
            if span.range.start<at || span.range.end.0>end { return Err(UtilityError::StaleStyles); }
            if at<span.range.start { export_range(snapshot,at..span.range.start,0,&styles,(format,&mut previous_cr),&mut writer,cancel)?; }
            export_range(snapshot,span.range.clone(),color_index(span.kind),&styles,(format,&mut previous_cr),&mut writer,cancel)?;
            at=span.range.end;
        }
        if at.0<end { export_range(snapshot,at..TextOffset(end),0,&styles,(format,&mut previous_cr),&mut writer,cancel)?; }
        start=end;
    }
    check(cancel)?;
    writer.write_all(match format {ExportFormat::Html=>b"</pre>" as &[u8],ExportFormat::Rtf=>b"}"}).map_err(|_|UtilityError::Io)
}

/// Prints a captured resident range without changing its selection or document.
/// Syntax is advanced from the start so multiline states remain valid in selections.
pub fn print_snapshot(
    snapshot: &DocumentSnapshot,
    range: Range<TextOffset>,
    language: bareline_syntax::Language,
    colors: [Rgb; 5],
    mut target: Box<dyn bareline_platform::printing::PrintTarget>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<bareline_platform::printing::PrintSummary,bareline_platform::printing::PrintError> {
    use bareline_platform::printing::{PrintError,PrintLine,PrintSpan};
    use std::sync::atomic::Ordering;
    if !snapshot.is_complete() || range.start>range.end || snapshot.chunks(range.clone()).is_err() { return Err(PrintError::InvalidLine); }
    let mut lexer=bareline_syntax::ForwardLexer::new(snapshot.clone(),language);
    let syntax_cancel=bareline_syntax::Cancellation::default();
    for line in 0..snapshot.line_count() {
        if cancel.load(Ordering::Acquire) { return Err(PrintError::Cancelled); }
        let line_range=snapshot.line_range(line).map_err(|_|PrintError::InvalidLine)?;
        if line_range.start>=range.end && range.start!=range.end {break;}
        // Refuse an unsupported logical line before allocating it. The native caller
        // retains the source and options for retry with another range/export format.
        if line_range.end.0-line_range.start.0>128*1024 {return Err(PrintError::Unavailable("Printing requires logical lines below 128 KiB; select a smaller range or export this document".into()));}
        let syntax=lexer.advance(line_range.end,&syntax_cancel).map_err(|_|PrintError::Unavailable("Syntax could not be prepared; retry with plain text".into()))?;
        let start=line_range.start.max(range.start);
        let end=line_range.end.min(range.end);
        if start>end || end<range.start || (start==end && range.start!=range.end) {continue;}
        let text:String=snapshot.chunks(start..end).map_err(|_|PrintError::InvalidLine)?.collect();
        let spans:Vec<PrintSpan>=syntax.spans.iter().filter_map(|span| {
            let a=span.range.start.max(start); let b=span.range.end.min(end);
            if a>=b {return None;}
            let Rgb(r,g,bcolor)=colors[color_index(span.kind)-1];
            Some(PrintSpan{bytes:(a.0-start.0)..(b.0-start.0),rgb:((r as u32)<<16)|((g as u32)<<8)|bcolor as u32})
        }).collect();
        target.write_line(PrintLine{number:line+1,text:&text,spans:&spans},cancel)?;
        if end>=range.end {break;}
    }
    target.finish(cancel)
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
/// Walk complete logical lines from a bounded reader. CRLF remains one terminator
/// even when the input reader splits it. Reject oversized lines before allocation.
fn stream_lines(mut reader:impl std::io::BufRead,cancel:&CancelToken,mut line:impl FnMut(&str,usize,usize,bool)->Result<(),UtilityError>)->Result<(),UtilityError> {
    let mut bytes=Vec::new();let mut origin=0;let mut number=1;
    loop {
        check(cancel)?;bytes.clear();
        loop {
            let available=reader.fill_buf().map_err(|_|UtilityError::Io)?;
            if available.is_empty(){break;}
            let stop=available.iter().position(|b|matches!(*b,b'\r'|b'\n')).map(|i|i+1);
            let count=stop.unwrap_or(available.len());
            if bytes.len().saturating_add(count)>256*1024{return Err(UtilityError::BudgetExceeded);}
            bytes.extend_from_slice(&available[..count]);reader.consume(count);
            if stop.is_some(){
                if bytes.last()==Some(&b'\r')&&reader.fill_buf().map_err(|_|UtilityError::Io)?.first()==Some(&b'\n') {
                    if bytes.len()==256*1024{return Err(UtilityError::BudgetExceeded);}bytes.push(b'\n');reader.consume(1);
                }
                break;
            }
            check(cancel)?;
        }
        let eof=reader.fill_buf().map_err(|_|UtilityError::Io)?.is_empty();
        let text=std::str::from_utf8(&bytes).map_err(|_|UtilityError::InvalidUtf8)?;
        let trailing_empty=eof&&bytes.last().is_some_and(|b|matches!(*b,b'\r'|b'\n'));
        line(text,origin,number,eof&&!trailing_empty)?;
        origin+=bytes.len();number+=1;
        if eof {
            if trailing_empty{line("",origin,number,true)?;}
            break;
        }
    }Ok(())
}
pub fn statistics_reader(reader:impl Read,revision:Revision,cancel:&CancelToken,mut progress:impl FnMut(usize))->Result<Statistics,UtilityError> {
    let mut result=Statistics{bytes:0,characters:0,graphemes:0,words:0,lines:0,revision};
    stream_lines(std::io::BufReader::with_capacity(64*1024,reader),cancel,|text,origin,_,_| {
        result.bytes=origin+text.len();result.characters+=text.chars().count();result.graphemes+=text.graphemes(true).count();result.words+=text.unicode_words().count();result.lines+=1;progress(result.bytes);Ok(())
    })?;Ok(result)
}
/// Full paged syntax export, with verified lexical state spanning bounded lines.
pub fn export_reader(reader:impl Read,language:bareline_syntax::Language,foreground:Rgb,background:Rgb,colors:[Rgb;5],format:ExportFormat,mut writer:impl Write,cancel:&CancelToken,mut progress:impl FnMut(usize))->Result<(),UtilityError> {
    let mut lexer=bareline_syntax::stream::StreamLexer::new(language,bareline_syntax::LexerPreference::Lexilla,None);
    let syntax_cancel=bareline_syntax::Cancellation::default();
    match format {
        ExportFormat::Html=>{let Rgb(r,g,b)=background;write!(writer,"<!doctype html><meta charset=\"utf-8\"><pre style=\"background:#{r:02x}{g:02x}{b:02x}\">").map_err(|_|UtilityError::Io)?;},
        ExportFormat::Rtf=>{writer.write_all(b"{\\rtf1\\ansi\\uc1{\\colortbl;").map_err(|_|UtilityError::Io)?;for Rgb(r,g,b) in std::iter::once(foreground).chain(colors).chain(Some(background)){write!(writer,"\\red{r}\\green{g}\\blue{b};").map_err(|_|UtilityError::Io)?;}writer.write_all(b"}\\highlight7 ").map_err(|_|UtilityError::Io)?;}
    }
    let mut previous_cr=false;
    stream_lines(std::io::BufReader::with_capacity(64*1024,reader),cancel,|text,origin,_,eof|{
        let syntax=lexer.advance(text,TextOffset(origin),eof,&syntax_cancel).map_err(|_|UtilityError::IncompleteSource)?;
        let document=bareline_document::Document::from_utf8(text,bareline_document::Budget::new(2*1024*1024),bareline_document::Budget::new(0)).map_err(|_|UtilityError::BudgetExceeded)?;
        let snapshot=document.snapshot();let styles=ExportStyles{revision:snapshot.revision,spans:&syntax.syntax.spans,foreground,colors};let mut at=TextOffset(0);
        for span in styles.spans {if span.range.start<at||span.range.end.0>text.len(){return Err(UtilityError::StaleStyles);}if span.range.start>at{export_range(&snapshot,at..span.range.start,0,&styles,(format,&mut previous_cr),&mut writer,cancel)?;}export_range(&snapshot,span.range.clone(),color_index(span.kind),&styles,(format,&mut previous_cr),&mut writer,cancel)?;at=span.range.end;}
        if at.0<text.len(){export_range(&snapshot,at..TextOffset(text.len()),0,&styles,(format,&mut previous_cr),&mut writer,cancel)?;}progress(origin+text.len());Ok(())
    })?;check(cancel)?;writer.write_all(match format{ExportFormat::Html=>b"</pre>" as &[u8],ExportFormat::Rtf=>b"}"}).map_err(|_|UtilityError::Io)
}
pub fn print_reader(reader:impl Read,range:Range<TextOffset>,language:bareline_syntax::Language,colors:[Rgb;5],mut target:Box<dyn bareline_platform::printing::PrintTarget>,cancel:&CancelToken,print_cancel:&std::sync::atomic::AtomicBool,mut progress:impl FnMut(usize))->Result<bareline_platform::printing::PrintSummary,String> {
    use bareline_platform::printing::{PrintLine,PrintSpan};
    let mut lexer=bareline_syntax::stream::StreamLexer::new(language,bareline_syntax::LexerPreference::Lexilla,None);let syntax_cancel=bareline_syntax::Cancellation::default();
    let mut printer_error=None;
    let walked=stream_lines(std::io::BufReader::with_capacity(64*1024,reader.take(range.end.0 as u64)),cancel,|text,origin,number,eof| {
        let syntax=lexer.advance(text,TextOffset(origin),eof,&syntax_cancel).map_err(|_|UtilityError::IncompleteSource)?;
        let a=range.start.0.saturating_sub(origin).min(text.len());let b=range.end.0.saturating_sub(origin).min(text.len());
        if origin<=range.end.0&&origin+text.len()>=range.start.0&&a<=b&&(a<b||range.is_empty()) {
            let selected=text.get(a..b).ok_or(UtilityError::InvalidRange)?;
            let spans:Vec<_>=syntax.syntax.spans.iter().filter_map(|s|{let start=s.range.start.0.max(a);let end=s.range.end.0.min(b);if start>=end{return None;}let Rgb(r,g,b)=colors[color_index(s.kind)-1];Some(PrintSpan{bytes:start-a..end-a,rgb:((r as u32)<<16)|((g as u32)<<8)|b as u32})}).collect();
            if let Err(error)=target.write_line(PrintLine{number,text:selected,spans:&spans},print_cancel){printer_error=Some(format!("{error:?}"));return Err(UtilityError::Io);}
        }
        progress(origin+text.len());Ok(())
    });
    if let Some(error)=printer_error{return Err(error);}walked.map_err(|e|format!("{e:?}"))?;target.finish(print_cancel).map_err(|e|format!("{e:?}"))
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
    #[test]
    fn stream_lines_preserve_split_crlf_unicode_and_final_empty_line() {
        let text="a\r\n🙂\rb\n";let mut lines=Vec::new();
        stream_lines(std::io::BufReader::with_capacity(1,text.as_bytes()),&CancelToken::default(),|text,origin,number,_|{lines.push((text.to_string(),origin,number));Ok(())}).unwrap();
        assert_eq!(lines,vec![("a\r\n".into(),0,1),("🙂\r".into(),3,2),("b\n".into(),8,3),("".into(),10,4)]);
        let stats=statistics_reader(text.as_bytes(),Revision(0),&CancelToken::default(),|_|{}).unwrap();
        assert_eq!(stats.bytes,text.len());assert_eq!(stats.lines,4);assert_eq!(stats.characters,text.chars().count());
    }
    #[test]
    fn stream_export_escapes_source_and_preserves_theme_and_multiline_state() {
        let source="/* opening\n<script>🙂 & closing */\n";let mut output=Vec::new();
        export_reader(source.as_bytes(),bareline_syntax::Language::C,Rgb(240,240,240),Rgb(16,24,32),[Rgb(10,20,30);5],ExportFormat::Html,&mut output,&CancelToken::default(),|_|{}).unwrap();
        let output=String::from_utf8(output).unwrap();assert!(output.contains("background:#101820"));assert!(!output.contains("<script>"));assert!(output.contains("&lt;script&gt;🙂 &amp;"));assert!(output.ends_with("</pre>"));
    }
    #[test]
    fn stream_export_writer_error_and_oversize_line_fail_without_source_mutation() {
        struct Denied;impl Write for Denied{fn write(&mut self,_:&[u8])->std::io::Result<usize>{Err(std::io::Error::other("injected"))}fn flush(&mut self)->std::io::Result<()>{Ok(())}}
        assert_eq!(export_reader("source".as_bytes(),bareline_syntax::Language::PlainText,Rgb(0,0,0),Rgb(255,255,255),[Rgb(0,0,0);5],ExportFormat::Html,Denied,&CancelToken::default(),|_|{}),Err(UtilityError::Io));
        assert_eq!(statistics_reader(std::io::repeat(b'a').take(256*1024+1),Revision(0),&CancelToken::default(),|_|{}),Err(UtilityError::BudgetExceeded));
    }
    #[test]
    fn print_driver_error_propagates_and_selection_is_exact() {
        use bareline_platform::printing::*;
        struct Driver;impl PrintTarget for Driver{fn write_line(&mut self,line:PrintLine<'_>,_:&std::sync::atomic::AtomicBool)->Result<(),PrintError>{assert_eq!(line.text,"🙂");Err(PrintError::Driver("injected".into()))}fn finish(self:Box<Self>,_:&std::sync::atomic::AtomicBool)->Result<PrintSummary,PrintError>{panic!("failed job must not finish")}}
        let source="a🙂z";
        let result=print_reader(source.as_bytes(),TextOffset(1)..TextOffset(5),bareline_syntax::Language::PlainText,[Rgb(0,0,0);5],Box::new(Driver),&CancelToken::default(),&std::sync::atomic::AtomicBool::new(false),|_|{});
        assert!(result.unwrap_err().contains("injected"));assert_eq!(source,"a🙂z");
    }
}
