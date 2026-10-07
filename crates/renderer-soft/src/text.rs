// SPDX-License-Identifier: MPL-2.0
//! Font discovery, shaping and the geometry queries behind `TextBackend`.
//!
//! Text is shaped once, in DIPs, into a [`Shaped`] value that keeps the glyphs
//! to paint and a cluster map in visual order for hit tests, carets and range
//! rectangles. Nothing here depends on the output scale: the canvas scales the
//! glyphs when it rasterizes them, so a layout survives resizes and DPI changes
//! unchanged, as a DirectWrite layout does.
use bareline_renderer::{Color, LayoutError, Point, Rect, TextHit, TextStyle};
use cosmic_text::{
    Align, Attrs, AttrsList, AttrsOwned, BufferLine, CacheKey, Ellipsize, Family, FontSystem, Hinting, LayoutGlyph,
    LineEnding, Shaping, SwashCache, SwashImage, Wrap, fontdb,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

/// DejaVu Sans Mono 2.37 (Bitstream Vera and Arev licences with public-domain
/// DejaVu changes; see `fonts/LICENSE-DejaVu.txt`). Always loaded, so every family
/// resolves to a real face and offscreen goldens are reproducible.
const BUNDLED_FONT: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");
/// Family name of the bundled face.
pub const BUNDLED_FONT_FAMILY: &str = "DejaVu Sans Mono";
/// Editor families tried, in order, when the requested family is not installed
/// (the counterpart of the Windows renderer's monospace fallbacks, UI-10). The
/// bundled face ends the list, so a monospace face is always found.
const MONOSPACE_FAMILIES: [&str; 5] = ["Cascadia Mono", "Consolas", "Menlo", "SF Mono", BUNDLED_FONT_FAMILY];
/// Interface families for text below 16 px, as Segoe UI is on Windows.
const INTERFACE_FAMILIES: [&str; 8] = [
    "Segoe UI",
    "Helvetica Neue",
    "Ubuntu",
    "Cantarell",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Arial",
];
/// Colour emoji families, the first installed of which draws emoji sequences
/// (the counterpart of Segoe UI Emoji in DirectWrite's system fallback).
const COLOR_EMOJI_FAMILIES: [&str; 6] = [
    "Noto Color Emoji",
    "Apple Color Emoji",
    "Segoe UI Emoji",
    "Twemoji",
    "JoyPixels",
    "OpenMoji Color",
];
/// LEFT-TO-RIGHT MARK, put at the start of each bidi paragraph of a shaping
/// copy ([`ShapingCopy`]) so its base direction is left-to-right whatever its
/// first strong character is.
const LTR_MARK: &str = "\u{200E}";
/// Resolved family names; the cache is flushed when it overflows.
const MAX_RESOLVED_FAMILIES: usize = 64;
/// Glyph bitmaps kept between frames. Past this, a frame keeps only the
/// bitmaps it drew, so a screen that needs more than this (dense CJK text at
/// several sizes) still rasterizes each glyph once, not once per frame.
const MAX_GLYPH_IMAGES: usize = 8192;
/// Largest glyph rasterized, in pixels per em. Bigger glyphs are not drawn:
/// their bitmaps would be hundreds of megabytes (text sizes are capped at
/// [`MAX_TEXT_SIZE`] DIPs, but a window's scale factor is not).
const MAX_GLYPH_PIXELS: f32 = 4096.0;
/// Largest text size accepted, in DIPs, by shaping, measuring and `Text`
/// operations; larger sizes are `LayoutError::ResourceLimit` or
/// `SoftError::InvalidOperations`.
pub(crate) const MAX_TEXT_SIZE: f32 = 2048.0;
/// Tab advance in spaces.
const TAB_WIDTH: u16 = 4;
/// Line height of wrapped layouts, in font sizes: Windows sets uniform spacing.
const WRAPPED_LINE_HEIGHT: f32 = 1.2;

/// Which fonts a renderer may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontSource {
    /// Installed system fonts (through fontdb) plus the bundled fallback face.
    #[default]
    System,
    /// The bundled face only. Output depends on nothing installed on the
    /// machine, which golden tests require.
    BundledOnly,
}

/// The cosmic-text font system and glyph cache, reused across frames.
pub(crate) struct Fonts {
    pub(crate) system: FontSystem,
    swash: SwashCache,
    /// Glyph bitmaps drawn since the last [`Fonts::trim`].
    drawn: HashSet<CacheKey>,
    /// Glyph bitmaps kept between frames ([`MAX_GLYPH_IMAGES`]; tests lower it).
    pub(crate) glyph_cap: usize,
    /// Requested family name to the installed family used for it.
    resolved: HashMap<String, String>,
    /// Natural line height in font sizes, per installed family.
    line_heights: HashMap<String, f32>,
    monospace: String,
    interface: String,
    /// The installed colour emoji family, if any.
    emoji: Option<String>,
    /// Paragraphs shaped since creation; tests use it to prove caching.
    #[cfg(test)]
    pub(crate) shaped_paragraphs: usize,
    /// Emoji parts shaped on their own since creation ([`shape_parts`]).
    #[cfg(test)]
    shaped_parts: usize,
}

impl Fonts {
    pub(crate) fn new(source: FontSource) -> Self {
        let mut db = fontdb::Database::new();
        if source == FontSource::System {
            db.load_system_fonts();
        }
        db.load_font_source(fontdb::Source::Binary(Arc::new(BUNDLED_FONT)));
        Self::with_database(db)
    }
    /// Fonts drawn from `db`, which must hold the bundled face.
    fn with_database(db: fontdb::Database) -> Self {
        let monospace = first_installed(&db, &MONOSPACE_FAMILIES).unwrap_or_else(|| BUNDLED_FONT_FAMILY.to_owned());
        let interface = first_installed(&db, &INTERFACE_FAMILIES).unwrap_or_else(|| monospace.clone());
        let emoji = first_installed(&db, &COLOR_EMOJI_FAMILIES);
        // A fixed locale keeps fallback order independent of the user's settings.
        let system = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        Self {
            system,
            swash: SwashCache::new(),
            drawn: HashSet::new(),
            glyph_cap: MAX_GLYPH_IMAGES,
            resolved: HashMap::new(),
            line_heights: HashMap::new(),
            monospace,
            interface,
            emoji,
            #[cfg(test)]
            shaped_paragraphs: 0,
            #[cfg(test)]
            shaped_parts: 0,
        }
    }
    /// The installed family that draws `requested`, or the default family for
    /// `size` when none is requested: monospace from 16 px up, interface below,
    /// matching the Windows renderer.
    pub(crate) fn family(&mut self, requested: Option<&str>, size: f32) -> String {
        let Some(requested) = requested else {
            return if size >= 16.0 {
                self.monospace.clone()
            } else {
                self.interface.clone()
            };
        };
        if let Some(resolved) = self.resolved.get(requested) {
            return resolved.clone();
        }
        let resolved = installed(self.system.db(), requested).unwrap_or_else(|| self.monospace.clone());
        if self.resolved.len() >= MAX_RESOLVED_FAMILIES {
            self.resolved.clear();
        }
        self.resolved.insert(requested.to_owned(), resolved.clone());
        resolved
    }
    /// The default editor family, after fallback.
    pub(crate) fn monospace(&self) -> &str {
        &self.monospace
    }
    /// Ascent plus descent plus line gap of `family`'s regular face, in font sizes.
    fn line_height(&mut self, family: &str) -> f32 {
        if let Some(ratio) = self.line_heights.get(family) {
            return *ratio;
        }
        let query = fontdb::Query {
            families: &[Family::Name(family)],
            ..Default::default()
        };
        let ratio = self
            .system
            .db()
            .query(&query)
            .and_then(|id| self.system.get_font(id, fontdb::Weight::NORMAL))
            .map(|font| {
                let metrics = font.metrics();
                (metrics.ascent - metrics.descent + metrics.leading) / f32::from(metrics.units_per_em.max(1))
            })
            .filter(|ratio| ratio.is_finite() && (0.8..=3.0).contains(ratio))
            .unwrap_or(WRAPPED_LINE_HEIGHT);
        if self.line_heights.len() >= MAX_RESOLVED_FAMILIES {
            self.line_heights.clear();
        }
        self.line_heights.insert(family.to_owned(), ratio);
        ratio
    }
    /// The bitmap of a glyph at `key`, rasterized once and cached; `None` for
    /// blank glyphs and for glyphs above [`MAX_GLYPH_PIXELS`].
    pub(crate) fn glyph_image(&mut self, key: CacheKey) -> Option<&SwashImage> {
        if f32::from_bits(key.font_size_bits) > MAX_GLYPH_PIXELS {
            return None;
        }
        self.drawn.insert(key);
        self.swash.get_image(&mut self.system, key).as_ref()
    }
    /// Bound the glyph bitmap cache between frames: once it is over
    /// [`MAX_GLYPH_IMAGES`], drop the bitmaps the last frame did not draw.
    pub(crate) fn trim(&mut self) {
        let cache = &mut self.swash.image_cache;
        if cache.len() > self.glyph_cap {
            cache.retain(|key, _| self.drawn.contains(key));
        }
        self.drawn.clear();
    }
    /// Cached glyph bitmaps.
    #[cfg(test)]
    pub(crate) fn glyph_images(&self) -> usize {
        self.swash.image_cache.len()
    }
}

/// Every family of the system fonts plus the bundled face, sorted by name, with
/// whether its faces are monospaced. Each face counts under its primary
/// (first, usually English) family name only.
pub(crate) fn installed_families() -> Vec<(String, bool)> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db.load_font_source(fontdb::Source::Binary(Arc::new(BUNDLED_FONT)));
    let mut families = std::collections::BTreeMap::<String, bool>::new();
    for face in db.faces() {
        if let Some((family, _)) = face.families.first() {
            *families.entry(family.clone()).or_insert(true) &= face.monospaced;
        }
    }
    families.into_iter().collect()
}

/// The face's own spelling of the first family in `names` that is installed.
fn first_installed(db: &fontdb::Database, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| installed(db, name))
}
fn installed(db: &fontdb::Database, name: &str) -> Option<String> {
    db.faces().find_map(|face| {
        face.families
            .iter()
            .find(|(family, _)| family.eq_ignore_ascii_case(name))
            .map(|(family, _)| family.clone())
    })
}

/// One painted glyph: cosmic-text's positioned glyph, its line's baseline and
/// its first byte in the whole layout text.
pub(crate) struct Glyph {
    pub(crate) layout: LayoutGlyph,
    pub(crate) baseline: f32,
    pub(crate) start: usize,
}
/// A shaping cluster on one visual line: the bytes it covers and its x extent.
#[derive(Clone, Copy, Debug)]
struct Cluster {
    start: usize,
    end: usize,
    left: f32,
    right: f32,
    rtl: bool,
}
/// A visual line; `clusters` indexes `Shaped::clusters` in visual order.
#[derive(Clone, Debug)]
struct Line {
    paragraph: usize,
    top: f32,
    height: f32,
    clusters: Range<usize>,
}

/// Shaped text with its glyphs and cluster geometry, in DIPs from the layout origin.
pub(crate) struct Shaped {
    text: String,
    /// Byte ranges of the paragraphs, line endings excluded.
    paragraphs: Vec<Range<usize>>,
    lines: Vec<Line>,
    clusters: Vec<Cluster>,
    glyphs: Vec<Glyph>,
    width: f32,
    height: f32,
    /// Sorted, nonoverlapping foreground colours by byte range.
    styles: Vec<(Range<usize>, Color)>,
}

/// Split `text` at CR, LF and CRLF. The last paragraph is always present, so
/// empty text and text ending in a newline both end with an empty line, as in
/// DirectWrite.
fn paragraphs(text: &str) -> Vec<(Range<usize>, LineEnding)> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let (mut start, mut index) = (0, 0);
    while index < bytes.len() {
        let ending = match (bytes[index], bytes.get(index + 1)) {
            (b'\r', Some(b'\n')) => LineEnding::CrLf,
            (b'\r', _) => LineEnding::Cr,
            (b'\n', _) => LineEnding::Lf,
            _ => {
                index += 1;
                continue;
            }
        };
        result.push((start..index, ending));
        index += ending.as_str().len();
        start = index;
    }
    result.push((start..text.len(), LineEnding::None));
    result
}

/// Bidi paragraph separators (class B). [`paragraphs`] splits at CR and LF;
/// the bidi algorithm also starts a new paragraph after each of the others.
fn is_paragraph_separator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}')
}

/// A paragraph's copy for cosmic-text, which takes each bidi paragraph's base
/// direction from its first strong character, so a line starting in Arabic
/// would read right to left (and asserts that all bidi paragraphs of a line
/// share one direction). Editor lines read left to right with right-to-left
/// runs inside, as DirectWrite's default reading direction lays them out: a
/// LEFT-TO-RIGHT MARK starts the copy and follows every paragraph separator.
/// ASCII has no right-to-left characters and is copied unmarked.
struct ShapingCopy {
    text: String,
    /// Source offsets a mark was put before, ascending.
    marks: Vec<usize>,
    /// Copy offsets just past each mark, ascending.
    ends: Vec<usize>,
}

impl ShapingCopy {
    fn new(source: &str) -> Self {
        let mut copy = Self {
            text: String::with_capacity(source.len() + LTR_MARK.len()),
            marks: Vec::new(),
            ends: Vec::new(),
        };
        if source.is_ascii() {
            copy.text.push_str(source);
            return copy;
        }
        copy.mark(0);
        for (index, c) in source.char_indices() {
            copy.text.push(c);
            if is_paragraph_separator(c) {
                copy.mark(index + c.len_utf8());
            }
        }
        copy
    }
    fn mark(&mut self, source: usize) {
        self.text.push_str(LTR_MARK);
        self.marks.push(source);
        self.ends.push(self.text.len());
    }
    /// The copy offset of source offset `offset` (after any mark put there).
    fn copy_offset(&self, offset: usize) -> usize {
        offset + LTR_MARK.len() * self.marks.partition_point(|&mark| mark <= offset)
    }
    /// The source offset of copy offset `offset`; inside a mark, where the mark was put.
    fn source_offset(&self, offset: usize) -> usize {
        let before = self.ends.partition_point(|&end| end <= offset);
        let source = offset - LTR_MARK.len() * before;
        self.marks.get(before).map_or(source, |&mark| source.min(mark))
    }
    /// Whether the copy range `start..end` lies within one mark.
    fn is_mark(&self, start: usize, end: usize) -> bool {
        let index = self.ends.partition_point(|&mark_end| mark_end <= start);
        self.ends
            .get(index)
            .is_some_and(|&mark_end| mark_end - LTR_MARK.len() <= start && end <= mark_end)
    }
}

/// Shape `text` in `family` (an installed family name). Unwrapped text uses the
/// face's natural line height; wrapped text breaks at words (or inside a word
/// that does not fit) within `width` and uses uniform 1.2 em lines.
pub(crate) fn shape(fonts: &mut Fonts, text: &str, size: f32, width: f32, family: &str, wrapped: bool) -> Shaped {
    let line_height = size
        * if wrapped {
            WRAPPED_LINE_HEIGHT
        } else {
            fonts.line_height(family)
        };
    let attrs = Attrs::new().family(Family::Name(family));
    let mut shaped = Shaped {
        text: text.to_owned(),
        paragraphs: Vec::new(),
        lines: Vec::new(),
        clusters: Vec::new(),
        glyphs: Vec::new(),
        width: 0.0,
        height: 0.0,
        styles: Vec::new(),
    };
    let emoji = fonts.emoji.clone();
    // Emoji parts shaped for this text, which repeats them often.
    let mut parts = HashMap::new();
    let mut top = 0.0f32;
    for (range, ending) in paragraphs(text) {
        let paragraph = shaped.paragraphs.len();
        let source = &text[range.clone()];
        let copy = ShapingCopy::new(source);
        let mut attrs_list = AttrsList::new(&attrs);
        if !source.is_ascii()
            && let Some(emoji) = emoji.as_deref()
        {
            // Emoji sequences prefer the colour face, as DirectWrite's fallback does.
            let emoji_attrs = attrs.clone().family(Family::Name(emoji));
            for (index, cluster) in source.grapheme_indices(true) {
                if is_emoji_sequence(cluster) {
                    let start = copy.copy_offset(index);
                    attrs_list.add_span(start..start + cluster.len(), &emoji_attrs);
                }
            }
        }
        let mut line = BufferLine::new(copy.text.as_str(), ending, attrs_list.clone(), Shaping::Advanced);
        // Leading alignment, as DirectWrite's default reading direction does.
        line.set_align(Some(Align::Left));
        #[cfg(test)]
        {
            fonts.shaped_paragraphs += 1;
        }
        let layout = line.layout(
            &mut fonts.system,
            size,
            wrapped.then_some(width),
            if wrapped { Wrap::WordOrGlyph } else { Wrap::None },
            Ellipsize::None,
            None,
            TAB_WIDTH,
            Hinting::Disabled,
        );
        for visual in layout {
            let height = visual.line_height_opt.unwrap_or(line_height);
            // Glyphs are centred in the line box, as cosmic-text's own runs are.
            let baseline = top + (height - (visual.max_ascent + visual.max_descent)) / 2.0 + visual.max_ascent;
            let first = shaped.clusters.len();
            // The glyphs in `source` offsets, without the direction marks'.
            let mut glyphs: Vec<LayoutGlyph> = visual
                .glyphs
                .iter()
                .filter(|glyph| !copy.is_mark(glyph.start, glyph.end))
                .map(|glyph| LayoutGlyph {
                    start: copy.source_offset(glyph.start),
                    end: copy.source_offset(glyph.end),
                    ..glyph.clone()
                })
                .collect();
            let growth = split_missing_clusters(fonts, &mut glyphs, source, size, &attrs_list, &copy, &mut parts)
                + overlay_unattached_marks(fonts, &mut glyphs, source);
            for glyph in glyphs {
                let (start, end) = (range.start + glyph.start, range.start + glyph.end);
                let (left, right, rtl) = (glyph.x, glyph.x + glyph.w, glyph.level.is_rtl());
                shaped.glyphs.push(Glyph {
                    layout: glyph,
                    baseline,
                    start,
                });
                // Hit testing needs at least one grapheme per cluster.
                if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                    continue;
                }
                // A cluster's glyphs (a base and its marks) share one byte range.
                let on_line = shaped.clusters.len() > first;
                match shaped.clusters.last_mut() {
                    Some(cluster) if on_line && cluster.start == start && cluster.end == end => {
                        cluster.left = cluster.left.min(left);
                        cluster.right = cluster.right.max(right);
                    }
                    _ => shaped.clusters.push(Cluster {
                        start,
                        end,
                        left,
                        right,
                        rtl,
                    }),
                }
            }
            let right = shaped.clusters[first..]
                .iter()
                .map(|cluster| cluster.right)
                .fold(visual.w + growth, f32::max);
            shaped.width = shaped.width.max(right);
            shaped.lines.push(Line {
                paragraph,
                top,
                height,
                clusters: first..shaped.clusters.len(),
            });
            top += height;
        }
        if shaped.lines.last().is_none_or(|line| line.paragraph != paragraph) {
            let clusters = shaped.clusters.len()..shaped.clusters.len();
            shaped.lines.push(Line {
                paragraph,
                top,
                height: line_height,
                clusters,
            });
            top += line_height;
        }
        shaped.paragraphs.push(range);
    }
    shaped.height = top;
    shaped
}

/// Emoji_Presentation=Yes code points (Unicode 16.0 `emoji-data.txt`), as
/// inclusive ranges in order.
const EMOJI_PRESENTATION: [(u32, u32); 80] = [
    (0x231A, 0x231B),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F1E6, 0x1F1FF),
    (0x1F201, 0x1F201),
    (0x1F21A, 0x1F21A),
    (0x1F22F, 0x1F22F),
    (0x1F232, 0x1F236),
    (0x1F238, 0x1F23A),
    (0x1F250, 0x1F251),
    (0x1F300, 0x1F320),
    (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA),
    (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440),
    (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D7),
    (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF),
    (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FA89),
    (0x1FA8F, 0x1FAC6),
    (0x1FACE, 0x1FADC),
    (0x1FADF, 0x1FAE9),
    (0x1FAF0, 0x1FAF8),
];

/// Whether the grapheme `cluster` asks for emoji presentation (UTS #51): it
/// starts with an emoji-presentation character or carries an emoji variation
/// selector, a keycap, a skin-tone modifier or tags, and no text variation
/// selector.
fn is_emoji_sequence(cluster: &str) -> bool {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let presentation = u32::from(first);
    !cluster.contains('\u{FE0E}')
        && (EMOJI_PRESENTATION
            .iter()
            .any(|(low, high)| (*low..=*high).contains(&presentation))
            || chars
                .any(|c| matches!(c, '\u{FE0F}' | '\u{20E3}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}')))
}

/// Combining diacritics shared by every script (the Inherited blocks), which
/// monospace faces often draw as spacing glyphs in a cell of their own.
fn is_generic_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// Characters that join or select inside an emoji sequence and draw nothing of
/// their own: zero-width joiner, variation selectors and tags.
fn is_emoji_joiner(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0020}'..='\u{E007F}')
}

/// The glyphs one visual line's cluster spans: the run of glyphs from the
/// first that share its byte range.
fn cluster_len(glyphs: &[LayoutGlyph]) -> usize {
    glyphs.first().map_or(0, |first| {
        glyphs
            .iter()
            .take_while(|glyph| glyph.start == first.start && glyph.end == first.end)
            .count()
    })
}

/// Repair clusters with missing glyphs on one visual line (glyphs in visual
/// order, offsets into `text`, the paragraph). cosmic-text falls back per
/// cluster, so an emoji sequence no installed face draws whole (`👍🏽`
/// without a colour emoji face) kept a box per character even where a symbol
/// face has its parts. Each part of such a sequence is shaped on its own with
/// the whole fallback chain instead, and a cluster draws at most one
/// missing-glyph box, as DirectWrite does. `attrs` is in `copy` offsets and
/// `parts` keeps the parts shaped so far. Returns the change in line width.
fn split_missing_clusters(
    fonts: &mut Fonts,
    glyphs: &mut Vec<LayoutGlyph>,
    text: &str,
    size: f32,
    attrs: &AttrsList,
    copy: &ShapingCopy,
    parts: &mut PartCache,
) -> f32 {
    if glyphs.iter().all(|glyph| glyph.glyph_id != 0) {
        return 0.0;
    }
    let mut repaired = Vec::with_capacity(glyphs.len());
    let mut growth = 0.0f32;
    let mut rest = glyphs.as_slice();
    while !rest.is_empty() {
        let (cluster, tail) = rest.split_at(cluster_len(rest));
        rest = tail;
        let first = &cluster[0];
        let missing = cluster.iter().filter(|glyph| glyph.glyph_id == 0).count();
        let source = text.get(first.start..first.end).unwrap_or_default();
        let replacement = if missing == 0 {
            None
        } else if is_emoji_sequence(source) {
            Some(shape_parts(
                fonts,
                parts,
                source,
                size,
                &attrs.get_span(copy.copy_offset(first.start)),
                first.level.is_rtl(),
            ))
        } else if missing > 1 && cluster.iter().all(|glyph| glyph.glyph_id == 0 || glyph.w == 0.0) {
            // Nothing in the cluster is drawn: one box stands for it.
            cluster.iter().find(|glyph| glyph.glyph_id == 0).map(|glyph| {
                vec![LayoutGlyph {
                    x: 0.0,
                    ..glyph.clone()
                }]
            })
        } else {
            None
        };
        let Some(parts) = replacement else {
            repaired.extend(cluster.iter().map(|glyph| LayoutGlyph {
                x: glyph.x + growth,
                ..glyph.clone()
            }));
            continue;
        };
        let left = cluster.iter().map(|glyph| glyph.x).fold(f32::INFINITY, f32::min) + growth;
        let old: f32 = cluster.iter().map(|glyph| glyph.w).sum();
        let new: f32 = parts.iter().map(|glyph| glyph.w).sum();
        repaired.extend(parts.into_iter().map(|glyph| LayoutGlyph {
            start: first.start,
            end: first.end,
            x: left + glyph.x,
            y: first.y,
            level: first.level,
            ..glyph
        }));
        growth += new - old;
    }
    *glyphs = repaired;
    growth
}

/// `cluster`'s parts (a character with any generic marks after it; joiners
/// and selectors dropped) shaped one by one with full fallback and laid out
/// from x = 0 in visual order. Parts no face draws share one missing-glyph box.
/// A part shaped before (in `cache`) is not shaped again.
fn shape_parts(
    fonts: &mut Fonts,
    cache: &mut PartCache,
    cluster: &str,
    size: f32,
    attrs: &Attrs,
    rtl: bool,
) -> Vec<LayoutGlyph> {
    let mut parts: Vec<Range<usize>> = Vec::new();
    let mut open = false;
    for (index, c) in cluster.char_indices() {
        let end = index + c.len_utf8();
        match parts.last_mut() {
            _ if is_emoji_joiner(c) => open = false,
            Some(part) if open && is_generic_mark(c) => part.end = end,
            _ => {
                parts.push(index..end);
                open = true;
            }
        }
    }
    if rtl {
        parts.reverse();
    }
    let mut glyphs = Vec::new();
    let (mut x, mut boxed) = (0.0f32, false);
    for part in parts {
        let shaped = cache
            .entry((cluster[part].to_owned(), AttrsOwned::new(attrs)))
            .or_insert_with_key(|(part, _)| shape_part(fonts, part, size, attrs));
        if let Some(missing) = shaped.iter().find(|glyph| glyph.glyph_id == 0) {
            if !boxed {
                boxed = true;
                glyphs.push(LayoutGlyph { x, ..missing.clone() });
                x += missing.w;
            }
            continue;
        }
        let width = shaped.iter().map(|glyph| glyph.x + glyph.w).fold(0.0, f32::max);
        glyphs.extend(shaped.iter().map(|glyph| LayoutGlyph {
            x: x + glyph.x,
            ..glyph.clone()
        }));
        x += width;
    }
    glyphs
}

/// Emoji parts already shaped in one [`shape`] call, by text and attributes.
type PartCache = HashMap<(String, AttrsOwned), Vec<LayoutGlyph>>;

/// `part` shaped on its own with full fallback, from x = 0.
fn shape_part(fonts: &mut Fonts, part: &str, size: f32, attrs: &Attrs) -> Vec<LayoutGlyph> {
    #[cfg(test)]
    {
        fonts.shaped_parts += 1;
    }
    let mut line = BufferLine::new(part, LineEnding::None, AttrsList::new(attrs), Shaping::Advanced);
    let layout = line.layout(
        &mut fonts.system,
        size,
        None,
        Wrap::None,
        Ellipsize::None,
        None,
        TAB_WIDTH,
        Hinting::Disabled,
    );
    layout.iter().flat_map(|visual| visual.glyphs.iter().cloned()).collect()
}

/// Draw generic combining marks the face does not attach over their base, as
/// fallback mark positioning does: centred on the base's advance, taking no
/// width. Monospace faces draw such marks in a cell of their own, either as
/// spacing glyphs (DejaVu Sans Mono's U+0335 and U+0336) or, when the shaper
/// zeroes their advance but no anchor fits the base (a mark after a
/// precomposed letter, as in `x` + U+0323 + U+0307), over the next character.
/// Only left-to-right clusters of one base and generic marks qualify. Returns
/// the change in line width.
fn overlay_unattached_marks(fonts: &mut Fonts, glyphs: &mut [LayoutGlyph], text: &str) -> f32 {
    let mut growth = 0.0f32;
    let mut index = 0;
    while index < glyphs.len() {
        let count = cluster_len(&glyphs[index..]);
        let cluster = &mut glyphs[index..index + count];
        index += count;
        for glyph in cluster.iter_mut() {
            glyph.x += growth;
        }
        let (base, marks) = cluster.split_at_mut(1);
        let base = &base[0];
        let mut chars = text.get(base.start..base.end).unwrap_or_default().chars().skip(1);
        if marks.is_empty() || base.level.is_rtl() || marks.len() > chars.clone().count() || !chars.all(is_generic_mark)
        {
            continue;
        }
        for mark in marks {
            if mark.x_offset != 0.0 || mark.y_offset != 0.0 {
                continue;
            }
            let advance = if mark.w > 0.0 {
                mark.w
            } else {
                design_advance(fonts, mark)
            };
            if advance > 0.0 {
                growth -= mark.w;
                mark.x = base.x + (base.w - advance) / 2.0;
                mark.w = 0.0;
            }
        }
    }
    growth
}

/// `glyph`'s advance in its face's metrics, at its size.
fn design_advance(fonts: &mut Fonts, glyph: &LayoutGlyph) -> f32 {
    fonts
        .system
        .get_font(glyph.font_id, glyph.font_weight)
        .map_or(0.0, |font| {
            let face = font.as_swash();
            let units = f32::from(face.metrics(&[]).units_per_em.max(1));
            face.glyph_metrics(&[]).advance_width(glyph.glyph_id) * glyph.font_size / units
        })
}

impl Cluster {
    fn width(&self) -> f32 {
        self.right - self.left
    }
    /// Visual x of the boundary `cells` graphemes into a cluster of `total`.
    fn edge(&self, cells: usize, total: usize) -> f32 {
        let fraction = cells as f32 / total.max(1) as f32;
        if self.rtl {
            self.right - self.width() * fraction
        } else {
            self.left + self.width() * fraction
        }
    }
}

impl Shaped {
    /// Width including trailing whitespace, and height of all lines.
    pub(crate) fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }
    pub(crate) fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }
    fn check_offset(&self, offset: usize) -> Result<(), LayoutError> {
        if offset <= self.text.len() && self.text.is_char_boundary(offset) {
            Ok(())
        } else {
            Err(LayoutError::InvalidOffset)
        }
    }
    /// Replace the foreground styles without reshaping. Ranges must be sorted,
    /// nonempty, nonoverlapping and on character boundaries; an invalid list
    /// leaves the previous styles in place.
    pub(crate) fn set_styles(&mut self, styles: &[TextStyle]) -> Result<(), LayoutError> {
        let mut mapped = Vec::with_capacity(styles.len());
        let mut previous = 0;
        for style in styles {
            if style.bytes.start < previous || style.bytes.start >= style.bytes.end {
                return Err(LayoutError::InvalidOffset);
            }
            self.check_offset(style.bytes.start)?;
            self.check_offset(style.bytes.end)?;
            mapped.push((style.bytes.clone(), style.color));
            previous = style.bytes.end;
        }
        self.styles = mapped;
        Ok(())
    }
    /// Foreground colour of the byte at `offset`.
    pub(crate) fn color_at(&self, offset: usize, fallback: Color) -> Color {
        let index = self.styles.partition_point(|(range, _)| range.end <= offset);
        match self.styles.get(index) {
            Some((range, color)) if range.start <= offset => *color,
            _ => fallback,
        }
    }
    fn line_clusters(&self, line: &Line) -> &[Cluster] {
        &self.clusters[line.clusters.clone()]
    }
    /// Byte offsets of the grapheme boundaries inside `cluster`, both ends included.
    fn cells(&self, cluster: &Cluster) -> Vec<usize> {
        self.text[cluster.start..cluster.end]
            .grapheme_indices(true)
            .map(|(index, _)| cluster.start + index)
            .chain(Some(cluster.end))
            .collect()
    }
    /// DirectWrite `HitTestPoint`: the nearest grapheme on the nearest line;
    /// `trailing` when the point is on the grapheme's trailing half, in which
    /// case the offset is the grapheme's end.
    pub(crate) fn hit_test(&self, point: Point) -> TextHit {
        let index = self
            .lines
            .iter()
            .position(|line| point.y < line.top + line.height)
            .unwrap_or(self.lines.len() - 1);
        let line = &self.lines[index];
        let inside_y = point.y >= 0.0 && point.y < self.height;
        let clusters = self.line_clusters(line);
        let (Some(first), Some(last)) = (clusters.first(), clusters.last()) else {
            return TextHit {
                byte_offset: self.paragraphs[line.paragraph].start,
                inside: false,
                trailing: false,
            };
        };
        let right = clusters.iter().map(|cluster| cluster.right).fold(last.right, f32::max);
        // Outside the line: the outermost cluster's near edge.
        let outside = if point.x < first.left {
            Some((first, first.rtl))
        } else if point.x >= right {
            Some((last, !last.rtl))
        } else {
            None
        };
        if let Some((cluster, trailing)) = outside {
            return TextHit {
                byte_offset: if trailing { cluster.end } else { cluster.start },
                inside: false,
                trailing,
            };
        }
        let cluster = clusters.iter().find(|cluster| point.x < cluster.right).unwrap_or(last);
        let cells = self.cells(cluster);
        let total = cells.len() - 1;
        let cell_width = cluster.width() / total as f32;
        let visual = if cell_width > 0.0 {
            (((point.x - cluster.left) / cell_width).floor().max(0.0) as usize).min(total - 1)
        } else {
            0
        };
        let logical = if cluster.rtl { total - 1 - visual } else { visual };
        let middle = cluster.left + cell_width * (visual as f32 + 0.5);
        let trailing = (point.x >= middle) != cluster.rtl;
        TextHit {
            byte_offset: if trailing { cells[logical + 1] } else { cells[logical] },
            inside: inside_y,
            trailing,
        }
    }
    /// DirectWrite `HitTestTextPosition` (leading edge): a 1.5 DIP caret at the
    /// leading edge of the grapheme at `offset`, or after the paragraph's last
    /// cluster when `offset` ends a paragraph.
    pub(crate) fn caret(&self, offset: usize) -> Result<Rect, LayoutError> {
        self.check_offset(offset)?;
        let (line, x) = self.caret_position(offset);
        Ok(Rect {
            x,
            y: line.top,
            width: 1.5,
            height: line.height,
        })
    }
    fn caret_position(&self, offset: usize) -> (&Line, f32) {
        // The paragraph holding `offset`; offsets inside a line ending belong to
        // the paragraph that ending closes.
        let paragraph = self
            .paragraphs
            .partition_point(|range| range.start <= offset)
            .saturating_sub(1);
        let lines = self.lines.iter().filter(move |line| line.paragraph == paragraph);
        for line in lines.clone() {
            for cluster in self.line_clusters(line) {
                if cluster.start <= offset && offset < cluster.end {
                    let cells = self.cells(cluster);
                    let before = cells.iter().skip(1).filter(|end| **end <= offset).count();
                    return (line, cluster.edge(before, cells.len() - 1));
                }
            }
        }
        for line in lines.clone().rev() {
            if let Some(cluster) = self.line_clusters(line).iter().find(|cluster| cluster.end == offset) {
                return (line, if cluster.rtl { cluster.left } else { cluster.right });
            }
        }
        let line = lines.clone().next_back().unwrap_or(&self.lines[0]);
        let end = self
            .line_clusters(line)
            .iter()
            .map(|cluster| cluster.right)
            .fold(0.0, f32::max);
        // Past the last cluster only when the paragraph's text precedes `offset`.
        let x = if offset > self.paragraphs[paragraph].start {
            end
        } else {
            0.0
        };
        (line, x)
    }
    /// DirectWrite `HitTestTextRange`: one rectangle per visually contiguous
    /// selected span per line, spanning the line box.
    pub(crate) fn range_rects(&self, bytes: Range<usize>) -> Result<Vec<Rect>, LayoutError> {
        self.check_offset(bytes.start)?;
        self.check_offset(bytes.end)?;
        if bytes.start == bytes.end {
            return Ok(Vec::new());
        }
        let mut rects = Vec::new();
        for line in &self.lines {
            let mut span: Option<(f32, f32)> = None;
            for cluster in self.line_clusters(line) {
                if cluster.end <= bytes.start || cluster.start >= bytes.end {
                    continue;
                }
                let (left, right) = if bytes.start <= cluster.start && cluster.end <= bytes.end {
                    (cluster.left, cluster.right)
                } else {
                    // A partly selected ligature: the selected graphemes' share.
                    let cells = self.cells(cluster);
                    let total = cells.len() - 1;
                    let first = cells.windows(2).position(|cell| cell[1] > bytes.start).unwrap_or(0);
                    let last = cells
                        .windows(2)
                        .rposition(|cell| cell[0] < bytes.end)
                        .unwrap_or(total - 1);
                    let (a, b) = (cluster.edge(first, total), cluster.edge(last + 1, total));
                    (a.min(b), a.max(b))
                };
                span = match span {
                    Some((start, end)) if left <= end + 0.01 && right >= start - 0.01 => {
                        Some((start.min(left), end.max(right)))
                    }
                    Some((start, end)) => {
                        rects.push(line_rect(line, start, end));
                        Some((left, right))
                    }
                    None => Some((left, right)),
                };
            }
            if let Some((start, end)) = span {
                rects.push(line_rect(line, start, end));
            }
        }
        if rects.is_empty() {
            // Only line-ending bytes: an empty rectangle at the caret, as the
            // DirectWrite range always yields at least one rectangle.
            let (line, x) = self.caret_position(bytes.start);
            rects.push(line_rect(line, x, x));
        }
        Ok(rects)
    }
}
fn line_rect(line: &Line, left: f32, right: f32) -> Rect {
    Rect {
        x: left,
        y: line.top,
        width: right - left,
        height: line.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paragraphs_split_every_line_ending_and_keep_a_trailing_empty_line() {
        let spans = |text: &str| -> Vec<(usize, usize)> {
            paragraphs(text)
                .into_iter()
                .map(|(range, _)| (range.start, range.end))
                .collect()
        };
        assert_eq!(spans(""), [(0, 0)]);
        assert_eq!(spans("ab"), [(0, 2)]);
        assert_eq!(spans("a\nb\r\nc\rd"), [(0, 1), (2, 3), (5, 6), (7, 8)]);
        assert_eq!(spans("a\n"), [(0, 1), (2, 2)]);
    }
    #[test]
    fn missing_families_fall_back_to_the_bundled_monospace_face() {
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        assert_eq!(fonts.family(Some("Cascadia Mono"), 14.0), BUNDLED_FONT_FAMILY);
        assert_eq!(fonts.family(Some("dejavu sans mono"), 14.0), BUNDLED_FONT_FAMILY);
        assert_eq!(fonts.family(None, 16.0), BUNDLED_FONT_FAMILY);
        assert_eq!(
            fonts.family(None, 13.0),
            BUNDLED_FONT_FAMILY,
            "no interface face installed"
        );
        for index in 0..(MAX_RESOLVED_FAMILIES * 2) {
            fonts.family(Some(&format!("Missing {index}")), 14.0);
            assert!(fonts.resolved.len() <= MAX_RESOLVED_FAMILIES);
        }
        let ratio = fonts.line_height(BUNDLED_FONT_FAMILY);
        assert!((1.1..1.3).contains(&ratio), "DejaVu Sans Mono natural height {ratio}");
    }
    #[test]
    fn styles_resolve_by_byte_and_reject_crossed_or_split_ranges() {
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        let mut shaped = shape(&mut fonts, "ab文cd", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let red = Color(0xFF0000);
        shaped
            .set_styles(&[TextStyle {
                bytes: 2..5,
                color: red,
            }])
            .unwrap();
        assert_eq!(shaped.color_at(1, Color(0)), Color(0));
        assert_eq!(shaped.color_at(2, Color(0)), red);
        assert_eq!(shaped.color_at(5, Color(0)), Color(0));
        let crossed = Range { start: 4, end: 2 };
        for invalid in [3..5, 2..2, crossed, 2..9] {
            let style = TextStyle {
                bytes: invalid.clone(),
                color: Color(1),
            };
            assert_eq!(
                shaped.set_styles(&[style]),
                Err(LayoutError::InvalidOffset),
                "{invalid:?}"
            );
        }
        assert_eq!(
            shaped.color_at(2, Color(0)),
            red,
            "a rejected list keeps the old styles"
        );
    }

    /// The glyphs of `shaped` grouped by cluster byte range, in visual order.
    fn clusters_of(shaped: &Shaped) -> Vec<(usize, Vec<&LayoutGlyph>)> {
        let mut clusters: Vec<(usize, Vec<&LayoutGlyph>)> = Vec::new();
        for glyph in shaped.glyphs() {
            match clusters.last_mut() {
                Some((start, glyphs)) if *start == glyph.start => glyphs.push(&glyph.layout),
                _ => clusters.push((glyph.start, vec![&glyph.layout])),
            }
        }
        clusters
    }
    /// The id of the face whose first family is `family`.
    fn face_id(fonts: &Fonts, family: &str) -> fontdb::ID {
        fonts
            .system
            .db()
            .faces()
            .find(|face| face.families.first().is_some_and(|(name, _)| name == family))
            .map(|face| face.id)
            .unwrap()
    }

    #[test]
    fn spacing_combining_marks_overlay_their_base_and_take_no_width() {
        // LNX-UI-009: DejaVu Sans Mono draws U+0336 and U+0335 as spacing glyphs.
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        let plain = shape(&mut fonts, "Zx", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let text = "Z\u{336}\u{335}x";
        let marked = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        assert_eq!(marked.size(), plain.size(), "the marks add no width");
        let glyphs: Vec<&LayoutGlyph> = marked.glyphs().iter().map(|glyph| &glyph.layout).collect();
        assert_eq!(glyphs.len(), 4);
        for mark in &glyphs[1..3] {
            assert!(mark.glyph_id != 0 && mark.w == 0.0, "{mark:?}");
            assert_eq!(mark.x, glyphs[0].x, "a monospace mark's cell lies over its base");
        }
        assert_eq!(glyphs[3].x, plain.glyphs()[1].layout.x);
        let x = text.find('x').unwrap();
        assert_eq!(marked.caret(x).unwrap(), plain.caret(1).unwrap());
        assert_eq!(
            marked.caret(1).unwrap().x,
            0.0,
            "a mark's offset is its cluster's leading edge"
        );
        assert_eq!(marked.range_rects(0..x).unwrap(), plain.range_rects(0..1).unwrap());
        // A sequence with a precomposed form keeps every mark: o + U+0302 + U+0323
        // is drawn as U+1ED9, dot below included.
        let composed = shape(&mut fonts, "\u{1ED9}", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let decomposed = shape(&mut fonts, "o\u{302}\u{323}", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        assert_eq!(decomposed.glyphs().len(), 1);
        assert_eq!(
            decomposed.glyphs()[0].layout.glyph_id,
            composed.glyphs()[0].layout.glyph_id
        );
        // A mark no anchor attaches to a precomposed base (U+1E8B, U+1EB9) is
        // drawn over that base, not over the next character.
        for text in ["x\u{323}\u{307}z", "e\u{301}\u{323}z", "a\u{302}\u{301}z"] {
            let shaped = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
            let glyphs: Vec<&LayoutGlyph> = shaped.glyphs().iter().map(|glyph| &glyph.layout).collect();
            let (base, next) = (glyphs[0], glyphs[glyphs.len() - 1]);
            assert!(glyphs.len() > 2, "{text:?} keeps a separate mark");
            for mark in &glyphs[1..glyphs.len() - 1] {
                assert!(mark.w == 0.0 && mark.x < base.x + base.w, "{text:?}: {mark:?}");
            }
            assert!(
                (next.x - base.w).abs() < 0.01,
                "{text:?}: the next letter keeps its place"
            );
        }
        // Marks the face attaches keep the face's placement.
        let attached = shape(&mut fonts, "q\u{323}", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        assert!(attached.glyphs()[1].layout.x_offset < 0.0);
    }

    #[test]
    fn a_cluster_no_face_draws_shows_one_missing_glyph_box() {
        // LNX-UI-017: one box per cluster, not one per character.
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        let text = "👍🏽 👨\u{200D}👩\u{200D}👧 🇯🇵 नमस्ते ❤\u{FE0F} e\u{301}";
        let shaped = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let clusters = clusters_of(&shaped);
        for (start, glyphs) in &clusters {
            let boxes = glyphs.iter().filter(|glyph| glyph.glyph_id == 0).count();
            assert!(boxes <= 1, "{boxes} boxes for the cluster at {start} of {text:?}");
        }
        let (_, family) = clusters
            .iter()
            .find(|(start, _)| *start == text.find('👨').unwrap())
            .unwrap();
        assert_eq!(family.len(), 1, "the joiners draw nothing");
        let (_, heart) = clusters
            .iter()
            .find(|(start, _)| *start == text.find('❤').unwrap())
            .unwrap();
        assert!(heart[0].glyph_id != 0, "the bundled face's heart is kept");
        // Geometry stays consistent: the cluster is as wide as its one box.
        let flag = text.find('🇯').unwrap();
        let rects = shaped.range_rects(flag..flag + "🇯🇵".len()).unwrap();
        let (_, boxed) = clusters.iter().find(|(start, _)| *start == flag).unwrap();
        assert!(
            rects.len() == 1 && (rects[0].width - boxed[0].w).abs() < 0.01,
            "{rects:?}"
        );
    }

    #[test]
    fn emoji_sequences_prefer_an_installed_colour_face() {
        // A stand-in colour face: the bundled data under the Noto Color Emoji name.
        let mut db = fontdb::Database::new();
        db.load_font_source(fontdb::Source::Binary(Arc::new(BUNDLED_FONT)));
        let mut colour = db.faces().next().unwrap().clone();
        colour.families = vec![("Noto Color Emoji".to_owned(), fontdb::Language::English_UnitedStates)];
        colour.post_script_name = "NotoColorEmoji".to_owned();
        db.push_face_info(colour);
        let mut fonts = Fonts::with_database(db);
        assert_eq!(fonts.emoji.as_deref(), Some("Noto Color Emoji"));
        let (mono, emoji) = (
            face_id(&fonts, BUNDLED_FONT_FAMILY),
            face_id(&fonts, "Noto Color Emoji"),
        );
        // Emoji presentation by selector or by default; text presentation stays.
        let text = "❤\u{FE0F} ❤ ⌚ ⌚\u{FE0E} ✔ a";
        let shaped = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let starts = text.match_indices(['❤', '⌚', '✔', 'a']).map(|(index, _)| index);
        let expected = [emoji, mono, emoji, mono, mono, mono];
        for (start, face) in starts.zip(expected) {
            let glyph = shaped.glyphs().iter().find(|glyph| glyph.start == start).unwrap();
            assert_eq!(glyph.layout.font_id, face, "the cluster at {start} of {text:?}");
        }
        // Without a colour face nothing changes.
        assert_eq!(Fonts::new(FontSource::BundledOnly).emoji, None);
    }

    #[test]
    fn emoji_parts_fall_back_one_by_one_when_no_face_has_the_sequence() {
        // LNX-UI-017: 👍🏽 without a colour face keeps the 👍 a symbol face
        // (Noto Sans Symbols2, Segoe UI Symbol) has. Needs such a face installed.
        let mut system = fontdb::Database::new();
        system.load_system_fonts();
        let mut probe = FontSystem::new_with_locale_and_db("en-US".to_owned(), system);
        let ids: Vec<fontdb::ID> = probe.db().faces().map(|face| face.id).collect();
        let symbols = ids.into_iter().find(|id| {
            probe.get_font(*id, fontdb::Weight::NORMAL).is_some_and(|font| {
                let charmap = font.as_swash().charmap();
                charmap.map('👍') != 0 && charmap.map('\u{1F3FD}') == 0
            })
        });
        let Some(symbols) = symbols else {
            eprintln!("skipped: no installed face has U+1F44D without U+1F3FD");
            return;
        };
        let face = probe.db().face(symbols).unwrap();
        let (source, family) = (face.source.clone(), face.families[0].0.clone());
        let mut db = fontdb::Database::new();
        db.load_font_source(fontdb::Source::Binary(Arc::new(BUNDLED_FONT)));
        db.load_font_source(source);
        let mut fonts = Fonts::with_database(db);
        let shaped = shape(&mut fonts, "👍🏽 ok", 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let clusters = clusters_of(&shaped);
        let thumb = &clusters[0].1;
        let symbol_face = face_id(&fonts, &family);
        assert!(
            thumb
                .iter()
                .any(|glyph| glyph.font_id == symbol_face && glyph.glyph_id != 0),
            "{family} draws the thumb: {thumb:?}"
        );
        assert_eq!(
            thumb.iter().filter(|glyph| glyph.glyph_id == 0).count(),
            1,
            "one box for the modifier"
        );
        // The rest of the line follows the repaired cluster.
        let right = thumb.iter().map(|glyph| glyph.x + glyph.w).fold(0.0, f32::max);
        assert!((shaped.caret("👍🏽".len()).unwrap().x - right).abs() < 0.01);
    }

    #[test]
    fn repeated_emoji_parts_are_shaped_once_per_layout() {
        // LNX-UI-017: a line full of sequences no face draws whole shapes each
        // part with fallback once, not once per cluster.
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        let unit = "👍🏽 ";
        let text = unit.repeat(200);
        let shaped = shape(&mut fonts, &text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        assert_eq!(fonts.shaped_parts, 2, "👍 and 🏽, once each");
        let step = shaped.caret(unit.len()).unwrap().x;
        let last = shaped.caret(199 * unit.len()).unwrap().x;
        assert!(step > 0.0 && (last - 199.0 * step).abs() < 0.1, "{step} {last}");
        for (start, glyphs) in clusters_of(&shaped) {
            let boxes = glyphs.iter().filter(|glyph| glyph.glyph_id == 0).count();
            assert!(boxes <= 1, "{boxes} boxes for the cluster at {start}");
        }
    }

    #[test]
    fn shaping_copies_mark_every_bidi_paragraph_and_map_offsets_back() {
        let ascii = ShapingCopy::new("a\u{1C}b");
        assert_eq!((ascii.text.as_str(), ascii.marks.len()), ("a\u{1C}b", 0));
        // Copy: mark 0..3, ש 3..5, U+2029 5..8, mark 8..11, a 11..12.
        let copy = ShapingCopy::new("ש\u{2029}a");
        assert_eq!(copy.text, "\u{200E}ש\u{2029}\u{200E}a");
        assert_eq!(
            (copy.marks.as_slice(), copy.ends.as_slice()),
            ([0, 5].as_slice(), [3, 11].as_slice())
        );
        for (source, shaping) in [(0, 3), (2, 5), (5, 11), (6, 12)] {
            assert_eq!(copy.copy_offset(source), shaping);
            assert_eq!(copy.source_offset(shaping), source);
        }
        for (inside, source) in [(0, 0), (1, 0), (8, 5), (9, 5)] {
            assert_eq!(copy.source_offset(inside), source, "copy offset {inside}");
        }
        assert!(copy.is_mark(0, 3) && copy.is_mark(8, 11));
        assert!(!copy.is_mark(3, 5) && !copy.is_mark(5, 8) && !copy.is_mark(11, 12));
    }

    #[test]
    fn every_bidi_paragraph_of_a_line_reads_left_to_right() {
        // LNX-UI-010: the bidi algorithm also starts a paragraph after U+001C-U+001E,
        // U+0085 and U+2029. Each must keep the left-to-right base direction
        // (cosmic-text asserts that a line's bidi paragraphs agree, so a
        // right-to-left one after a left-to-right one panicked).
        let mut fonts = Fonts::new(FontSource::BundledOnly);
        let rtl_words = ["مرحبا", "שלום"];
        for text in [
            "مرحبا\u{2029}שלום",
            "a\u{1C}مرحبا",
            "مرحبا\u{85}abc",
            "x\u{1D}שלום end",
            "\u{2029}مرحبا\u{1E}",
        ] {
            let shaped = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
            let caret = |offset: usize| shaped.caret(offset).unwrap().x;
            for glyph in shaped.glyphs() {
                let end = glyph.start + (glyph.layout.end - glyph.layout.start);
                assert!(
                    glyph.start < end && text.is_char_boundary(glyph.start) && text.is_char_boundary(end),
                    "{text:?}: {:?} at {}",
                    glyph.layout,
                    glyph.start
                );
            }
            let all = shaped.range_rects(0..text.len()).unwrap();
            assert!(
                all.iter().all(|rect| rect.x >= -0.01) && all.iter().any(|rect| rect.x.abs() < 0.01),
                "{text:?} starts at the left: {all:?}"
            );
            // Text after a separator lies right of the text before it.
            for (at, separator) in text.match_indices(is_paragraph_separator) {
                let after = at + separator.len();
                let before = shaped.range_rects(0..after).unwrap();
                let right = before.iter().map(|rect| rect.x + rect.width).fold(0.0, f32::max);
                if after < text.len() {
                    let rest = shaped.range_rects(after..text.len()).unwrap();
                    let left = rest.iter().map(|rect| rect.x).fold(f32::INFINITY, f32::min);
                    assert!(right <= left + 0.01, "{text:?}: {before:?} then {rest:?}");
                }
            }
            // Right-to-left words still read right to left.
            for word in rtl_words {
                if let Some(start) = text.find(word) {
                    let last = start + word.char_indices().last().unwrap().0;
                    assert!(caret(last) < caret(start), "{text:?}: {word}");
                }
            }
        }
        // A left-to-right base after the separator: "end" follows the Hebrew word.
        let text = "x\u{1D}שלום end";
        let shaped = shape(&mut fonts, text, 16.0, 1.0e6, BUNDLED_FONT_FAMILY, false);
        let hebrew = text.find('ש').unwrap()..text.find(" end").unwrap();
        let rects = shaped.range_rects(hebrew).unwrap();
        assert!(rects.len() == 1 && shaped.caret(text.find("end").unwrap()).unwrap().x >= rects[0].x + rects[0].width);
    }
}
