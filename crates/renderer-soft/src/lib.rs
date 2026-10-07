// SPDX-License-Identifier: MPL-2.0
//! Portable software renderer (PR-029, ADR-A): [`RenderBackend`] and
//! [`TextBackend`] on pure-Rust dependencies, so Linux and macOS (and Windows,
//! as an alternative to Direct2D) can paint the shared UI.
//!
//! * Text: cosmic-text shapes, lays out, wraps and reorders bidirectional text
//!   with font fallback. System fonts come from fontdb; DejaVu Sans Mono is
//!   bundled and always loaded, so every family resolves to a real face.
//! * Pixels: tiny-skia rasterizes shapes and images; glyph bitmaps from swash
//!   are composited directly. Frames are premultiplied RGBA8.
//! * Presentation: softbuffer copies the frame to a winit window.
//!
//! # API the shell calls
//!
//! * [`SoftRenderer::for_window`] paints a window (and
//!   [`SoftRenderer::for_borrowed_window`] one the caller keeps owning);
//!   [`SoftRenderer::offscreen`] paints into memory (no display needed), read
//!   back with [`SoftRenderer::frame_rgba`] or saved with
//!   [`SoftRenderer::write_png`].
//! * [`installed_font_families`] lists the system families a renderer can use.
//! * [`RenderBackend::resize`] takes the size in physical pixels and the scale
//!   factor; [`RenderBackend::render`] draws a frame and presents it.
//! * [`TextBackend`] shapes and queries layouts, which live until
//!   [`TextBackend::release_layout`] and survive resizes and scale changes.
//!
//! # Conventions (those of the Windows Direct2D renderer)
//!
//! * Draw operations and text geometry are in DIPs; a pixel is DIPs times the
//!   scale. Shapes are antialiased; clip edges round to the nearest pixel.
//! * `Color(0xRRGGBB)` is opaque. `Stroke`, `StrokeRounded` and `Line` centre a
//!   stroke of the given DIP width on the outline; lines have flat caps.
//! * `Text` draws one unwrapped line in the default family (monospace from
//!   16 px, interface below), clipped to the window's right edge and to
//!   1.8 x size below its origin.
//! * `Layout` draws a shaped layout with its foreground styles; a layout this
//!   renderer did not create fails the frame, as does an unbalanced clip or
//!   layer stack (see [`bareline_renderer::balanced_clips`]).
//! * `PushLayer` composites everything drawn until `PopLayer` at the layer's
//!   opacity, clipped to its bounds.
//! * [`FrameStatus::Recreate`] means the window surface was lost and has been
//!   recreated: draw the frame again. Three losses in a row are an error.
//!
//! # Caches and limits
//!
//! * Layouts are shaped once and kept until released. `Text` and
//!   `measure_text` strings are shaped once per size and kept least recently
//!   used first out, 1024 to 2048 of them, plus whatever one frame draws.
//! * Glyph bitmaps are rasterized once per font, size and subpixel position.
//!   Past 8192 bitmaps, each frame keeps only those it drew.
//! * Text sizes above 2048 DIPs are refused; glyphs above 4096 pixels per em
//!   (2048 DIPs at a window scale above 2) are not drawn.
//!
//! Golden images for the offscreen tests live in `tests/golden`; see
//! `tests/golden.rs` for how to regenerate them.
use bareline_renderer::{
    Color, DrawOp, FrameStatus, LayoutError, LayoutId, MAX_LAYOUT_BYTES, MAX_LAYOUTS, Point, Rect, RenderBackend,
    TextBackend, TextHit, TextStyle, balanced_clips,
};
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
use std::sync::Arc;
use tiny_skia::LineCap;
use winit::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};
use winit::window::Window;

mod raster;
mod text;

pub use text::{BUNDLED_FONT_FAMILY, FontSource};

/// The font families a renderer with [`FontSource::System`] can draw, the
/// bundled face included, by name and whether they are monospaced. It scans
/// the installed fonts as creating a renderer does, so call it off the UI
/// thread.
pub fn installed_font_families() -> Vec<(String, bool)> {
    text::installed_families()
}

/// Largest offscreen surface, in pixels (the Windows offscreen limit).
const MAX_OFFSCREEN_PIXELS: u64 = 16 * 1024 * 1024;
/// Largest window surface, in pixels: an 8K display with room to spare.
const MAX_WINDOW_PIXELS: u64 = 64 * 1024 * 1024;
/// Consecutive lost surfaces after which a frame reports its error.
const MAX_RECREATE_STREAK: u32 = 3;
/// Shaped `DrawOp::Text` and `measure_text` strings kept: the cache trims
/// itself back to this many once it holds twice as many.
const MAX_LABELS: usize = 1024;

/// Why the software renderer could not create a surface or draw a frame.
#[derive(Debug)]
pub enum SoftError {
    /// The surface size or scale is zero, non-finite or too large.
    InvalidSize,
    /// The operations have unbalanced clips or layers, invalid layer or image
    /// geometry, or a text size that is not a positive number of at most
    /// 2048 DIPs.
    InvalidOperations,
    /// A `DrawOp::Layout` names a layout this renderer does not hold.
    Layout(LayoutError),
    /// The window system refused or lost the presentation surface.
    Surface(softbuffer::SoftBufferError),
}
impl std::fmt::Display for SoftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize => f.write_str("the drawing surface size is not supported"),
            Self::InvalidOperations => f.write_str("the frame's drawing operations are invalid"),
            Self::Layout(error) => write!(f, "{error}"),
            Self::Surface(error) => write!(f, "the window surface is unavailable: {error}"),
        }
    }
}
impl std::error::Error for SoftError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Surface(error) => Some(error),
            _ => None,
        }
    }
}

/// The window a presenter draws into: shared with the caller
/// ([`SoftRenderer::for_window`]) or borrowed through its raw handles
/// ([`SoftRenderer::for_borrowed_window`]).
#[derive(Clone)]
enum Target {
    Shared(Arc<Window>),
    Borrowed {
        display: RawDisplayHandle,
        window: RawWindowHandle,
    },
}
impl HasDisplayHandle for Target {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        match self {
            Self::Shared(window) => window.display_handle(),
            // SAFETY: the caller of `for_borrowed_window` keeps the window, and
            // with it its display connection, alive until the renderer that owns
            // every copy of these handles is dropped.
            Self::Borrowed { display, .. } => Ok(unsafe { DisplayHandle::borrow_raw(*display) }),
        }
    }
}
impl HasWindowHandle for Target {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        match self {
            Self::Shared(window) => window.window_handle(),
            // SAFETY: as above, the borrowed window outlives the renderer.
            Self::Borrowed { window, .. } => Ok(unsafe { WindowHandle::borrow_raw(*window) }),
        }
    }
}

/// A winit window's softbuffer surface.
struct Presenter {
    target: Target,
    // Declared before the context it was created from, so it drops first.
    surface: softbuffer::Surface<Target, Target>,
    _context: softbuffer::Context<Target>,
    /// Size the surface was last resized to.
    size: Option<(u32, u32)>,
    recreate_streak: u32,
}
impl Presenter {
    fn new(target: Target) -> Result<Self, softbuffer::SoftBufferError> {
        let context = softbuffer::Context::new(target.clone())?;
        let surface = softbuffer::Surface::new(&context, target.clone())?;
        Ok(Self {
            target,
            surface,
            _context: context,
            size: None,
            recreate_streak: 0,
        })
    }
    fn present(&mut self, frame: &tiny_skia::Pixmap) -> Result<FrameStatus, SoftError> {
        match self.copy_and_present(frame) {
            Ok(()) => {
                self.recreate_streak = 0;
                Ok(FrameStatus::Presented)
            }
            Err(error) => {
                self.recreate_streak += 1;
                if self.recreate_streak >= MAX_RECREATE_STREAK {
                    self.recreate_streak = 0;
                    return Err(SoftError::Surface(error));
                }
                // A lost surface is recreated and the frame drawn again.
                let streak = self.recreate_streak;
                *self = Self::new(self.target.clone()).map_err(SoftError::Surface)?;
                self.recreate_streak = streak;
                Ok(FrameStatus::Recreate)
            }
        }
    }
    fn copy_and_present(&mut self, frame: &tiny_skia::Pixmap) -> Result<(), softbuffer::SoftBufferError> {
        let size = (frame.width(), frame.height());
        if self.size != Some(size) {
            let (Some(width), Some(height)) = (NonZeroU32::new(size.0), NonZeroU32::new(size.1)) else {
                return Ok(());
            };
            self.surface.resize(width, height)?;
            self.size = Some(size);
        }
        let mut buffer = self.surface.buffer_mut()?;
        // softbuffer pixels are 0x00RRGGBB; frames are opaque wherever drawn.
        for (pixel, rgba) in buffer.iter_mut().zip(frame.data().as_chunks::<4>().0) {
            *pixel = (u32::from(rgba[0]) << 16) | (u32::from(rgba[1]) << 8) | u32::from(rgba[2]);
        }
        buffer.present()
    }
}

/// Shaped `DrawOp::Text` and `measure_text` strings by font size, least
/// recently used first out. Labels a frame draws are kept while it draws them.
struct Labels {
    by_size: HashMap<u32, HashMap<String, (text::Shaped, u64)>>,
    len: usize,
    /// Size at which the next insertion trims the cache.
    limit: usize,
    /// Use counter; every lookup through [`Labels::prepare`] advances it.
    tick: u64,
    /// First tick of the frame being prepared; `u64::MAX` otherwise.
    frame_start: u64,
    /// Scratch space for trimming.
    ticks: Vec<u64>,
}
impl Default for Labels {
    fn default() -> Self {
        Self {
            by_size: HashMap::new(),
            len: 0,
            limit: MAX_LABELS * 2,
            tick: 0,
            frame_start: u64::MAX,
            ticks: Vec::new(),
        }
    }
}
impl Labels {
    /// Shape `text` unless it is cached; marks it most recently used.
    fn prepare(&mut self, fonts: &mut text::Fonts, text: &str, size: f32) -> &text::Shaped {
        let text = &text[..text.floor_char_boundary(MAX_LAYOUT_BYTES)];
        self.tick += 1;
        let tick = self.tick;
        let cached = self
            .by_size
            .get(&size.to_bits())
            .is_some_and(|labels| labels.contains_key(text));
        if !cached {
            if self.len >= self.limit {
                self.trim();
            }
            let family = fonts.family(None, size);
            let shaped = text::shape(fonts, text, size, 1.0e9, &family, false);
            self.by_size
                .entry(size.to_bits())
                .or_default()
                .insert(text.to_owned(), (shaped, tick));
            self.len += 1;
        }
        let entry = self
            .by_size
            .get_mut(&size.to_bits())
            .and_then(|labels| labels.get_mut(text))
            .expect("the label was inserted above");
        entry.1 = tick;
        &entry.0
    }
    fn get(&self, text: &str, size: f32) -> Option<&text::Shaped> {
        let text = &text[..text.floor_char_boundary(MAX_LAYOUT_BYTES)];
        self.by_size.get(&size.to_bits())?.get(text).map(|(shaped, _)| shaped)
    }
    /// Keep the [`MAX_LABELS`] most recently used labels and every label the
    /// current frame uses. The next trim waits until the cache has doubled, so
    /// each insertion costs O(1) amortised.
    fn trim(&mut self) {
        self.ticks.clear();
        self.ticks
            .extend(self.by_size.values().flat_map(HashMap::values).map(|(_, used)| *used));
        if self.ticks.len() > MAX_LABELS {
            let index = self.ticks.len() - MAX_LABELS;
            let newest_kept = *self.ticks.select_nth_unstable(index).1;
            let keep_from = newest_kept.min(self.frame_start);
            self.by_size.retain(|_, labels| {
                labels.retain(|_, (_, used)| *used >= keep_from);
                !labels.is_empty()
            });
            self.len = self.by_size.values().map(HashMap::len).sum();
        }
        self.limit = (self.len * 2).max(MAX_LABELS * 2);
    }
    /// Shape the labels of a frame's `Text` operations, none of which a trim
    /// while preparing them can drop, so drawing finds every one.
    fn prepare_frame(&mut self, fonts: &mut text::Fonts, operations: &[DrawOp]) {
        self.frame_start = self.tick + 1;
        for op in operations {
            if let DrawOp::Text { text, size, .. } = op {
                self.prepare(fonts, text, *size);
            }
        }
        self.frame_start = u64::MAX;
    }
}

/// The portable software renderer. See the crate documentation.
pub struct SoftRenderer {
    fonts: text::Fonts,
    layouts: BTreeMap<LayoutId, text::Shaped>,
    labels: Labels,
    canvas: raster::Canvas,
    /// Surface size in pixels.
    size: (u32, u32),
    scale: f32,
    presenter: Option<Presenter>,
    rendered: bool,
}
impl SoftRenderer {
    fn new(size: (u32, u32), scale: f32, fonts: FontSource, presenter: Option<Presenter>) -> Self {
        Self {
            fonts: text::Fonts::new(fonts),
            layouts: BTreeMap::new(),
            labels: Labels::default(),
            canvas: raster::Canvas::new(),
            size,
            scale,
            presenter,
            rendered: false,
        }
    }
    /// Paint `window` through softbuffer, sized from its current inner size and
    /// scale factor, with system fonts. Call [`RenderBackend::resize`] when the
    /// window's size or scale changes.
    pub fn for_window(window: Arc<Window>) -> Result<Self, SoftError> {
        let (size, scale) = (window.inner_size(), window.scale_factor() as f32);
        Self::presenting(Target::Shared(window), size, scale)
    }
    /// [`SoftRenderer::for_window`] for a window the caller owns and keeps,
    /// for example in a field declared after the renderer's.
    ///
    /// # Safety
    ///
    /// `window` must outlive the returned renderer: the renderer keeps the
    /// window's raw display and window handles and presents through them on
    /// every frame, so drop it before the window.
    pub unsafe fn for_borrowed_window(window: &Window) -> Result<Self, SoftError> {
        let handle = |error| SoftError::Surface(softbuffer::SoftBufferError::RawWindowHandle(error));
        let target = Target::Borrowed {
            display: window.display_handle().map_err(handle)?.as_raw(),
            window: window.window_handle().map_err(handle)?.as_raw(),
        };
        Self::presenting(target, window.inner_size(), window.scale_factor() as f32)
    }
    fn presenting(target: Target, size: winit::dpi::PhysicalSize<u32>, scale: f32) -> Result<Self, SoftError> {
        let presenter = Presenter::new(target).map_err(SoftError::Surface)?;
        let mut renderer = Self::new((1, 1), 1.0, FontSource::System, Some(presenter));
        renderer.resize(size.width, size.height, scale)?;
        Ok(renderer)
    }
    /// Paint into memory only, with system fonts: `width` x `height` pixels
    /// (at most 16 Mi pixels) at a scale from 0.5 to 4.
    pub fn offscreen(width: u32, height: u32, scale: f32) -> Result<Self, SoftError> {
        Self::offscreen_with_fonts(width, height, scale, FontSource::System)
    }
    /// [`SoftRenderer::offscreen`] with a choice of fonts;
    /// [`FontSource::BundledOnly`] gives output independent of the machine.
    pub fn offscreen_with_fonts(width: u32, height: u32, scale: f32, fonts: FontSource) -> Result<Self, SoftError> {
        if width == 0
            || height == 0
            || u64::from(width) * u64::from(height) > MAX_OFFSCREEN_PIXELS
            || !scale.is_finite()
            || !(0.5..=4.0).contains(&scale)
        {
            return Err(SoftError::InvalidSize);
        }
        Ok(Self::new((width, height), scale, fonts, None))
    }
    /// The last rendered frame: width, height and premultiplied RGBA8 rows
    /// without padding (identical to straight RGBA wherever the frame is
    /// opaque). `None` before the first successful render.
    pub fn frame_rgba(&self) -> Option<(u32, u32, &[u8])> {
        let pixmap = self.canvas.pixmap().filter(|_| self.rendered)?;
        Some((pixmap.width(), pixmap.height(), pixmap.data()))
    }
    /// Save the last rendered frame as a straight-alpha RGBA PNG.
    pub fn write_png(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let pixmap = self
            .canvas
            .pixmap()
            .filter(|_| self.rendered)
            .ok_or_else(|| std::io::Error::other("no frame has been rendered"))?;
        let png = pixmap.encode_png().map_err(std::io::Error::other)?;
        std::fs::write(path, png)
    }
    /// Surface size in pixels and scale factor.
    pub fn size(&self) -> (u32, u32, f32) {
        (self.size.0, self.size.1, self.scale)
    }
    /// Live shaped layouts.
    pub fn layout_count(&self) -> usize {
        self.layouts.len()
    }
    /// The installed family the editor uses by default (after fallback).
    pub fn monospace_family(&self) -> &str {
        self.fonts.monospace()
    }
    fn shape_layout(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: Option<&str>,
        wrapped: bool,
    ) -> Result<LayoutId, LayoutError> {
        if text.len() > MAX_LAYOUT_BYTES
            || self.layouts.len() >= MAX_LAYOUTS
            || !valid_text_size(size)
            || !width.is_finite()
            || width <= 0.0
        {
            return Err(LayoutError::ResourceLimit);
        }
        let family = self.fonts.family(family, size);
        let shaped = text::shape(&mut self.fonts, text, size, width, &family, wrapped);
        let id = LayoutId::allocate();
        self.layouts.insert(id, shaped);
        Ok(id)
    }
}

/// Whether `size` (DIPs) is a text size this renderer shapes and draws.
fn valid_text_size(size: f32) -> bool {
    size.is_finite() && size > 0.0 && size <= text::MAX_TEXT_SIZE
}

/// Paint a shaped layout's glyphs with their top-left at `origin` (DIPs).
fn draw_glyphs(
    canvas: &mut raster::Canvas,
    fonts: &mut text::Fonts,
    shaped: &text::Shaped,
    origin: Point,
    color: Color,
) {
    let (width, height) = shaped.size();
    let bounds = Rect {
        x: origin.x,
        y: origin.y,
        width,
        height,
    };
    // Ink can overhang the layout box (accents, descenders, italics).
    if !canvas.visible(bounds, height.max(16.0)) {
        return;
    }
    let scale = canvas.scale();
    for glyph in shaped.glyphs() {
        let physical = glyph
            .layout
            .physical((origin.x * scale, (origin.y + glyph.baseline) * scale), scale);
        let Some(image) = fonts.glyph_image(physical.cache_key) else {
            continue;
        };
        let x = physical.x + image.placement.left;
        let y = physical.y - image.placement.top;
        let (w, h) = (image.placement.width, image.placement.height);
        match image.content {
            cosmic_text::SwashContent::Color => canvas.blit_rgba(x, y, w, h, &image.data),
            cosmic_text::SwashContent::Mask => {
                canvas.blit_mask(x, y, w, h, &image.data, shaped.color_at(glyph.start, color));
            }
            // swash produces these only for subpixel formats, which are not requested.
            cosmic_text::SwashContent::SubpixelMask => {}
        }
    }
}

/// What a frame draws from besides the canvas: fonts, layouts and labels.
struct Sources<'a> {
    fonts: &'a mut text::Fonts,
    layouts: &'a BTreeMap<LayoutId, text::Shaped>,
    labels: &'a Labels,
    /// Surface width in DIPs, the right edge of `Text` operations.
    width: f32,
}

fn draw_op(canvas: &mut raster::Canvas, sources: &mut Sources<'_>, op: &DrawOp) {
    match op {
        DrawOp::Fill(r, color) => canvas.fill_rect(*r, *color),
        DrawOp::Stroke(r, color, width) => {
            if let Some(path) = raster::rect_path(*r) {
                canvas.stroke_path(&path, *color, *width, LineCap::Butt);
            }
        }
        DrawOp::FillRounded(r, color, radius) => {
            if let Some(path) = raster::rounded_rect_path(*r, *radius) {
                canvas.fill_path(&path, *color);
            }
        }
        DrawOp::StrokeRounded(r, color, radius, width) => {
            if let Some(path) = raster::rounded_rect_path(*r, *radius) {
                canvas.stroke_path(&path, *color, *width, LineCap::Butt);
            }
        }
        DrawOp::Text {
            origin,
            text,
            size,
            color,
        } => {
            let Some(shaped) = sources.labels.get(text, *size) else {
                return;
            };
            canvas.push_clip(Rect {
                x: origin.x,
                y: origin.y,
                width: (sources.width - origin.x).max(0.0),
                height: size * 1.8,
            });
            draw_glyphs(canvas, sources.fonts, shaped, *origin, *color);
            canvas.pop_clip();
        }
        DrawOp::PushClip(r) => canvas.push_clip(*r),
        DrawOp::PopClip => canvas.pop_clip(),
        DrawOp::Image {
            image,
            destination,
            opacity,
        } => canvas.draw_image(image, *destination, *opacity),
        DrawOp::PushLayer { bounds, opacity } => canvas.push_layer(*bounds, *opacity),
        DrawOp::PopLayer => canvas.pop_layer(),
        DrawOp::Layout { origin, layout, color } => {
            if let Some(shaped) = sources.layouts.get(layout) {
                draw_glyphs(canvas, sources.fonts, shaped, *origin, *color);
            }
        }
        DrawOp::Line { from, to, color, width } => {
            if let Some(path) = raster::line_path((from.x, from.y), (to.x, to.y)) {
                canvas.stroke_path(&path, *color, *width, LineCap::Butt);
            }
        }
    }
}

impl RenderBackend for SoftRenderer {
    type Error = SoftError;
    /// `width` and `height` in physical pixels (zero counts as one).
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<(), SoftError> {
        let size = (width.max(1), height.max(1));
        if !scale.is_finite() || scale <= 0.0 || u64::from(size.0) * u64::from(size.1) > MAX_WINDOW_PIXELS {
            return Err(SoftError::InvalidSize);
        }
        self.size = size;
        self.scale = scale;
        Ok(())
    }
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus, SoftError> {
        let _frame_span = bareline_renderer::frame_span();
        if !balanced_clips(operations) {
            return Err(SoftError::InvalidOperations);
        }
        // Resolve everything fallible before drawing, so a failed frame draws nothing.
        for op in operations {
            match op {
                DrawOp::Layout { layout, .. } if !self.layouts.contains_key(layout) => {
                    return Err(SoftError::Layout(LayoutError::InvalidHandle));
                }
                DrawOp::Text { size, .. } if !valid_text_size(*size) => return Err(SoftError::InvalidOperations),
                _ => {}
            }
        }
        self.labels.prepare_frame(&mut self.fonts, operations);
        if !self.canvas.begin(self.size, self.scale) {
            return Err(SoftError::InvalidSize);
        }
        let mut sources = Sources {
            fonts: &mut self.fonts,
            layouts: &self.layouts,
            labels: &self.labels,
            width: self.size.0 as f32 / self.scale,
        };
        for op in operations {
            draw_op(&mut self.canvas, &mut sources, op);
        }
        self.fonts.trim();
        self.rendered = true;
        match (&mut self.presenter, self.canvas.pixmap()) {
            (Some(presenter), Some(frame)) => presenter.present(frame),
            _ => Ok(FrameStatus::Presented),
        }
    }
}

impl TextBackend for SoftRenderer {
    /// Shaped once per distinct string and size, and cached like `Text` labels.
    fn measure_text(&mut self, text: &str, size: f32) -> Result<(f32, f32), LayoutError> {
        if text.len() > MAX_LAYOUT_BYTES || !valid_text_size(size) {
            return Err(LayoutError::ResourceLimit);
        }
        Ok(self.labels.prepare(&mut self.fonts, text, size).size())
    }
    fn shape_wrapped(&mut self, text: &str, size: f32, width: f32, family: &str) -> Result<LayoutId, LayoutError> {
        if !bareline_renderer::valid_font_family(family) {
            return Err(LayoutError::InvalidOffset);
        }
        self.shape_layout(text, size, width, Some(family), true)
    }
    fn layout_size(&self, layout: LayoutId) -> Result<(f32, f32), LayoutError> {
        Ok(self.layouts.get(&layout).ok_or(LayoutError::InvalidHandle)?.size())
    }
    fn shape_with_font_family(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> Result<LayoutId, LayoutError> {
        if !bareline_renderer::valid_font_family(family) {
            return Err(LayoutError::InvalidOffset);
        }
        self.shape_layout(text, size, width, Some(family), false)
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> Result<LayoutId, LayoutError> {
        self.shape_layout(text, size, width, None, false)
    }
    fn set_styles(&mut self, layout: LayoutId, styles: &[TextStyle]) -> Result<(), LayoutError> {
        self.layouts
            .get_mut(&layout)
            .ok_or(LayoutError::InvalidHandle)?
            .set_styles(styles)
    }
    fn hit_test(&self, layout: LayoutId, point: Point) -> Result<TextHit, LayoutError> {
        Ok(self
            .layouts
            .get(&layout)
            .ok_or(LayoutError::InvalidHandle)?
            .hit_test(point))
    }
    fn caret(&self, layout: LayoutId, byte_offset: usize) -> Result<Rect, LayoutError> {
        self.layouts
            .get(&layout)
            .ok_or(LayoutError::InvalidHandle)?
            .caret(byte_offset)
    }
    fn range_rects(&self, layout: LayoutId, bytes: std::ops::Range<usize>) -> Result<Vec<Rect>, LayoutError> {
        if bytes.start > bytes.end {
            return Err(LayoutError::InvalidOffset);
        }
        self.layouts
            .get(&layout)
            .ok_or(LayoutError::InvalidHandle)?
            .range_rects(bytes)
    }
    fn release_layout(&mut self, layout: LayoutId) {
        self.layouts.remove(&layout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_labels_and_layouts_are_never_reshaped() {
        let mut renderer = SoftRenderer::offscreen_with_fonts(320, 120, 1.0, FontSource::BundledOnly).unwrap();
        let id = renderer.shape("let x = 1;", 16.0, 300.0).unwrap();
        let mut ops = bareline_ui::shell(320.0, 120.0, &["Untitled".into()], 0, false);
        ops.push(DrawOp::Layout {
            origin: Point { x: 60.0, y: 40.0 },
            layout: id,
            color: Color(0xFFFFFF),
        });
        renderer.render(&ops).unwrap();
        let shaped = renderer.fonts.shaped_paragraphs;
        let first = renderer.frame_rgba().unwrap().2.to_vec();
        renderer
            .set_styles(
                id,
                &[TextStyle {
                    bytes: 0..3,
                    color: Color(0xFF0000),
                }],
            )
            .unwrap();
        renderer.render(&ops).unwrap();
        renderer.measure_text("Untitled", 13.0).unwrap();
        assert_eq!(
            renderer.fonts.shaped_paragraphs, shaped,
            "styles and redraws reuse shaping"
        );
        assert_ne!(renderer.frame_rgba().unwrap().2, first.as_slice());
    }
    #[test]
    fn labels_stay_bounded_and_keep_the_most_recently_used() {
        let mut labels = Labels::default();
        let mut fonts = text::Fonts::new(FontSource::BundledOnly);
        let count = MAX_LABELS * 5;
        for index in 0..count {
            labels.prepare(&mut fonts, &index.to_string(), 12.0);
            labels.prepare(&mut fonts, "kept", 12.0);
            assert!(labels.len <= MAX_LABELS * 2, "measuring without frames stays bounded");
        }
        assert!(labels.get("kept", 12.0).is_some());
        assert!(labels.get(&(count - 1).to_string(), 12.0).is_some());
        assert!(labels.get("0", 12.0).is_none());
    }
    #[test]
    fn a_frame_keeps_every_label_it_draws_even_past_the_cap() {
        let mut renderer = SoftRenderer::offscreen_with_fonts(64, 32, 1.0, FontSource::BundledOnly).unwrap();
        for index in 0..MAX_LABELS * 2 - 1 {
            renderer.measure_text(&format!("m{index}"), 12.0).unwrap();
        }
        let labels = MAX_LABELS + 10;
        let ops: Vec<DrawOp> = (0..labels)
            .map(|index| DrawOp::Text {
                origin: Point { x: 0.0, y: 100.0 },
                text: format!("t{index}"),
                size: 12.0,
                color: Color(0xFFFFFF),
            })
            .collect();
        renderer.render(&ops).unwrap();
        assert!((0..labels).all(|index| renderer.labels.get(&format!("t{index}"), 12.0).is_some()));
        assert!(renderer.labels.get("m0", 12.0).is_none());
    }
    #[test]
    fn the_glyph_cache_keeps_what_the_last_frame_drew() {
        let mut renderer = SoftRenderer::offscreen_with_fonts(200, 40, 1.0, FontSource::BundledOnly).unwrap();
        renderer.fonts.glyph_cap = 4;
        let label = |text: &str| DrawOp::Text {
            origin: Point { x: 4.0, y: 4.0 },
            text: text.into(),
            size: 12.0,
            color: Color(0xFFFFFF),
        };
        renderer.render(&[label("abcdefgh")]).unwrap();
        // Over the cap, but every bitmap was drawn: nothing is rasterized again.
        assert_eq!(renderer.fonts.glyph_images(), 8);
        renderer.render(&[label("xyz")]).unwrap();
        assert_eq!(renderer.fonts.glyph_images(), 3);
    }
    #[test]
    fn installed_families_include_the_bundled_monospace_face_once() {
        let families = installed_font_families();
        let bundled: Vec<_> = families
            .iter()
            .filter(|(name, _)| name == BUNDLED_FONT_FAMILY)
            .collect();
        assert_eq!(bundled, [&(BUNDLED_FONT_FAMILY.to_owned(), true)]);
        assert!(
            families.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "sorted and unique"
        );
    }
    #[test]
    fn glyphs_beyond_the_pixel_cap_are_skipped() {
        let mut renderer = SoftRenderer::offscreen_with_fonts(64, 64, 4.0, FontSource::BundledOnly).unwrap();
        let label = |size: f32| DrawOp::Text {
            origin: Point { x: 0.0, y: 0.0 },
            text: "x".into(),
            size,
            color: Color(0xFFFFFF),
        };
        renderer.render(&[label(text::MAX_TEXT_SIZE)]).unwrap();
        assert_eq!(renderer.fonts.glyph_images(), 0, "8192 px per em is not rasterized");
        renderer.render(&[label(4.0)]).unwrap();
        assert_eq!(renderer.fonts.glyph_images(), 1);
    }
}
