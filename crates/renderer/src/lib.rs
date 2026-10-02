// SPDX-License-Identifier: MPL-2.0
/// Instrumentation is removed at compile time unless explicitly enabled.
#[cfg(feature = "perf-spans")]
pub fn frame_span() -> tracing::span::EnteredSpan {
    tracing::info_span!("renderer.frame").entered()
}
#[cfg(not(feature = "perf-spans"))]
pub struct DisabledFrameSpan;
#[cfg(not(feature = "perf-spans"))]
#[inline(always)]
pub fn frame_span() -> DisabledFrameSpan {
    DisabledFrameSpan
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u32);
/// Immutable straight-alpha RGBA8 pixels; validation happens before native allocation.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: std::sync::Arc<[u8]>,
}
impl Image {
    pub fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, LayoutError> {
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4))
            .ok_or(LayoutError::ResourceLimit)?;
        if width == 0 || height == 0 || bytes > 16 * 1024 * 1024 || bytes != pixels.len() as u64 {
            return Err(LayoutError::ResourceLimit);
        }
        Ok(Self {
            width,
            height,
            pixels: pixels.into(),
        })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum DrawOp {
    Fill(Rect, Color),
    Stroke(Rect, Color, f32),
    FillRounded(Rect, Color, f32),
    StrokeRounded(Rect, Color, f32, f32),
    Text {
        origin: Point,
        text: String,
        size: f32,
        color: Color,
    },
    PushClip(Rect),
    PopClip,
    Image {
        image: Image,
        destination: Rect,
        opacity: f32,
    },
    PushLayer {
        bounds: Rect,
        opacity: f32,
    },
    PopLayer,
    Layout {
        origin: Point,
        layout: LayoutId,
        color: Color,
    },
    Line {
        from: Point,
        to: Point,
        color: Color,
        width: f32,
    },
}

/// Opaque, process-unique identifier; only the creating backend can resolve it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LayoutId(u64);
impl LayoutId {
    pub fn allocate() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextHit {
    pub byte_offset: usize,
    pub inside: bool,
    pub trailing: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    InvalidHandle,
    InvalidOffset,
    ResourceLimit,
    BackendFailure,
}
impl LayoutError {
    /// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
    pub const fn user_message(self) -> &'static str {
        match self {
            Self::InvalidHandle => "the text layout was released before it was drawn; it will be redrawn",
            Self::InvalidOffset => "the position is outside the laid-out text",
            Self::ResourceLimit => "too much text is laid out at once; close some views or panels",
            Self::BackendFailure => "the graphics system could not lay out the text",
        }
    }
}
impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.user_message())
    }
}
pub const MAX_LAYOUT_BYTES: usize = 64 * 1024;
pub const MAX_LAYOUTS: usize = 512;
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// Sorted, nonoverlapping UTF-8 byte ranges in the shaped text.
    pub bytes: std::ops::Range<usize>,
    pub color: Color,
}
pub trait TextBackend {
    fn measure_text(&mut self, text: &str, size: f32) -> Result<(f32, f32), LayoutError> {
        let layout = self.shape(text, size, 1.0e9)?;
        let measured = self.layout_size(layout);
        self.release_layout(layout);
        measured
    }
    fn shape_wrapped(&mut self, text: &str, size: f32, width: f32, family: &str) -> Result<LayoutId, LayoutError> {
        self.shape_with_font_family(text, size, width, family)
    }
    fn layout_size(&self, layout: LayoutId) -> Result<(f32, f32), LayoutError>;
    fn shape_with_font_family(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> Result<LayoutId, LayoutError> {
        if !valid_font_family(family) {
            return Err(LayoutError::InvalidOffset);
        }
        self.shape(text, size, width)
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> Result<LayoutId, LayoutError>;
    /// Replace foreground styles without reshaping text or changing hit tests.
    /// Invalid ranges fail before changing the existing styles.
    fn set_styles(&mut self, layout: LayoutId, styles: &[TextStyle]) -> Result<(), LayoutError>;
    fn hit_test(&self, layout: LayoutId, point: Point) -> Result<TextHit, LayoutError>;
    /// UTF-8 byte offset. Returns leading caret bounds in logical pixels.
    fn caret(&self, layout: LayoutId, byte_offset: usize) -> Result<Rect, LayoutError>;
    fn range_rects(&self, layout: LayoutId, bytes: std::ops::Range<usize>) -> Result<Vec<Rect>, LayoutError>;
    fn release_layout(&mut self, layout: LayoutId);
}
pub fn valid_font_family(family: &str) -> bool {
    !family.trim().is_empty() && family.len() <= 256 && !family.chars().any(char::is_control)
}

/// Validate before entering a native draw frame. Prevents stack underflow at FFI boundaries.
pub fn balanced_clips(ops: &[DrawOp]) -> bool {
    fn valid_rect(r: Rect) -> bool {
        [r.x, r.y, r.width, r.height, r.x + r.width, r.y + r.height]
            .iter()
            .all(|n| n.is_finite())
            && r.width >= 0.0
            && r.height >= 0.0
    }
    let mut stack = Vec::new();
    for op in ops {
        match op {
            DrawOp::PushClip(_) => stack.push(false),
            DrawOp::PushLayer { bounds, opacity } => {
                if !valid_rect(*bounds) || !opacity.is_finite() || !(0.0..=1.0).contains(opacity) {
                    return false;
                }
                stack.push(true);
            }
            DrawOp::PopClip if stack.pop() != Some(false) => return false,
            DrawOp::PopLayer if stack.pop() != Some(true) => return false,
            DrawOp::Image {
                destination, opacity, ..
            } if !valid_rect(*destination) || !opacity.is_finite() || !(0.0..=1.0).contains(opacity) => return false,
            _ => {}
        }
        if stack.len() > 256 {
            return false;
        }
    }
    stack.is_empty()
}
/// A wavy underline for a spelling error (BIZ-31): a zig-zag of `Line`
/// segments from `left` to `right` whose low points sit on `bottom`. At most
/// [`MAX_SQUIGGLE_SEGMENTS`] segments, so an absurd width cannot flood a frame.
pub fn squiggle(left: f32, right: f32, bottom: f32, color: Color, ops: &mut Vec<DrawOp>) {
    const STEP: f32 = 2.0;
    const DEPTH: f32 = 2.0;
    if !(left.is_finite() && right.is_finite() && bottom.is_finite()) || right - left < STEP {
        return;
    }
    let mut x = left;
    let mut y = bottom;
    for segment in 0..MAX_SQUIGGLE_SEGMENTS {
        if x >= right {
            break;
        }
        let next = (x + STEP).min(right);
        let peak = if segment % 2 == 0 { bottom - DEPTH } else { bottom };
        let to = y + (peak - y) * (next - x) / STEP;
        ops.push(DrawOp::Line {
            from: Point { x, y },
            to: Point { x: next, y: to },
            color,
            width: 1.0,
        });
        x = next;
        y = to;
    }
}
pub const MAX_SQUIGGLE_SEGMENTS: usize = 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameStatus {
    Presented,
    Recreate,
}
pub trait RenderBackend {
    type Error;
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<(), Self::Error>;
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus, Self::Error>;
    fn begin_frame(&mut self) -> Painter<'_, Self>
    where
        Self: Sized,
    {
        Painter {
            backend: self,
            operations: Vec::new(),
        }
    }
}

/// A frame owns its commands and exclusively borrows its backend. Dropping cancels
/// the frame; only `finish` enters native drawing, after backend validation.
pub struct Painter<'a, B: RenderBackend + ?Sized> {
    backend: &'a mut B,
    operations: Vec<DrawOp>,
}
impl<B: RenderBackend + ?Sized> Painter<'_, B> {
    pub fn draw(&mut self, operation: DrawOp) {
        self.operations.push(operation);
    }
    pub fn extend(&mut self, operations: &[DrawOp]) {
        self.operations.extend_from_slice(operations);
    }
    pub fn finish(self) -> Result<FrameStatus, B::Error> {
        self.backend.render(&self.operations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn segments(ops: &[DrawOp]) -> Vec<(Point, Point)> {
        ops.iter()
            .map(|op| match op {
                DrawOp::Line { from, to, .. } => (*from, *to),
                other => panic!("squiggle drew {other:?}"),
            })
            .collect()
    }
    #[test]
    fn squiggle_zigzags_under_the_whole_range_without_leaving_its_band() {
        let mut ops = Vec::new();
        squiggle(10.0, 20.0, 30.0, Color(0xFF0000), &mut ops);
        let lines = segments(&ops);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines.first().unwrap().0, Point { x: 10.0, y: 30.0 });
        assert_eq!(lines.last().unwrap().1.x, 20.0);
        for pair in lines.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "segments join into one wave");
            assert_ne!(pair[0].0.y, pair[0].1.y, "each segment slopes");
        }
        assert!(
            lines
                .iter()
                .all(|(a, b)| (28.0..=30.0).contains(&a.y) && (28.0..=30.0).contains(&b.y))
        );
    }
    #[test]
    fn squiggle_skips_empty_or_invalid_ranges_and_caps_huge_ones() {
        let mut ops = Vec::new();
        squiggle(5.0, 6.0, 10.0, Color(0), &mut ops);
        squiggle(f32::NAN, 60.0, 10.0, Color(0), &mut ops);
        squiggle(9.0, 3.0, 10.0, Color(0), &mut ops);
        assert!(ops.is_empty());
        squiggle(0.0, 1.0e7, 10.0, Color(0), &mut ops);
        assert_eq!(ops.len(), MAX_SQUIGGLE_SEGMENTS);
    }
}
