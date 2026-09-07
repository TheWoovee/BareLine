// SPDX-License-Identifier: MPL-2.0
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
    fn shape(&mut self, text: &str, size: f32, width: f32) -> Result<LayoutId, LayoutError>;
    /// Replace foreground styles without reshaping text or changing hit tests.
    /// Invalid ranges fail before changing the existing styles.
    fn set_styles(&mut self, layout: LayoutId, styles: &[TextStyle]) -> Result<(), LayoutError>;
    fn hit_test(&self, layout: LayoutId, point: Point) -> Result<TextHit, LayoutError>;
    /// UTF-8 byte offset. Returns leading caret bounds in logical pixels.
    fn caret(&self, layout: LayoutId, byte_offset: usize) -> Result<Rect, LayoutError>;
    fn range_rects(
        &self,
        layout: LayoutId,
        bytes: std::ops::Range<usize>,
    ) -> Result<Vec<Rect>, LayoutError>;
    fn release_layout(&mut self, layout: LayoutId);
}

/// Validate before entering a native draw frame. Prevents stack underflow at FFI boundaries.
pub fn balanced_clips(ops: &[DrawOp]) -> bool {
    let mut depth = 0usize;
    for op in ops {
        match op {
            DrawOp::PushClip(_) => depth += 1,
            DrawOp::PopClip if depth == 0 => return false,
            DrawOp::PopClip => depth -= 1,
            _ => {}
        }
    }
    depth == 0
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
}
