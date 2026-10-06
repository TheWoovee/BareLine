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
    Align, Attrs, AttrsList, BufferLine, CacheKey, Ellipsize, Family, FontSystem, Hinting, LayoutGlyph, LineEnding,
    Shaping, SwashCache, SwashImage, Wrap, fontdb,
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
    /// Paragraphs shaped since creation; tests use it to prove caching.
    #[cfg(test)]
    pub(crate) shaped_paragraphs: usize,
}

impl Fonts {
    pub(crate) fn new(source: FontSource) -> Self {
        let mut db = fontdb::Database::new();
        if source == FontSource::System {
            db.load_system_fonts();
        }
        db.load_font_source(fontdb::Source::Binary(Arc::new(BUNDLED_FONT)));
        let monospace = first_installed(&db, &MONOSPACE_FAMILIES).unwrap_or_else(|| BUNDLED_FONT_FAMILY.to_owned());
        let interface = first_installed(&db, &INTERFACE_FAMILIES).unwrap_or_else(|| monospace.clone());
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
            #[cfg(test)]
            shaped_paragraphs: 0,
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
    let mut top = 0.0f32;
    for (range, ending) in paragraphs(text) {
        let paragraph = shaped.paragraphs.len();
        let mut line = BufferLine::new(&text[range.clone()], ending, AttrsList::new(&attrs), Shaping::Advanced);
        // Leading alignment for every paragraph direction, as DirectWrite's
        // default left-to-right reading direction does.
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
            for glyph in &visual.glyphs {
                let (start, end) = (range.start + glyph.start, range.start + glyph.end);
                shaped.glyphs.push(Glyph {
                    layout: glyph.clone(),
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
                        cluster.left = cluster.left.min(glyph.x);
                        cluster.right = cluster.right.max(glyph.x + glyph.w);
                    }
                    _ => shaped.clusters.push(Cluster {
                        start,
                        end,
                        left: glyph.x,
                        right: glyph.x + glyph.w,
                        rtl: glyph.level.is_rtl(),
                    }),
                }
            }
            let right = shaped.clusters[first..]
                .iter()
                .map(|cluster| cluster.right)
                .fold(visual.w, f32::max);
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
}
