// SPDX-License-Identifier: MPL-2.0
//! Stateless encoding catalog and bounded streaming adapters. Original source ranges
//! must be copied on unchanged same-encoding saves; encoding is not a bijection.
pub mod disk;
pub mod failure;
mod oem;
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
    // Catalog v2 (BIZ-09): appended so serialized variant order stays stable.
    Iso8859_2,
    Iso8859_3,
    Iso8859_4,
    Iso8859_5,
    Iso8859_6,
    Iso8859_7,
    Iso8859_8,
    Iso8859_10,
    Iso8859_13,
    Iso8859_14,
    Iso8859_15,
    Iso8859_16,
    Koi8R,
    Koi8U,
    Windows874,
    MacRoman,
    MacCyrillic,
    Cp437,
    Cp850,
    Cp852,
    Cp866,
}
impl Encoding {
    /// Every catalog entry, in catalog order. Detection only ever chooses the
    /// Unicode encodings, Windows-1252 and the scored CJK candidates.
    pub const ALL: &[Encoding] = &[
        Self::Utf8,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Utf32Le,
        Self::Utf32Be,
        Self::Latin1,
        Self::Windows1250,
        Self::Windows1251,
        Self::Windows1252,
        Self::Windows1253,
        Self::Windows1254,
        Self::Windows1255,
        Self::Windows1256,
        Self::Windows1257,
        Self::Windows1258,
        Self::ShiftJis,
        Self::Gbk,
        Self::Big5,
        Self::EucJp,
        Self::EucKr,
        Self::Iso8859_2,
        Self::Iso8859_3,
        Self::Iso8859_4,
        Self::Iso8859_5,
        Self::Iso8859_6,
        Self::Iso8859_7,
        Self::Iso8859_8,
        Self::Iso8859_10,
        Self::Iso8859_13,
        Self::Iso8859_14,
        Self::Iso8859_15,
        Self::Iso8859_16,
        Self::Koi8R,
        Self::Koi8U,
        Self::Windows874,
        Self::MacRoman,
        Self::MacCyrillic,
        Self::Cp437,
        Self::Cp850,
        Self::Cp852,
        Self::Cp866,
    ];
    /// Catalog version 2. ISO-8859-1 aliases deliberately bypass encoding_rs.
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
            "iso-8859-2" | "iso8859-2" | "latin2" => Self::Iso8859_2,
            "iso-8859-3" | "iso8859-3" | "latin3" => Self::Iso8859_3,
            "iso-8859-4" | "iso8859-4" | "latin4" => Self::Iso8859_4,
            "iso-8859-5" | "iso8859-5" => Self::Iso8859_5,
            "iso-8859-6" | "iso8859-6" => Self::Iso8859_6,
            "iso-8859-7" | "iso8859-7" => Self::Iso8859_7,
            "iso-8859-8" | "iso8859-8" => Self::Iso8859_8,
            "iso-8859-10" | "iso8859-10" | "latin6" => Self::Iso8859_10,
            "iso-8859-13" | "iso8859-13" | "latin7" => Self::Iso8859_13,
            "iso-8859-14" | "iso8859-14" | "latin8" => Self::Iso8859_14,
            "iso-8859-15" | "iso8859-15" | "latin9" => Self::Iso8859_15,
            "iso-8859-16" | "iso8859-16" | "latin10" => Self::Iso8859_16,
            "koi8-r" | "koi8r" => Self::Koi8R,
            "koi8-u" | "koi8u" => Self::Koi8U,
            "windows-874" | "cp874" => Self::Windows874,
            "macintosh" | "x-mac-roman" | "mac-roman" => Self::MacRoman,
            "x-mac-cyrillic" | "mac-cyrillic" => Self::MacCyrillic,
            "ibm437" | "cp437" => Self::Cp437,
            "ibm850" | "cp850" => Self::Cp850,
            "ibm852" | "cp852" => Self::Cp852,
            "ibm866" | "cp866" => Self::Cp866,
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
            Self::Iso8859_2 => "ISO-8859-2 (Central European)",
            Self::Iso8859_3 => "ISO-8859-3 (South European)",
            Self::Iso8859_4 => "ISO-8859-4 (Baltic)",
            Self::Iso8859_5 => "ISO-8859-5 (Cyrillic)",
            Self::Iso8859_6 => "ISO-8859-6 (Arabic)",
            Self::Iso8859_7 => "ISO-8859-7 (Greek)",
            Self::Iso8859_8 => "ISO-8859-8 (Hebrew)",
            Self::Iso8859_10 => "ISO-8859-10 (Nordic)",
            Self::Iso8859_13 => "ISO-8859-13 (Baltic)",
            Self::Iso8859_14 => "ISO-8859-14 (Celtic)",
            Self::Iso8859_15 => "ISO-8859-15 (Western / Euro)",
            Self::Iso8859_16 => "ISO-8859-16 (South-Eastern European)",
            Self::Koi8R => "KOI8-R (Russian)",
            Self::Koi8U => "KOI8-U (Ukrainian)",
            Self::Windows874 => "Windows-874 (Thai)",
            Self::MacRoman => "Mac Roman (Western)",
            Self::MacCyrillic => "Mac Cyrillic",
            Self::Cp437 => "OEM 437 (US)",
            Self::Cp850 => "OEM 850 (Western European)",
            Self::Cp852 => "OEM 852 (Central European)",
            Self::Cp866 => "OEM 866 (Cyrillic)",
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
            Self::Iso8859_2 => ISO_8859_2,
            Self::Iso8859_3 => ISO_8859_3,
            Self::Iso8859_4 => ISO_8859_4,
            Self::Iso8859_5 => ISO_8859_5,
            Self::Iso8859_6 => ISO_8859_6,
            Self::Iso8859_7 => ISO_8859_7,
            Self::Iso8859_8 => ISO_8859_8,
            Self::Iso8859_10 => ISO_8859_10,
            Self::Iso8859_13 => ISO_8859_13,
            Self::Iso8859_14 => ISO_8859_14,
            Self::Iso8859_15 => ISO_8859_15,
            Self::Iso8859_16 => ISO_8859_16,
            Self::Koi8R => KOI8_R,
            Self::Koi8U => KOI8_U,
            Self::Windows874 => WINDOWS_874,
            Self::MacRoman => MACINTOSH,
            Self::MacCyrillic => X_MAC_CYRILLIC,
            Self::Cp866 => IBM866,
            // Latin1, Unicode and the `oem` tables bypass encoding_rs.
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

#[derive(Clone)]
pub struct Decoder {
    encoding: Encoding,
    pending: Vec<u8>,
    offset: u64,
    start: bool,
}
impl Decoder {
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            pending: Vec::with_capacity(4),
            offset: 0,
            start: true,
        }
    }
    // None means one more byte is needed. Invalid units retain their exact bytes.
    fn unit(&self, end: bool) -> Option<(usize, String, bool)> {
        let p = &self.pending;
        if p.is_empty() {
            return None;
        }
        let invalid = |n| Some((n, "\u{fffd}".to_owned(), true));
        let scalar = |n, c: Option<char>| match c {
            Some(c) => Some((n, c.to_string(), false)),
            None => invalid(n),
        };
        match self.encoding {
            Encoding::Utf8 => match std::str::from_utf8(p) {
                Ok(s) => {
                    let c = s.chars().next().unwrap();
                    scalar(c.len_utf8(), Some(c))
                }
                Err(e) if e.valid_up_to() > 0 => {
                    let s = std::str::from_utf8(&p[..e.valid_up_to()]).unwrap();
                    let c = s.chars().next().unwrap();
                    scalar(c.len_utf8(), Some(c))
                }
                Err(e) => match e.error_len() {
                    Some(n) => invalid(n),
                    None if end => invalid(p.len()),
                    None => None,
                },
            },
            Encoding::Latin1 => scalar(1, Some(char::from(p[0]))),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                if p.len() < 2 {
                    return if end { invalid(p.len()) } else { None };
                }
                let word = |b: &[u8]| {
                    if self.encoding == Encoding::Utf16Le {
                        u16::from_le_bytes([b[0], b[1]])
                    } else {
                        u16::from_be_bytes([b[0], b[1]])
                    }
                };
                let w = word(p);
                if (0xd800..=0xdbff).contains(&w) {
                    if p.len() < 4 {
                        return if end { invalid(2) } else { None };
                    }
                    let low = word(&p[2..]);
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return invalid(2);
                    }
                    scalar(
                        4,
                        char::from_u32(0x10000 + ((w as u32 - 0xd800) << 10) + low as u32 - 0xdc00),
                    )
                } else {
                    scalar(2, char::from_u32(w as u32))
                }
            }
            Encoding::Utf32Le | Encoding::Utf32Be => {
                if p.len() < 4 {
                    return if end { invalid(p.len()) } else { None };
                }
                let b = [p[0], p[1], p[2], p[3]];
                scalar(
                    4,
                    char::from_u32(if self.encoding == Encoding::Utf32Le {
                        u32::from_le_bytes(b)
                    } else {
                        u32::from_be_bytes(b)
                    }),
                )
            }
            e => {
                if let Some(table) = oem::table(e) {
                    return scalar(1, Some(oem::decode(table, p[0])));
                }
                let width = match e {
                    Encoding::ShiftJis if matches!(p[0],0x81..=0x9f|0xe0..=0xfc) => 2,
                    Encoding::Gbk | Encoding::Big5 | Encoding::EucKr if p[0] >= 0x81 => 2,
                    Encoding::EucJp if p[0] == 0x8f => 3,
                    Encoding::EucJp if p[0] >= 0x80 => 2,
                    _ => 1,
                };
                if p.len() < width && !end {
                    return None;
                }
                let n = width.min(p.len());
                match e
                    .legacy()
                    .unwrap()
                    .decode_without_bom_handling_and_without_replacement(&p[..n])
                {
                    Some(s) => Some((n, s.into_owned(), false)),
                    None => invalid(1),
                }
            }
        }
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
            if let Some((n, text, invalid)) = self.unit(final_unit) {
                if out.remaining_capacity() < text.len() {
                    return Ok(Progress {
                        consumed,
                        produced,
                        needs_output: true,
                    });
                }
                out.write(DecodedSpan {
                    text: &text,
                    original: RawOffset(self.offset)..RawOffset(self.offset + n as u64),
                    opaque_bytes: invalid.then_some(&self.pending[..n]),
                })?;
                self.pending.drain(..n);
                self.offset += n as u64;
                produced += text.len();
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
                    if let Some(table) = oem::table(e) {
                        result.push(oem::encode(table, c).ok_or(CodecError::Unrepresentable)?);
                        continue;
                    }
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
    /// Known text in each catalog v2 entry (BIZ-09). Bytes come from the WHATWG
    /// indexes (encoding_rs) and the Unicode vendor mappings (OEM 437/850/852).
    const KNOWN: &[(Encoding, &str, &[u8])] = &[
        (Encoding::Iso8859_2, "Łódź", &[0xa3, 0xf3, 0x64, 0xbc]),
        (Encoding::Iso8859_3, "ĉĝĥĵŝŭ", &[0xe6, 0xf8, 0xb6, 0xbc, 0xfe, 0xfd]),
        (Encoding::Iso8859_4, "āēķ", &[0xe0, 0xba, 0xf3]),
        (Encoding::Iso8859_5, "Привет", &[0xbf, 0xe0, 0xd8, 0xd2, 0xd5, 0xe2]),
        (Encoding::Iso8859_6, "مرحبا", &[0xe5, 0xd1, 0xcd, 0xc8, 0xc7]),
        (Encoding::Iso8859_7, "Ελλάδα", &[0xc5, 0xeb, 0xeb, 0xdc, 0xe4, 0xe1]),
        (Encoding::Iso8859_8, "שלום", &[0xf9, 0xec, 0xe5, 0xed]),
        (Encoding::Iso8859_10, "ŋđ", &[0xbf, 0xb9]),
        (Encoding::Iso8859_13, "ąčę", &[0xe0, 0xe8, 0xe6]),
        (Encoding::Iso8859_14, "ŵŷ", &[0xf0, 0xfe]),
        (Encoding::Iso8859_15, "€œŸ", &[0xa4, 0xbd, 0xbe]),
        (Encoding::Iso8859_16, "șț", &[0xba, 0xfe]),
        (Encoding::Koi8R, "Привет", &[0xf0, 0xd2, 0xc9, 0xd7, 0xc5, 0xd4]),
        (Encoding::Koi8U, "Київ", &[0xeb, 0xc9, 0xa7, 0xd7]),
        (Encoding::Windows874, "ไทย", &[0xe4, 0xb7, 0xc2]),
        (Encoding::MacRoman, "café", &[0x63, 0x61, 0x66, 0x8e]),
        (Encoding::MacCyrillic, "Привет", &[0x8f, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2]),
        (Encoding::Cp437, "╔═╗Ç", &[0xc9, 0xcd, 0xbb, 0x80]),
        (Encoding::Cp850, "Øß", &[0x9d, 0xe1]),
        (Encoding::Cp852, "Łódź", &[0x9d, 0xa2, 0x64, 0xab]),
        (Encoding::Cp866, "Привет", &[0x8f, 0xe0, 0xa8, 0xa2, 0xa5, 0xe2]),
    ];
    #[test]
    fn catalog_v2_known_text_encodes_and_decodes() {
        for &(e, text, bytes) in KNOWN {
            assert_eq!(Encoder::new(e, false).encode_text(text).unwrap(), bytes, "{e:?}");
            for n in 1..=3 {
                let s = decode(e, bytes, n);
                assert_eq!((s.text.as_str(), s.invalid.len()), (text, 0), "{e:?}");
            }
            // Scalars outside the table are refused, never substituted.
            assert!(
                matches!(
                    Encoder::new(e, false).encode_text("中"),
                    Err(CodecError::Unrepresentable)
                ),
                "{e:?}"
            );
        }
        for (label, e) in [
            ("ISO_8859_15", Encoding::Iso8859_15),
            ("latin2", Encoding::Iso8859_2),
            ("KOI8-R", Encoding::Koi8R),
            ("koi8u", Encoding::Koi8U),
            ("windows-874", Encoding::Windows874),
            ("macintosh", Encoding::MacRoman),
            ("x-mac-cyrillic", Encoding::MacCyrillic),
            ("cp437", Encoding::Cp437),
            ("IBM850", Encoding::Cp850),
            ("cp852", Encoding::Cp852),
            ("ibm866", Encoding::Cp866),
        ] {
            assert_eq!(Encoding::from_label(label), Some(e), "{label}");
        }
        let names: std::collections::BTreeSet<_> = Encoding::ALL.iter().map(|e| e.display_name()).collect();
        assert_eq!(names.len(), Encoding::ALL.len(), "one distinct name per catalog entry");
    }
    /// Every catalog entry except Unicode and the CJK multi-byte encodings.
    fn single_byte(e: Encoding) -> bool {
        !matches!(
            e,
            Encoding::Utf8
                | Encoding::Utf16Le
                | Encoding::Utf16Be
                | Encoding::Utf32Le
                | Encoding::Utf32Be
                | Encoding::ShiftJis
                | Encoding::Gbk
                | Encoding::Big5
                | Encoding::EucJp
                | Encoding::EucKr
        )
    }
    #[test]
    fn single_byte_tables_round_trip_every_byte_exactly() {
        let tables: Vec<_> = Encoding::ALL.iter().copied().filter(|&e| single_byte(e)).collect();
        assert_eq!(tables.len(), 31);
        let all: Vec<u8> = (0..=255).collect();
        for e in tables {
            let (mut text, mut unassigned) = (String::new(), 0);
            for byte in 0..=255u8 {
                let s = decode(e, &[byte], 1);
                assert_eq!(s.ranges, [(0, 1)], "{e:?} {byte:#04x}");
                if s.invalid.is_empty() {
                    assert_eq!(s.text.chars().count(), 1, "{e:?} {byte:#04x}");
                    assert_eq!(
                        Encoder::new(e, false).encode_text(&s.text).unwrap(),
                        [byte],
                        "{e:?} {byte:#04x}"
                    );
                } else {
                    // An unassigned byte keeps its exact provenance as an opaque span.
                    assert_eq!((s.text.as_str(), s.invalid), ("\u{fffd}", vec![(0, vec![byte])]));
                    unassigned += 1;
                }
                text.push_str(&s.text);
            }
            for n in [1, 5, 256] {
                let s = decode(e, &all, n);
                assert_eq!(s.text, text, "{e:?}");
                assert_eq!(s.ranges.len(), 256, "{e:?}");
            }
            if matches!(
                e,
                Encoding::Latin1 | Encoding::Cp437 | Encoding::Cp850 | Encoding::Cp852 | Encoding::Cp866
            ) {
                assert_eq!(unassigned, 0, "{e:?} maps every byte");
            }
        }
    }
    #[test]
    fn catalog_v2_entries_are_never_detected() {
        let detectable = [
            Encoding::Utf8,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
            Encoding::Windows1252,
        ];
        let mut samples = vec![(0..=255).collect::<Vec<u8>>(), (0x80..=0xff).collect()];
        for &(e, text, _) in KNOWN {
            samples.push(Encoder::new(e, false).encode_text(&text.repeat(40)).unwrap());
        }
        for raw in samples {
            let d = detect(&raw);
            assert!(
                detectable.contains(&d.encoding) || LEGACY_CANDIDATES.contains(&d.encoding),
                "{:?} detected from {raw:02x?}",
                d.encoding
            );
            assert!(
                d.candidates.iter().flatten().all(|e| LEGACY_CANDIDATES.contains(e)),
                "{:?}",
                d.candidates
            );
        }
    }
}
