// SPDX-License-Identifier: MPL-2.0
//! Stateless encoding catalog and bounded streaming adapters. Original source ranges
//! must be copied on unchanged same-encoding saves; encoding is not a bijection.
pub mod disk;
pub mod failure;
pub mod resident;
pub mod state;
use crate::{ByteSink, CodecError, DecodedSink, DecodedSpan, Progress, RawOffset, StreamingDecoder, StreamingEncoder};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    Latin1,
    Windows1250,
    Windows1251,
    Windows1252,
    Windows1253,
    Windows1254,
    Windows1255,
    Windows1256,
    Windows1257,
    Windows1258,
    ShiftJis,
    Gbk,
    Big5,
    EucJp,
    EucKr,
}
impl Encoding {
    /// Catalog version 1. ISO-8859-1 aliases deliberately bypass encoding_rs.
    pub fn from_label(label: &str) -> Option<Self> {
        let label = label.trim().to_ascii_lowercase().replace('_', "-");
        Some(match label.as_str() {
            "utf-8" | "utf8" => Self::Utf8,
            "utf-16le" | "utf16le" => Self::Utf16Le,
            "utf-16be" | "utf16be" => Self::Utf16Be,
            "utf-32le" | "utf32le" => Self::Utf32Le,
            "utf-32be" | "utf32be" => Self::Utf32Be,
            "iso-8859-1" | "latin1" | "latin-1" => Self::Latin1,
            "shift-jis" | "sjis" | "windows-31j" => Self::ShiftJis,
            "gbk" | "cp936" => Self::Gbk,
            "big5" | "big-5" => Self::Big5,
            "euc-jp" => Self::EucJp,
            "euc-kr" | "windows-949" => Self::EucKr,
            "windows-1250" | "cp1250" => Self::Windows1250,
            "windows-1251" | "cp1251" => Self::Windows1251,
            "windows-1252" | "cp1252" => Self::Windows1252,
            "windows-1253" | "cp1253" => Self::Windows1253,
            "windows-1254" | "cp1254" => Self::Windows1254,
            "windows-1255" | "cp1255" => Self::Windows1255,
            "windows-1256" | "cp1256" => Self::Windows1256,
            "windows-1257" | "cp1257" => Self::Windows1257,
            "windows-1258" | "cp1258" => Self::Windows1258,
            _ => return None,
        })
    }
    /// The one user-facing name of each encoding, shared by the status bar,
    /// menus and pickers (UI-07). Never show the `Debug` spelling.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Utf32Le => "UTF-32 LE",
            Self::Utf32Be => "UTF-32 BE",
            Self::Latin1 => "ISO-8859-1 (Western)",
            Self::Windows1250 => "Windows-1250 (Central European)",
            Self::Windows1251 => "Windows-1251 (Cyrillic)",
            Self::Windows1252 => "Windows-1252 (Western / ANSI)",
            Self::Windows1253 => "Windows-1253 (Greek)",
            Self::Windows1254 => "Windows-1254 (Turkish)",
            Self::Windows1255 => "Windows-1255 (Hebrew)",
            Self::Windows1256 => "Windows-1256 (Arabic)",
            Self::Windows1257 => "Windows-1257 (Baltic)",
            Self::Windows1258 => "Windows-1258 (Vietnamese)",
            Self::ShiftJis => "Shift-JIS (Japanese)",
            Self::Gbk => "GBK (Simplified Chinese)",
            Self::Big5 => "Big5 (Traditional Chinese)",
            Self::EucJp => "EUC-JP (Japanese)",
            Self::EucKr => "EUC-KR (Korean)",
        }
    }
    /// Status-bar label: the display name, marked when a byte-order mark is kept.
    pub fn status_label(self, bom: bool) -> String {
        if bom {
            format!("{} BOM", self.display_name())
        } else {
            self.display_name().to_owned()
        }
    }
    pub fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 => &[239, 187, 191],
            Self::Utf16Le => &[255, 254],
            Self::Utf16Be => &[254, 255],
            Self::Utf32Le => &[255, 254, 0, 0],
            Self::Utf32Be => &[0, 0, 254, 255],
            _ => &[],
        }
    }
    /// One raw byte per scalar: ISO-8859-1 and the Windows code pages.
    fn single_byte(self) -> bool {
        matches!(
            self,
            Self::Latin1
                | Self::Windows1250
                | Self::Windows1251
                | Self::Windows1252
                | Self::Windows1253
                | Self::Windows1254
                | Self::Windows1255
                | Self::Windows1256
                | Self::Windows1257
                | Self::Windows1258
        )
    }
    fn legacy(self) -> Option<&'static encoding_rs::Encoding> {
        use encoding_rs::*;
        Some(match self {
            Self::Windows1250 => WINDOWS_1250,
            Self::Windows1251 => WINDOWS_1251,
            Self::Windows1252 => WINDOWS_1252,
            Self::Windows1253 => WINDOWS_1253,
            Self::Windows1254 => WINDOWS_1254,
            Self::Windows1255 => WINDOWS_1255,
            Self::Windows1256 => WINDOWS_1256,
            Self::Windows1257 => WINDOWS_1257,
            Self::Windows1258 => WINDOWS_1258,
            Self::ShiftJis => SHIFT_JIS,
            Self::Gbk => GBK,
            Self::Big5 => BIG5,
            Self::EucJp => EUC_JP,
            Self::EucKr => EUC_KR,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Confidence {
    Bom,
    Utf8Sample,
    /// BOM-less UTF-16 inferred from its NUL byte pattern.
    Utf16Sample,
    LegacySample,
    LegacyFallback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Detection {
    pub encoding: Encoding,
    pub confidence: Confidence,
    pub bom: bool,
    pub binary_warning: bool,
    /// Likeliest legacy encodings, best first, when an ambiguous sample fell back
    /// (LegacyFallback). The UI offers them as an "encoding may be wrong" hint.
    #[serde(default)]
    pub candidates: [Option<Encoding>; 3],
}
/// Legacy multi-byte encodings scored when the sample is not UTF-8 (FIO-05).
const LEGACY_CANDIDATES: [Encoding; 5] = [
    Encoding::Gbk,
    Encoding::Big5,
    Encoding::EucJp,
    Encoding::ShiftJis,
    Encoding::EucKr,
];
/// Fewer decoded non-ASCII scalars than this are too little evidence to choose.
const MIN_LEGACY_SCALARS: usize = 12;
/// Validity covers the whole sample; frequency scoring reads this many scalars.
const MAX_SCORED_SCALARS: usize = 4096;
/// A winner needs this share (per mille) of common characters, and at least twice
/// the runner-up's share. Mojibake decodes land near zero; real text near 60%+.
const MIN_COMMON_PER_MILLE: usize = 150;
/// Candidates offered by the ambiguity hint need this much evidence.
const HINT_COMMON_PER_MILLE: usize = 100;
/// Most frequent Simplified Chinese characters and punctuation.
const COMMON_HANS: &str = "的一是不了在人有我他这个们中来上大为和国地到以说时要就出会可也你对生能而子那得于着下自之年过发后作里用道行所然家种事成方多经么去法学如都同现当没动面起看定天分还进好小部其些主样理心她本前开但因只从想实日军者意无力它与长把机十民第公此已工使情明性知全三又关点正业外将两高间由问很最重并物手应战向头文体政美相见被利什二等产或新己制身果加西月话合回特代内信表化老给世位次度门任常先海通教儿原东声提立及比员解，。、；：？！“”（）《》";
/// Most frequent Traditional Chinese characters and punctuation.
const COMMON_HANT: &str = "的一是不了在人有我他這個們中來上大為和國地到以說時要就出會可也你對生能而子那得於著下自之年過發後作裡用道行所然家種事成方多經麼去法學如都同現當沒動面起看定天分還進好小部其些主樣理心她本前開但因只從想實日者意無力它與長把機十民第公此已工使情明性知全三又關點正業外將兩高間由問很最重並物手應向頭文體政美相見被利什二等產或新己制身果加西月話合回特代內信表化老給世位次度門任常先海通教兒原東聲提立及比員解水名，。、；：？！「」『』（）";
/// Most frequent Hangul syllables.
const COMMON_HANGUL: &str = "이다는의에하고을가지로서기한리도사자어를일대수으인나시해그있들것적정여장아보게전부제상주우라국과거되면만없연동성소스발신내원공오문비요위경구무화계학생중음선개결관방물세조치말마은저까때했입년모할러니합습좋또알같두데드르받분영실안않와용유작잘점진차체터통트파표함행현회후날너네운월";
/// Most frequent kanji; kana and Japanese punctuation are matched by range.
const COMMON_KANJI: &str = "日本人年大中出会時行事自者生分上前見言地社国一二三十月今何私手気方思家間業東京長";
fn is_common(encoding: Encoding, c: char) -> bool {
    match encoding {
        Encoding::Gbk => COMMON_HANS.contains(c),
        Encoding::Big5 => COMMON_HANT.contains(c),
        Encoding::EucKr => COMMON_HANGUL.contains(c),
        _ => {
            matches!(c, '\u{3041}'..='\u{3096}' | '\u{30a1}'..='\u{30fc}' | '、' | '。' | '「' | '」')
                || COMMON_KANJI.contains(c)
        }
    }
}
/// `None` unless the whole sample is valid in `encoding` (a truncated sample may
/// end inside a unit); otherwise (common, scored) non-ASCII scalar counts.
fn legacy_evidence(encoding: Encoding, raw: &[u8], truncated: bool) -> Option<(usize, usize)> {
    let mut decoder = encoding.legacy()?.new_decoder_without_bom_handling();
    let mut text = String::with_capacity(decoder.max_utf8_buffer_length_without_replacement(raw.len())?);
    let (result, _) = decoder.decode_to_string_without_replacement(raw, &mut text, !truncated);
    if result != encoding_rs::DecoderResult::InputEmpty {
        return None;
    }
    let (mut common, mut scored) = (0, 0);
    for c in text.chars().filter(|c| !c.is_ascii()).take(MAX_SCORED_SCALARS) {
        scored += 1;
        common += usize::from(is_common(encoding, c));
    }
    Some((common, scored))
}
/// Scored legacy detection (FIO-05): every valid candidate is ranked by its share
/// of common characters. Returns the winner, or `None` and the hint candidates
/// when the sample is too small or no candidate clearly leads.
fn detect_legacy(raw: &[u8], truncated: bool) -> (Option<Encoding>, [Option<Encoding>; 3]) {
    let mut scored: Vec<(Encoding, usize, usize)> = LEGACY_CANDIDATES
        .into_iter()
        .filter_map(|e| legacy_evidence(e, raw, truncated).map(|(common, n)| (e, common, n)))
        .filter(|&(_, _, n)| n > 0)
        .collect();
    // Descending common/scored ratio, cross-multiplied to stay exact; stable ties.
    scored.sort_by(|a, b| (b.1 * a.2).cmp(&(a.1 * b.2)));
    if let [best, rest @ ..] = scored.as_slice()
        && best.2 >= MIN_LEGACY_SCALARS
        && best.1 * 1000 >= best.2 * MIN_COMMON_PER_MILLE
        && rest
            .first()
            .is_none_or(|second| best.1 * second.2 >= 2 * second.1 * best.2)
    {
        return (Some(best.0), [None; 3]);
    }
    let mut candidates = [None; 3];
    for (slot, &(e, _, _)) in candidates.iter_mut().zip(
        scored
            .iter()
            .filter(|&&(_, common, n)| common * 1000 >= n * HINT_COMMON_PER_MILLE),
    ) {
        *slot = Some(e);
    }
    (None, candidates)
}
/// BOM-less UTF-16 (FIO-06): text that is partly ASCII puts a NUL in the high
/// byte of many units and almost never in the low byte. The units must also read
/// as text: paired surrogates and no more controls than the binary threshold.
fn detect_utf16_without_bom(raw: &[u8]) -> Option<Encoding> {
    let (pairs, _) = raw.as_chunks::<2>();
    let units = pairs.len();
    if units < 2 {
        return None;
    }
    let even = pairs.iter().filter(|pair| pair[0] == 0).count();
    let odd = pairs.iter().filter(|pair| pair[1] == 0).count();
    let encoding = if odd * 5 >= units && even * 50 <= units {
        Encoding::Utf16Le
    } else if even * 5 >= units && odd * 50 <= units {
        Encoding::Utf16Be
    } else {
        return None;
    };
    let (mut controls, mut high) = (0, false);
    for &pair in pairs {
        let unit = if encoding == Encoding::Utf16Le {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        };
        // A low surrogate must follow a high one, and only a low one may. A high
        // surrogate at the end of a truncated sample stays optimistic.
        if high != (0xdc00..=0xdfff).contains(&unit) {
            return None;
        }
        high = (0xd800..=0xdbff).contains(&unit);
        controls += usize::from(unit < 32 && !matches!(unit, 9 | 10 | 13));
    }
    (controls * 10 <= units).then_some(encoding)
}
/// FF FE 00 00 is also a UTF-16LE BOM followed by U+0000 (FIO-06). UTF-32LE is
/// kept only when the sample is whole 32-bit units that are all scalar values.
fn fits_utf32le(raw: &[u8]) -> bool {
    let (units, rest) = raw[4..].as_chunks::<4>();
    rest.is_empty()
        && units
            .iter()
            .all(|&unit| char::from_u32(u32::from_le_bytes(unit)).is_some())
}
/// At most 64 KiB are inspected. An incomplete final UTF-8 sequence remains optimistic.
pub fn detect(raw: &[u8]) -> Detection {
    let truncated = raw.len() >= 65536;
    let raw = &raw[..raw.len().min(65536)];
    let binary_warning = !raw.is_empty()
        && raw
            .iter()
            .filter(|&&b| b == 0 || (b < 32 && !matches!(b, 9 | 10 | 13)))
            .count()
            * 10
            > raw.len();
    for encoding in [
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
    ] {
        if raw.starts_with(encoding.bom()) && (encoding != Encoding::Utf32Le || fits_utf32le(raw)) {
            return Detection {
                encoding,
                confidence: Confidence::Bom,
                bom: true,
                binary_warning: false,
                candidates: [None; 3],
            };
        }
    }
    if let Some(encoding) = detect_utf16_without_bom(raw) {
        return Detection {
            encoding,
            confidence: Confidence::Utf16Sample,
            bom: false,
            binary_warning: false,
            candidates: [None; 3],
        };
    }
    let utf8 = match std::str::from_utf8(raw) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    };
    let mut candidates = [None; 3];
    if !utf8 {
        // Bounded scored trial decoding: script evidence is advisory, never grounds
        // to reinterpret later chunks. Ambiguous samples fall back with candidates.
        let (winner, hint) = detect_legacy(raw, truncated);
        if let Some(encoding) = winner {
            return Detection {
                encoding,
                confidence: Confidence::LegacySample,
                bom: false,
                binary_warning,
                candidates: [None; 3],
            };
        }
        candidates = hint;
    }
    Detection {
        encoding: if utf8 { Encoding::Utf8 } else { Encoding::Windows1252 },
        confidence: if utf8 {
            Confidence::Utf8Sample
        } else {
            Confidence::LegacyFallback
        },
        bom: false,
        binary_warning,
        candidates,
    }
}

/// Raw bytes one bulk span of a non-UTF-8 encoding covers at most (FIO-09). It
/// bounds a mixed-width provenance run, and so the unit walk that locates a
/// boundary inside one (`run_boundaries`).
pub(crate) const SPAN_RAW: usize = 16 * 1024;
#[cfg(test)]
thread_local! {
    /// Unit widths the bulk legacy decoder walked, for the test that bounds them.
    static WIDTH_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
const REPLACEMENT: &str = "\u{fffd}";
fn put(text: &mut [u8; 16], s: &str) -> usize {
    text[..s.len()].copy_from_slice(s.as_bytes());
    s.len()
}
fn utf16_word(encoding: Encoding, b: &[u8]) -> u16 {
    if encoding == Encoding::Utf16Le {
        u16::from_le_bytes([b[0], b[1]])
    } else {
        u16::from_be_bytes([b[0], b[1]])
    }
}
/// Byte length of the legacy unit that starts `p`, as its lead byte announces it.
/// `None` when the lead byte alone cannot tell: a GBK lead followed by an ASCII
/// digit starts a four-byte GB18030 unit, which the GBK decoder accepts.
fn legacy_width(encoding: Encoding, p: &[u8]) -> Option<usize> {
    let lead = p[0];
    Some(match encoding {
        Encoding::ShiftJis if matches!(lead, 0x81..=0x9f | 0xe0..=0xfc) => 2,
        Encoding::Gbk if (0x81..=0xfe).contains(&lead) => match p.get(1)? {
            0x30..=0x39 => 4,
            _ => 2,
        },
        Encoding::Gbk | Encoding::Big5 | Encoding::EucKr if lead >= 0x81 => 2,
        Encoding::EucJp if lead == 0x8f => 3,
        Encoding::EucJp if lead >= 0x80 => 2,
        _ => 1,
    })
}
/// Decode the unit that starts `p` into `text`: (raw bytes, text bytes, invalid).
/// `None` means one more byte is needed. Invalid units retain their exact bytes
/// and display one U+FFFD. `p` needs to hold at most the four bytes of one unit.
fn unit(encoding: Encoding, p: &[u8], end: bool, text: &mut [u8; 16]) -> Option<(usize, usize, bool)> {
    let first = *p.first()?;
    let scalar = |n: usize, c: Option<char>, text: &mut [u8; 16]| match c {
        Some(c) => (n, c.encode_utf8(text).len(), false),
        None => (n, put(text, REPLACEMENT), true),
    };
    Some(match encoding {
        Encoding::Utf8 => {
            let valid = match std::str::from_utf8(p) {
                Ok(s) => s,
                Err(e) if e.valid_up_to() > 0 => std::str::from_utf8(&p[..e.valid_up_to()]).ok()?,
                Err(e) => match e.error_len() {
                    Some(n) => return Some(scalar(n, None, text)),
                    None if end => return Some(scalar(p.len(), None, text)),
                    None => return None,
                },
            };
            let c = valid.chars().next()?;
            scalar(c.len_utf8(), Some(c), text)
        }
        Encoding::Latin1 => scalar(1, Some(char::from(first)), text),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            if p.len() < 2 {
                return end.then(|| scalar(p.len(), None, text));
            }
            let w = utf16_word(encoding, p);
            if (0xd800..=0xdbff).contains(&w) {
                if p.len() < 4 {
                    return end.then(|| scalar(2, None, text));
                }
                let low = utf16_word(encoding, &p[2..]);
                if !(0xdc00..=0xdfff).contains(&low) {
                    return Some(scalar(2, None, text));
                }
                scalar(
                    4,
                    char::from_u32(0x10000 + ((u32::from(w) - 0xd800) << 10) + u32::from(low) - 0xdc00),
                    text,
                )
            } else {
                scalar(2, char::from_u32(u32::from(w)), text)
            }
        }
        Encoding::Utf32Le | Encoding::Utf32Be => {
            if p.len() < 4 {
                return end.then(|| scalar(p.len(), None, text));
            }
            let b = [p[0], p[1], p[2], p[3]];
            scalar(
                4,
                char::from_u32(if encoding == Encoding::Utf32Le {
                    u32::from_le_bytes(b)
                } else {
                    u32::from_be_bytes(b)
                }),
                text,
            )
        }
        legacy => {
            let width = match legacy_width(legacy, p) {
                Some(width) => width,
                None if end => 1,
                None => return None,
            };
            if p.len() < width && !end {
                return None;
            }
            let n = width.min(p.len());
            let mut decoder = legacy.legacy()?.new_decoder_without_bom_handling();
            match decoder.decode_to_utf8_without_replacement(&p[..n], text, true) {
                (encoding_rs::DecoderResult::InputEmpty, read, written) if read == n => (n, written, false),
                _ => (1, put(text, REPLACEMENT), true),
            }
        }
    })
}
/// Length of `raw` without a trailing sequence that is only cut short, so a bulk
/// UTF-8 span validates once instead of twice at a chunk end.
fn utf8_complete_len(raw: &[u8]) -> usize {
    for back in 1..=raw.len().min(4) {
        let byte = raw[raw.len() - back];
        if byte & 0xc0 != 0x80 {
            let width = match byte {
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => 1,
            };
            return if width > back { raw.len() - back } else { raw.len() };
        }
    }
    raw.len()
}
/// The longest prefix of `raw` that `encoding` decodes as whole valid units, with
/// its text left in `text`. A malformed or cut-short unit ends the prefix.
fn legacy_prefix(encoding: &'static encoding_rs::Encoding, raw: &[u8], text: &mut String) -> usize {
    let decode = |bytes: &[u8], text: &mut String| {
        text.clear();
        let mut decoder = encoding.new_decoder_without_bom_handling();
        if let Some(needed) = decoder.max_utf8_buffer_length_without_replacement(bytes.len()) {
            text.reserve(needed);
        }
        decoder.decode_to_string_without_replacement(bytes, text, true)
    };
    match decode(raw, text) {
        (encoding_rs::DecoderResult::InputEmpty, read) if read == raw.len() => read,
        (encoding_rs::DecoderResult::Malformed(bad, after), read) => {
            // Decode the valid prefix again on its own, so its text can never hold
            // output of bytes the decoder consumed after the malformed sequence.
            let valid = read.saturating_sub(usize::from(bad) + usize::from(after));
            if valid > 0 && decode(&raw[..valid], text) == (encoding_rs::DecoderResult::InputEmpty, valid) {
                valid
            } else {
                text.clear();
                0
            }
        }
        _ => {
            text.clear();
            0
        }
    }
}
/// Constant (text, raw) unit widths of a valid decoded span, when they are cheap
/// to know (FIO-02): valid UTF-8 is its own bytes, and ASCII text is one scalar per
/// code unit. Any other valid span is a mixed-width run (`run_boundaries`).
pub(crate) fn uniform_units(encoding: Encoding, text: &str, raw_len: usize) -> Option<(usize, usize)> {
    let raw_unit = match encoding {
        Encoding::Utf8 => return (text.len() == raw_len).then_some((1, 1)),
        Encoding::Utf16Le | Encoding::Utf16Be => 2,
        Encoding::Utf32Le | Encoding::Utf32Be => 4,
        _ => 1,
    };
    (text.is_ascii() && text.len() * raw_unit == raw_len).then_some((1, raw_unit))
}
/// Unit boundaries inside a mixed-width provenance run (FIO-02). The run's raw
/// bytes are whole valid units, walked here exactly as the decoder reads them.
/// Returns the first unit boundary at or after text offset `from` and the last one
/// at or before `to`, as (text, raw) ranges relative to the run, or `None` when no
/// whole unit lies between them. Fails unless every unit decodes and the run holds
/// exactly `text_len` text bytes, so a disagreeing store is never misaligned.
pub(crate) fn run_boundaries(
    encoding: Encoding,
    raw: &[u8],
    text_len: usize,
    from: usize,
    to: usize,
) -> Result<Option<(std::ops::Range<usize>, std::ops::Range<usize>)>, CodecError> {
    let mut buffer = [0u8; 16];
    let (mut text, mut at) = (0usize, 0usize);
    let (mut first, mut last) = (None, None);
    loop {
        if first.is_none() && text >= from {
            first = Some((text, at));
        }
        if text <= to {
            last = Some((text, at));
        }
        if at == raw.len() {
            break;
        }
        let window = &raw[at..raw.len().min(at + 4)];
        match unit(encoding, window, true, &mut buffer) {
            Some((n, len, false)) => {
                at += n;
                text += len;
            }
            _ => return Err(CodecError::InvalidSequence),
        }
    }
    if text != text_len {
        return Err(CodecError::InvalidSequence);
    }
    Ok(match (first, last) {
        (Some(a), Some(b)) if a.0 <= b.0 => Some((a.0..b.0, a.1..b.1)),
        _ => None,
    })
}

#[derive(Clone)]
pub struct Decoder {
    encoding: Encoding,
    pending: Vec<u8>,
    offset: u64,
    start: bool,
    /// Reused bulk text buffer; empty between pushes.
    text: String,
    /// Units read one at a time: the BOM, invalid bytes and units split across
    /// pushes or a full sink. Bulk decoding covers everything else (FIO-09).
    unit_reads: u64,
}
impl Decoder {
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            pending: Vec::with_capacity(4),
            offset: 0,
            start: true,
            text: String::new(),
            unit_reads: 0,
        }
    }
    /// Units read one at a time so far (the rest decode in bulk), for tests and
    /// diagnostics that pin bulk decoding (FIO-09).
    pub fn unit_reads(&self) -> u64 {
        self.unit_reads
    }
    /// Bulk decode (FIO-09): the longest prefix of `raw` made of whole valid units,
    /// written as one span. It stops before an invalid or incomplete unit, which
    /// the unit-by-unit path then reads exactly as it always did, so spans differ
    /// only in how many units they group. Returns the (raw, text) bytes written.
    fn bulk(&mut self, raw: &[u8], out: &mut dyn DecodedSink) -> Result<(usize, usize), CodecError> {
        if self.encoding == Encoding::Utf8 {
            let complete = &raw[..utf8_complete_len(raw)];
            let text = match std::str::from_utf8(complete) {
                Ok(text) => text,
                Err(error) => {
                    std::str::from_utf8(&complete[..error.valid_up_to()]).map_err(|_| CodecError::InvalidSequence)?
                }
            };
            return self.emit(text, text.len(), out);
        }
        let mut text = std::mem::take(&mut self.text);
        text.clear();
        let valid = match self.encoding {
            Encoding::Utf8 => 0,
            Encoding::Latin1 => {
                text.extend(raw.iter().map(|&byte| char::from(byte)));
                raw.len()
            }
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let mut at = 0;
                while at + 2 <= raw.len() {
                    let w = utf16_word(self.encoding, &raw[at..]);
                    let (c, n) = match w {
                        0xd800..=0xdbff => {
                            if at + 4 > raw.len() {
                                break;
                            }
                            let low = utf16_word(self.encoding, &raw[at + 2..]);
                            if !(0xdc00..=0xdfff).contains(&low) {
                                break;
                            }
                            (
                                char::from_u32(0x10000 + ((u32::from(w) - 0xd800) << 10) + u32::from(low) - 0xdc00),
                                4,
                            )
                        }
                        _ => (char::from_u32(u32::from(w)), 2),
                    };
                    let Some(c) = c else {
                        break;
                    };
                    text.push(c);
                    at += n;
                }
                at
            }
            Encoding::Utf32Le | Encoding::Utf32Be => {
                let mut at = 0;
                while at + 4 <= raw.len() {
                    let b = [raw[at], raw[at + 1], raw[at + 2], raw[at + 3]];
                    let value = if self.encoding == Encoding::Utf32Le {
                        u32::from_le_bytes(b)
                    } else {
                        u32::from_be_bytes(b)
                    };
                    let Some(c) = char::from_u32(value) else {
                        break;
                    };
                    text.push(c);
                    at += 4;
                }
                at
            }
            legacy => match legacy.legacy() {
                // Decode first: encoding_rs stops at the first malformed byte and
                // reports a unit cut short by the span end as malformed at its lead
                // byte, so an attempt costs only the prefix it decodes, never the
                // whole span, however dense the invalid units are (FIO-09).
                Some(decoder) => {
                    let valid = legacy_prefix(decoder, raw, &mut text);
                    if legacy.single_byte() {
                        valid
                    } else {
                        // End on a whole unit as the unit path counts them, walking
                        // only the prefix already decoded.
                        let mut cut = 0;
                        while cut < valid {
                            #[cfg(test)]
                            WIDTH_STEPS.with(|steps| steps.set(steps.get() + 1));
                            match legacy_width(legacy, &raw[cut..]) {
                                Some(width) if cut + width <= valid => cut += width,
                                _ => break,
                            }
                        }
                        if cut == valid {
                            valid
                        } else {
                            legacy_prefix(decoder, &raw[..cut], &mut text)
                        }
                    }
                }
                None => 0,
            },
        };
        let result = self.emit(&text, valid, out);
        text.clear();
        self.text = text;
        result
    }
    fn emit(&mut self, text: &str, raw_len: usize, out: &mut dyn DecodedSink) -> Result<(usize, usize), CodecError> {
        if raw_len == 0 {
            return Ok((0, 0));
        }
        out.write(DecodedSpan {
            text,
            original: RawOffset(self.offset)..RawOffset(self.offset + raw_len as u64),
            opaque_bytes: None,
        })?;
        self.offset += raw_len as u64;
        Ok((raw_len, text.len()))
    }
}
impl StreamingDecoder for Decoder {
    fn push(&mut self, raw: &[u8], end: bool, out: &mut dyn DecodedSink) -> Result<Progress, CodecError> {
        let mut consumed = 0;
        let mut produced = 0;
        loop {
            let final_unit = end && consumed == raw.len();
            if self.start {
                let bom = self.encoding.bom();
                if !bom.is_empty() && bom.starts_with(&self.pending) && self.pending.len() < bom.len() && !final_unit {
                    if consumed == raw.len() {
                        break;
                    }
                    self.pending.push(raw[consumed]);
                    consumed += 1;
                    continue;
                }
                if !bom.is_empty() && self.pending == bom {
                    out.write(DecodedSpan {
                        text: "",
                        original: RawOffset(0)..RawOffset(bom.len() as u64),
                        opaque_bytes: None,
                    })?;
                    self.offset += bom.len() as u64;
                    self.pending.clear();
                }
                self.start = false;
            }
            // Whole valid units decode in bulk. Text is at most three bytes per raw
            // byte, so a span limited to a third of the sink's room always fits.
            if self.pending.is_empty() && consumed < raw.len() {
                let room = out.remaining_capacity() / 3;
                if room >= 4 {
                    let limit = if self.encoding == Encoding::Utf8 {
                        room
                    } else {
                        room.min(SPAN_RAW)
                    };
                    let segment = &raw[consumed..raw.len().min(consumed.saturating_add(limit))];
                    let (read, written) = self.bulk(segment, out)?;
                    if read > 0 {
                        consumed += read;
                        produced += written;
                        continue;
                    }
                }
            }
            let mut buffer = [0u8; 16];
            if let Some((n, len, invalid)) = unit(self.encoding, &self.pending, final_unit, &mut buffer) {
                if out.remaining_capacity() < len {
                    return Ok(Progress {
                        consumed,
                        produced,
                        needs_output: true,
                    });
                }
                out.write(DecodedSpan {
                    text: std::str::from_utf8(&buffer[..len]).map_err(|_| CodecError::InvalidSequence)?,
                    original: RawOffset(self.offset)..RawOffset(self.offset + n as u64),
                    opaque_bytes: invalid.then_some(&self.pending[..n]),
                })?;
                self.pending.drain(..n);
                self.offset += n as u64;
                self.unit_reads += 1;
                produced += len;
            } else if consumed < raw.len() {
                self.pending.push(raw[consumed]);
                consumed += 1;
            } else {
                break;
            }
        }
        Ok(Progress {
            consumed,
            produced,
            needs_output: false,
        })
    }
}

pub struct Encoder {
    encoding: Encoding,
    bom: bool,
    span_offset: usize,
}
impl Encoder {
    pub fn new(encoding: Encoding, bom: bool) -> Self {
        Self {
            encoding,
            bom,
            span_offset: 0,
        }
    }
    /// Convenience for caller-bounded chunks. Streaming push never allocates by span size.
    pub fn encode_text(&self, text: &str) -> Result<Vec<u8>, CodecError> {
        if self.encoding == Encoding::Utf8 {
            return Ok(text.as_bytes().to_vec());
        }
        let mut result = Vec::new();
        for c in text.chars() {
            match self.encoding {
                Encoding::Utf8 => {
                    let mut b = [0; 4];
                    result.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                }
                Encoding::Latin1 => {
                    if c as u32 > 255 {
                        return Err(CodecError::Unrepresentable);
                    }
                    result.push(c as u8);
                }
                Encoding::Utf16Le | Encoding::Utf16Be => {
                    let mut b = [0; 2];
                    for w in c.encode_utf16(&mut b) {
                        result.extend_from_slice(&if self.encoding == Encoding::Utf16Le {
                            w.to_le_bytes()
                        } else {
                            w.to_be_bytes()
                        });
                    }
                }
                Encoding::Utf32Le => result.extend_from_slice(&(c as u32).to_le_bytes()),
                Encoding::Utf32Be => result.extend_from_slice(&(c as u32).to_be_bytes()),
                e => {
                    let mut b = [0; 4];
                    let (bytes, _, errors) = e.legacy().unwrap().encode(c.encode_utf8(&mut b));
                    if errors {
                        return Err(CodecError::Unrepresentable);
                    }
                    result.extend_from_slice(&bytes);
                }
            }
        }
        Ok(result)
    }
}
impl StreamingEncoder for Encoder {
    /// Progress.consumed counts complete input spans. On backpressure pass the
    /// unconsumed spans again unchanged: the partial scalar offset is retained.
    fn push(&mut self, spans: &[DecodedSpan<'_>], _end: bool, out: &mut dyn ByteSink) -> Result<Progress, CodecError> {
        let mut produced = 0;
        if self.bom {
            let bom = self.encoding.bom();
            if out.remaining_capacity() < bom.len() {
                return Ok(Progress {
                    consumed: 0,
                    produced: 0,
                    needs_output: true,
                });
            }
            out.write(bom)?;
            produced += bom.len();
            self.bom = false;
        }
        for (consumed, span) in spans.iter().enumerate() {
            if span.opaque_bytes.is_some() {
                return Err(CodecError::UnresolvedOpaqueBytes);
            }
            let remaining = span.text.get(self.span_offset..).ok_or(CodecError::InvalidSequence)?;
            for c in remaining.chars() {
                let mut utf8 = [0; 4];
                let bytes = self.encode_text(c.encode_utf8(&mut utf8))?;
                if out.remaining_capacity() < bytes.len() {
                    return Ok(Progress {
                        consumed,
                        produced,
                        needs_output: true,
                    });
                }
                out.write(&bytes)?;
                produced += bytes.len();
                self.span_offset += c.len_utf8();
            }
            self.span_offset = 0;
        }
        Ok(Progress {
            consumed: spans.len(),
            produced,
            needs_output: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Sink {
        text: String,
        invalid: Vec<(u64, Vec<u8>)>,
        ranges: Vec<(u64, u64)>,
    }
    impl DecodedSink for Sink {
        fn remaining_capacity(&self) -> usize {
            usize::MAX
        }
        fn write(&mut self, s: DecodedSpan<'_>) -> Result<(), CodecError> {
            self.text.push_str(s.text);
            self.ranges.push((s.original.start.0, s.original.end.0));
            if let Some(b) = s.opaque_bytes {
                self.invalid.push((s.original.start.0, b.to_vec()));
            }
            Ok(())
        }
    }
    fn decode(e: Encoding, raw: &[u8], chunk: usize) -> Sink {
        let mut d = Decoder::new(e);
        let mut s = Sink::default();
        d.push(&[], false, &mut s).unwrap();
        for part in raw.chunks(chunk) {
            let p = d.push(part, false, &mut s).unwrap();
            assert_eq!(p.consumed, part.len());
        }
        d.push(&[], true, &mut s).unwrap();
        s
    }
    #[test]
    fn unicode_boundaries_bom_and_mixed_eol() {
        for e in [
            Encoding::Utf8,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
        ] {
            let text = "A\r\n😀\r漢\n\u{fffd}\u{e000}";
            let mut raw = e.bom().to_vec();
            raw.extend(Encoder::new(e, false).encode_text(text).unwrap());
            for n in 1..=5 {
                let s = decode(e, &raw, n);
                assert_eq!(s.text, text);
                assert!(s.invalid.is_empty());
                assert_eq!(s.ranges.last().unwrap().1, raw.len() as u64);
            }
            assert_eq!(detect(&raw).encoding, e);
        }
    }
    #[test]
    fn malformed_sequences_retain_raw_ranges_without_unicode_collisions() {
        let raw = b"\xef\xbf\xbd\xee\x80\x80\xf0\x9fX\xff";
        for n in 1..=5 {
            let s = decode(Encoding::Utf8, raw, n);
            assert_eq!(s.text, "\u{fffd}\u{e000}\u{fffd}X\u{fffd}");
            assert_eq!(s.invalid, vec![(6, vec![0xf0, 0x9f]), (9, vec![0xff])]);
        }
        let s = decode(Encoding::Utf16Le, &[0, 0xd8, 65, 0, 1], 1);
        assert_eq!(s.text, "�A�");
        assert_eq!(s.invalid, vec![(0, vec![0, 0xd8]), (4, vec![1])]);
        let s = decode(Encoding::Utf32Be, &[0, 0x11, 0, 0, 1], 1);
        assert_eq!(s.invalid.len(), 2);
    }
    #[test]
    fn legacy_catalog_and_true_latin1() {
        for e in [
            Encoding::Windows1250,
            Encoding::Windows1251,
            Encoding::Windows1252,
            Encoding::Windows1253,
            Encoding::Windows1254,
            Encoding::Windows1255,
            Encoding::Windows1256,
            Encoding::Windows1257,
            Encoding::Windows1258,
            Encoding::ShiftJis,
            Encoding::Gbk,
            Encoding::Big5,
            Encoding::EucJp,
            Encoding::EucKr,
        ] {
            let raw = Encoder::new(e, false).encode_text("A\r\n").unwrap();
            assert_eq!(decode(e, &raw, 1).text, "A\r\n");
        }
        for (e, text) in [
            (Encoding::ShiftJis, "日本"),
            (Encoding::Gbk, "中文"),
            (Encoding::Big5, "中文"),
            (Encoding::EucJp, "日本"),
            (Encoding::EucKr, "한국"),
            (Encoding::Windows1252, "€é"),
        ] {
            let raw = Encoder::new(e, false).encode_text(text).unwrap();
            for n in 1..=3 {
                assert_eq!(decode(e, &raw, n).text, text);
            }
        }
        assert_eq!(decode(Encoding::Latin1, &[0x80, 0xff], 1).text, "\u{80}ÿ");
        assert_eq!(
            Encoder::new(Encoding::Latin1, false).encode_text("\u{80}ÿ").unwrap(),
            [0x80, 0xff]
        );
        assert!(matches!(
            Encoder::new(Encoding::Latin1, false).encode_text("€"),
            Err(CodecError::Unrepresentable)
        ));
    }
    #[test]
    fn detection_is_bounded_and_advisory() {
        let mut raw = vec![b'a'; 65537];
        raw[65536] = 255;
        assert_eq!(detect(&raw).encoding, Encoding::Utf8);
        assert_eq!(detect(&[255]).confidence, Confidence::LegacyFallback);
        assert!(detect(&[0; 100]).binary_warning);
    }
    const ZH_HANS: &str = "我们今天在会议上讨论了新的项目计划。大家都认为这个方案可以提高工作效率，但是还需要更多的时间来准备。经理说下个月开始实施，每个人都要按照时间表完成自己的任务。如果有问题，可以随时联系我们的技术部门。";
    const ZH_HANT: &str = "我們今天在會議上討論了新的專案計畫。大家都認為這個方案可以提高工作效率，但是還需要更多的時間來準備。經理說下個月開始實施，每個人都要按照時間表完成自己的任務。如果有問題，可以隨時聯繫我們的技術部門。";
    const JA: &str = "今日は会議で新しいプロジェクトの計画について話し合いました。みんなはこの案で仕事の効率が上がると考えていますが、準備にはもう少し時間が必要です。部長は来月から始めると言いました。";
    const KO: &str = "오늘 회의에서 새로운 프로젝트 계획에 대해 이야기했습니다. 모두 이 방안이 업무 효율을 높일 수 있다고 생각하지만, 준비하는 데 시간이 더 필요합니다. 부장님은 다음 달부터 시작한다고 말했습니다.";
    #[test]
    fn scored_legacy_detection_corpus() {
        for (e, text) in [
            (Encoding::Gbk, ZH_HANS),
            (Encoding::Big5, ZH_HANT),
            (Encoding::ShiftJis, JA),
            (Encoding::EucJp, JA),
            (Encoding::EucKr, KO),
        ] {
            let raw = Encoder::new(e, false).encode_text(text).unwrap();
            let d = detect(&raw);
            assert_eq!((d.encoding, d.confidence), (e, Confidence::LegacySample), "{e:?}");
            assert_eq!(
                (d.candidates, d.bom, d.binary_warning),
                ([None; 3], false, false),
                "{e:?}"
            );
            assert_eq!(decode(e, &raw, 7).text, text);
            // The 64 KiB sample cut splits a double-byte character; the partial
            // trailing unit neither invalidates the candidate nor decides it.
            let mut long = vec![];
            while long.len() < 60000 {
                long.extend_from_slice(&raw);
            }
            long.resize(65535, b' ');
            let first = text.chars().find(|c| !c.is_ascii()).unwrap().to_string();
            let split = Encoder::new(e, false).encode_text(&first).unwrap();
            assert_eq!(split.len(), 2);
            long.extend(split);
            assert_eq!(detect(&long).encoding, e, "{e:?} truncated");
        }
        // Too little evidence keeps the default and names the likeliest candidate.
        let d = detect(&Encoder::new(Encoding::Gbk, false).encode_text("你好世界").unwrap());
        assert_eq!(
            (d.encoding, d.confidence),
            (Encoding::Windows1252, Confidence::LegacyFallback)
        );
        assert_eq!(d.candidates, [Some(Encoding::Gbk), None, None]);
        // Western text that no CJK candidate explains offers no hint.
        let western = "Größe der Straße: naïve café résumé, déjà vu, señor. Übung macht den Meister; Ärger über Öl. Façade, crème brûlée, jalapeño.";
        let d = detect(&Encoder::new(Encoding::Windows1252, false).encode_text(western).unwrap());
        assert_eq!(
            (d.encoding, d.confidence, d.candidates),
            (Encoding::Windows1252, Confidence::LegacyFallback, [None; 3])
        );
        let d = detect(b"plain ASCII text\r\nsecond line\r\n");
        assert_eq!((d.encoding, d.confidence), (Encoding::Utf8, Confidence::Utf8Sample));
        assert!(!d.binary_warning);
    }
    #[test]
    fn bomless_utf16_and_utf16_bom_before_nul() {
        let text = "Plain text with some ümlauts, 中文 and 😀 in it.\r\nA second line follows here.\r\n";
        for e in [Encoding::Utf16Le, Encoding::Utf16Be] {
            let raw = Encoder::new(e, false).encode_text(text).unwrap();
            let d = detect(&raw);
            assert_eq!(
                (d.encoding, d.confidence, d.bom, d.binary_warning),
                (e, Confidence::Utf16Sample, false, false),
                "{e:?}"
            );
            assert_eq!(decode(e, &raw, 3).text, text);
            // An odd trailing byte, as in a cut sample, stays UTF-16.
            assert_eq!(detect(&raw[..raw.len() - 1]).encoding, e);
        }
        // FF FE 00 00 41 00 42 00 is UTF-16LE "\0AB": 0x00420041 is no UTF-32 scalar.
        let mut raw = Encoding::Utf16Le.bom().to_vec();
        raw.extend(Encoder::new(Encoding::Utf16Le, false).encode_text("\0AB").unwrap());
        let d = detect(&raw);
        assert_eq!((d.encoding, d.bom), (Encoding::Utf16Le, true));
        assert_eq!(decode(Encoding::Utf16Le, &raw, 1).text, "\0AB");
        // A length that is no multiple of four cannot be UTF-32.
        assert_eq!(detect(&raw[..6]).encoding, Encoding::Utf16Le);
        let mut utf32 = Encoding::Utf32Le.bom().to_vec();
        utf32.extend(Encoder::new(Encoding::Utf32Le, false).encode_text("\0AB").unwrap());
        assert_eq!(detect(&utf32).encoding, Encoding::Utf32Le);
        // BOM-less UTF-32 and little-endian 16-bit binary counters are not UTF-16.
        let d = detect(
            &Encoder::new(Encoding::Utf32Le, false)
                .encode_text("abc def ghi")
                .unwrap(),
        );
        assert!(!matches!(d.encoding, Encoding::Utf16Le | Encoding::Utf16Be));
        assert!(d.binary_warning);
        let counters: Vec<u8> = (0u16..200).flat_map(u16::to_le_bytes).collect();
        let d = detect(&counters);
        assert!(!matches!(d.encoding, Encoding::Utf16Le | Encoding::Utf16Be));
        assert!(d.binary_warning);
        // Plain ASCII has no NUL pattern.
        assert_eq!(detect(b"abcd").encoding, Encoding::Utf8);
    }
    /// The decoding every bulk span must agree with: each unit read on its own.
    fn per_unit(e: Encoding, raw: &[u8]) -> (String, Vec<(u64, Vec<u8>)>) {
        assert!(e.bom().is_empty() || !raw.starts_with(e.bom()));
        let (mut text, mut invalid, mut at) = (String::new(), Vec::new(), 0);
        let mut buffer = [0u8; 16];
        while at < raw.len() {
            let window = &raw[at..raw.len().min(at + 4)];
            let end = at + window.len() == raw.len();
            let (n, len, bad) = (1..=window.len())
                .find_map(|k| unit(e, &window[..k], end && k == window.len(), &mut buffer))
                .unwrap();
            text.push_str(std::str::from_utf8(&buffer[..len]).unwrap());
            if bad {
                invalid.push((at as u64, raw[at..at + n].to_vec()));
            }
            at += n;
        }
        (text, invalid)
    }
    fn mixed_bytes(e: Encoding, len: usize) -> Vec<u8> {
        let sample = match e {
            Encoding::ShiftJis | Encoding::EucJp => "日本語のテキスト abc\r\n",
            Encoding::Gbk | Encoding::Big5 => "中文文本 abc\r\n",
            Encoding::EucKr => "한국어 텍스트 abc\r\n",
            Encoding::Latin1 => "Größe naïve café\r\n",
            Encoding::Windows1251 => "Привет мир abc\r\n",
            Encoding::Windows1253 => "Γειά σου abc\r\n",
            Encoding::Windows1252 | Encoding::Windows1254 => "Größe naïve café €\r\n",
            Encoding::Windows1250 => "Příliš žluťoučký kůň €\r\n",
            Encoding::Windows1257 => "Ąžuolas ēdiens €\r\n",
            Encoding::Windows1255 => "שלום abc\r\n",
            Encoding::Windows1256 => "مرحبا abc\r\n",
            Encoding::Windows1258 => "Viêt Nam abc\r\n",
            _ => "aé中😀\r\n",
        };
        let unit = encoded_any(e, &[sample, "abc\r\n"]);
        let mut raw = Vec::new();
        let mut seed = 7u32;
        while raw.len() < len {
            raw.extend_from_slice(&unit);
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            // Sprinkle arbitrary bytes: invalid, undefined or noncanonical units.
            if seed % 5 == 0 {
                raw.push((seed >> 16) as u8 | 0x80);
            }
        }
        raw
    }
    fn encoded_any(e: Encoding, texts: &[&str]) -> Vec<u8> {
        texts
            .iter()
            .find_map(|text| Encoder::new(e, false).encode_text(text).ok())
            .unwrap()
    }
    const ALL: [Encoding; 20] = [
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Utf32Le,
        Encoding::Utf32Be,
        Encoding::Latin1,
        Encoding::Windows1250,
        Encoding::Windows1251,
        Encoding::Windows1252,
        Encoding::Windows1253,
        Encoding::Windows1254,
        Encoding::Windows1255,
        Encoding::Windows1256,
        Encoding::Windows1257,
        Encoding::Windows1258,
        Encoding::ShiftJis,
        Encoding::Gbk,
        Encoding::Big5,
        Encoding::EucJp,
        Encoding::EucKr,
    ];
    #[test]
    fn bulk_decoding_matches_unit_decoding_across_chunk_and_span_splits() {
        for e in ALL {
            let raw = mixed_bytes(e, 3 * SPAN_RAW + 777);
            let (text, invalid) = per_unit(e, &raw);
            for chunk in [1, 2, 3, 5, 7, 4093, SPAN_RAW - 1, SPAN_RAW + 1, 65536, raw.len()] {
                let s = decode(e, &raw, chunk);
                assert_eq!(s.text, text, "{e:?} chunk {chunk}");
                assert_eq!(s.invalid, invalid, "{e:?} chunk {chunk}");
                assert_eq!(s.ranges.last().unwrap().1, raw.len() as u64);
            }
        }
    }
    #[test]
    fn valid_text_decodes_in_bulk_spans_not_per_scalar() {
        for e in ALL {
            let unit = encoded_any(e, &["aé€中文 日本 한국 abc\r\n", "aé abc\r\n", "abc\r\n"]);
            let raw: Vec<u8> = unit.iter().copied().cycle().take(1024 * 1024).collect();
            let mut d = Decoder::new(e);
            let mut s = Sink::default();
            for part in raw.chunks(65536) {
                assert_eq!(d.push(part, false, &mut s).unwrap().consumed, part.len());
            }
            d.push(&[], true, &mut s).unwrap();
            // Only units cut by a push boundary, and the one unit cut at the end
            // of the sample, are read one at a time.
            assert!(d.unit_reads() <= 2 * 16 + 4, "{e:?}: {} unit reads", d.unit_reads());
            let spans = s.ranges.len() as u64;
            // Per-scalar spans would number in the hundreds of thousands.
            let bound = if e == Encoding::Utf8 {
                3 * 16
            } else {
                2 * (raw.len() / SPAN_RAW) as u64 + 3 * 16
            };
            assert!(spans <= bound, "{e:?}: {spans} spans");
            assert!(s.invalid.len() <= 1, "{e:?}");
        }
    }
    #[test]
    fn bulk_attempts_after_invalid_units_cost_only_their_decoded_prefix() {
        // FIO-09: after each invalid unit the next bulk attempt walks only the units
        // it decodes, not the rest of its span, so invalid-dense input stays linear.
        for e in [
            Encoding::ShiftJis,
            Encoding::Gbk,
            Encoding::Big5,
            Encoding::EucJp,
            Encoding::EucKr,
        ] {
            let raw: Vec<u8> = b"a\xff".iter().copied().cycle().take(4 * SPAN_RAW).collect();
            let (text, invalid) = per_unit(e, &raw);
            WIDTH_STEPS.with(|steps| steps.set(0));
            let s = decode(e, &raw, raw.len());
            let steps = WIDTH_STEPS.with(std::cell::Cell::get);
            // A walk over each attempt's whole span would take about SPAN_RAW / 2
            // steps for each of the raw.len() / 2 invalid units.
            assert!(steps <= raw.len() as u64, "{e:?}: {steps} width steps");
            assert_eq!(s.text, text, "{e:?}");
            assert_eq!(s.invalid, invalid, "{e:?}");
        }
    }
    #[test]
    fn gb18030_four_byte_units_are_one_scalar_in_every_split() {
        let raw = [b'a', 0x81, 0x30, 0x81, 0x30, b'b', 0x81, 0x30, b'c'];
        for n in 1..=raw.len() {
            let s = decode(Encoding::Gbk, &raw, n);
            assert_eq!(s.text, "a\u{80}b\u{fffd}0c", "chunk {n}");
            assert_eq!(s.invalid, vec![(6, vec![0x81])], "chunk {n}");
        }
        assert_eq!(per_unit(Encoding::Gbk, &raw).0, "a\u{80}b\u{fffd}0c");
    }
    #[test]
    fn mixed_run_boundaries_walk_units_and_refuse_disagreement() {
        // "aé€b" in Windows-1252: text widths 1, 2, 3, 1 over one byte each.
        let raw = [0x61, 0xe9, 0x80, 0x62];
        let e = Encoding::Windows1252;
        assert_eq!(run_boundaries(e, &raw, 7, 0, 7).unwrap(), Some((0..7, 0..4)));
        assert_eq!(run_boundaries(e, &raw, 7, 2, 7).unwrap(), Some((3..7, 2..4)));
        assert_eq!(run_boundaries(e, &raw, 7, 0, 5).unwrap(), Some((0..3, 0..2)));
        assert_eq!(run_boundaries(e, &raw, 7, 2, 2).unwrap(), None);
        assert!(run_boundaries(e, &raw, 8, 0, 7).is_err());
        // Big5 0x88 0x62 is one unit of two scalars: no boundary between them.
        let big5 = [b'a', 0x88, 0x62, b'z'];
        assert_eq!(
            run_boundaries(Encoding::Big5, &big5, 6, 2, 6).unwrap(),
            Some((5..6, 3..4))
        );
        assert!(run_boundaries(Encoding::Utf16Le, &[0, 0xd8, 0x41, 0], 3, 0, 3).is_err());
        assert_eq!(uniform_units(Encoding::Utf8, "aé", 3), Some((1, 1)));
        assert_eq!(uniform_units(Encoding::Utf16Be, "ab", 4), Some((1, 2)));
        assert_eq!(uniform_units(Encoding::Windows1252, "aé", 2), None);
    }
    #[test]
    fn decoder_backpressure_retains_token() {
        struct Full;
        impl DecodedSink for Full {
            fn remaining_capacity(&self) -> usize {
                0
            }
            fn write(&mut self, _: DecodedSpan<'_>) -> Result<(), CodecError> {
                panic!("full")
            }
        }
        let mut d = Decoder::new(Encoding::Utf8);
        let p = d.push(b"a", true, &mut Full).unwrap();
        assert!(p.needs_output);
        assert_eq!(p.consumed, 1);
        let mut s = Sink::default();
        d.push(&[], true, &mut s).unwrap();
        assert_eq!(s.text, "a");
    }

    #[test]
    fn encoder_backpressure_refusal_and_output_errors() {
        struct Bytes {
            bytes: Vec<u8>,
            capacity: usize,
            fail: bool,
        }
        impl ByteSink for Bytes {
            fn remaining_capacity(&self) -> usize {
                self.capacity
            }
            fn write(&mut self, b: &[u8]) -> Result<(), CodecError> {
                if self.fail {
                    return Err(CodecError::Output(std::io::Error::other("injected disk full")));
                }
                self.bytes.extend_from_slice(b);
                self.capacity -= b.len();
                Ok(())
            }
        }
        let spans = [DecodedSpan {
            text: "A😀B",
            original: RawOffset(0)..RawOffset(0),
            opaque_bytes: None,
        }];
        let mut e = Encoder::new(Encoding::Utf16Le, true);
        let mut out = Bytes {
            bytes: vec![],
            capacity: 4,
            fail: false,
        };
        let p = e.push(&spans, true, &mut out).unwrap();
        assert_eq!(p.consumed, 0);
        assert!(p.needs_output);
        out.capacity = 4;
        let p = e.push(&spans, true, &mut out).unwrap();
        assert!(p.needs_output);
        out.capacity = 2;
        let p = e.push(&spans, true, &mut out).unwrap();
        assert_eq!(p.consumed, 1);
        assert!(!p.needs_output);
        assert_eq!(decode(Encoding::Utf16Le, &out.bytes, 1).text, "A😀B");
        out.capacity = 100;
        out.fail = true;
        assert!(matches!(
            Encoder::new(Encoding::Utf8, false).push(&spans, true, &mut out),
            Err(CodecError::Output(_))
        ));
        let opaque = [DecodedSpan {
            text: "�",
            original: RawOffset(0)..RawOffset(1),
            opaque_bytes: Some(&[255]),
        }];
        assert!(matches!(
            Encoder::new(Encoding::Utf8, false).push(&opaque, true, &mut out),
            Err(CodecError::UnresolvedOpaqueBytes)
        ));
        assert_eq!(Encoding::from_label("ISO-8859-1"), Some(Encoding::Latin1));
        assert_eq!(Encoding::from_label("shift_jis"), Some(Encoding::ShiftJis));
        assert_eq!(Encoding::from_label("iso-2022-jp"), None);
    }
}
