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
