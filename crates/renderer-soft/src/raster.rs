// SPDX-License-Identifier: MPL-2.0
//! The frame canvas: a tiny-skia pixmap with clip and layer stacks.
//!
//! Geometry arrives in DIPs and is multiplied by the scale here, as Direct2D
//! does with a target DPI of 96 x scale. Shapes are antialiased. Clips are
//! axis-aligned and each edge is rounded to the nearest pixel, which matches
//! Direct2D's antialiased clip for the whole-pixel rectangles the shell pushes.
//! A layer is a transparent pixmap covering its bounds within the current clip;
//! popping it composites it onto the layer below at its opacity.
use bareline_renderer::{Color, Image, Rect};
use tiny_skia::{
    BlendMode, FillRule, FilterQuality, IntSize, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Stroke, Transform,
};

/// Layer pixmaps kept for reuse between frames.
const MAX_POOLED_LAYERS: usize = 8;
/// Bezier handle length for a quarter circle, in radii.
const KAPPA: f32 = 0.552_284_8;

/// Whether a horizontal span from `left` to `right` (pixels) starts and ends
/// partway into pixels and covers no whole pixel between them: both ends lie
/// on either side of the same pixel edge.
fn straddles_one_pixel_edge(left: f32, right: f32) -> bool {
    left.fract() != 0.0 && right.fract() != 0.0 && left.ceil() == right.floor()
}

/// A half-open pixel rectangle in frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PxRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}
impl PxRect {
    const EMPTY: Self = Self {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };
    /// Each edge rounded to the nearest pixel; `as` saturates non-finite input.
    fn from_dips(r: Rect, scale: f32) -> Self {
        Self {
            x0: (r.x * scale).round() as i32,
            y0: (r.y * scale).round() as i32,
            x1: ((r.x + r.width) * scale).round() as i32,
            y1: ((r.y + r.height) * scale).round() as i32,
        }
    }
    fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }
    fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// What a push saved: the clip to restore on the matching pop.
enum Pushed {
    Clip(PxRect),
    Layer(PxRect),
}
struct Layer {
    /// `None` when the layer is entirely clipped away.
    pixmap: Option<Pixmap>,
    origin: (i32, i32),
    opacity: f32,
}
/// The coverage mask of the current clip on the current target, kept while
/// neither changes so partly clipped shapes do not rebuild it per operation.
struct ClipMask {
    clip: PxRect,
    origin: (i32, i32),
    size: (u32, u32),
    mask: Mask,
}

pub(crate) struct Canvas {
    base: Option<Pixmap>,
    layers: Vec<Layer>,
    pool: Vec<Pixmap>,
    stack: Vec<Pushed>,
    clip: PxRect,
    mask: Option<ClipMask>,
    scale: f32,
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    let [_, r, g, b] = color.0.to_be_bytes();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    paint
}
/// The pixmap drawing currently targets and its origin in frame pixels.
fn target<'a>(base: &'a mut Option<Pixmap>, layers: &'a mut [Layer]) -> Option<(&'a mut Pixmap, (i32, i32))> {
    match layers.last_mut() {
        Some(layer) => layer.pixmap.as_mut().map(|pixmap| (pixmap, layer.origin)),
        None => base.as_mut().map(|pixmap| (pixmap, (0, 0))),
    }
}
/// Source-over of a straight-alpha colour at `coverage` onto a premultiplied pixel.
#[inline]
fn blend(destination: &mut [u8], source: [u8; 4], coverage: u8) {
    let alpha = (u32::from(source[3]) * u32::from(coverage) + 127) / 255;
    if alpha == 0 {
        return;
    }
    let inverse = 255 - alpha;
    for (channel, value) in destination[..3].iter_mut().zip(&source[..3]) {
        *channel = ((u32::from(*value) * alpha + u32::from(*channel) * inverse + 127) / 255) as u8;
    }
    destination[3] = ((255 * alpha + u32::from(destination[3]) * inverse + 127) / 255) as u8;
}
/// Premultiplied copy of straight-alpha RGBA8 pixels.
fn premultiplied(pixels: &[u8]) -> Vec<u8> {
    let mut result = pixels.to_vec();
    for pixel in result.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    result
}
/// A closed rectangle path; zero-sized rectangles still stroke as lines.
pub(crate) fn rect_path(r: Rect) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(r.x, r.y);
    builder.line_to(r.x + r.width, r.y);
    builder.line_to(r.x + r.width, r.y + r.height);
    builder.line_to(r.x, r.y + r.height);
    builder.close();
    builder.finish()
}
/// A rectangle with quarter-circle corners; the radius is limited to half the
/// shorter side, as Direct2D does.
pub(crate) fn rounded_rect_path(r: Rect, radius: f32) -> Option<Path> {
    let radius = radius.min(r.width / 2.0).min(r.height / 2.0);
    if radius.is_nan() || radius <= 0.0 {
        return rect_path(r);
    }
    let (left, top, right, bottom) = (r.x, r.y, r.x + r.width, r.y + r.height);
    let handle = radius * KAPPA;
    let mut builder = PathBuilder::new();
    builder.move_to(left + radius, top);
    builder.line_to(right - radius, top);
    builder.cubic_to(
        right - radius + handle,
        top,
        right,
        top + radius - handle,
        right,
        top + radius,
    );
    builder.line_to(right, bottom - radius);
    builder.cubic_to(
        right,
        bottom - radius + handle,
        right - radius + handle,
        bottom,
        right - radius,
        bottom,
    );
    builder.line_to(left + radius, bottom);
    builder.cubic_to(
        left + radius - handle,
        bottom,
        left,
        bottom - radius + handle,
        left,
        bottom - radius,
    );
    builder.line_to(left, top + radius);
    builder.cubic_to(
        left,
        top + radius - handle,
        left + radius - handle,
        top,
        left + radius,
        top,
    );
    builder.close();
    builder.finish()
}
pub(crate) fn line_path(from: (f32, f32), to: (f32, f32)) -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(from.0, from.1);
    builder.line_to(to.0, to.1);
    builder.finish()
}

impl Canvas {
    pub(crate) fn new() -> Self {
        Self {
            base: None,
            layers: Vec::new(),
            pool: Vec::new(),
            stack: Vec::new(),
            clip: PxRect::EMPTY,
            mask: None,
            scale: 1.0,
        }
    }
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }
    /// The last frame's pixels: premultiplied RGBA8, row-major, no padding.
    pub(crate) fn pixmap(&self) -> Option<&Pixmap> {
        self.base.as_ref()
    }
    /// Start a frame of `size` pixels, cleared to transparent black. Returns
    /// false when the pixmap cannot be allocated.
    pub(crate) fn begin(&mut self, size: (u32, u32), scale: f32) -> bool {
        if self
            .base
            .as_ref()
            .is_none_or(|base| (base.width(), base.height()) != size)
        {
            self.base = Pixmap::new(size.0, size.1);
        }
        let Some(base) = &mut self.base else {
            return false;
        };
        base.fill(tiny_skia::Color::TRANSPARENT);
        // A frame abandoned midway (never with balanced operations) leaves no state.
        while let Some(layer) = self.layers.pop() {
            if let Some(pixmap) = layer.pixmap {
                self.recycle(pixmap);
            }
        }
        self.stack.clear();
        self.mask = None;
        self.scale = scale;
        self.clip = PxRect {
            x0: 0,
            y0: 0,
            x1: i32::try_from(size.0).unwrap_or(i32::MAX),
            y1: i32::try_from(size.1).unwrap_or(i32::MAX),
        };
        true
    }
    /// Whether any part of `r`, grown by `outset` DIPs, is inside the clip.
    pub(crate) fn visible(&self, r: Rect, outset: f32) -> bool {
        let s = self.scale;
        let clip = self.clip;
        !clip.is_empty()
            && (r.x - outset) * s < clip.x1 as f32
            && (r.y - outset) * s < clip.y1 as f32
            && (r.x + r.width + outset) * s > clip.x0 as f32
            && (r.y + r.height + outset) * s > clip.y0 as f32
    }
    pub(crate) fn push_clip(&mut self, r: Rect) {
        self.stack.push(Pushed::Clip(self.clip));
        self.clip = self.clip.intersect(PxRect::from_dips(r, self.scale));
    }
    pub(crate) fn pop_clip(&mut self) {
        if let Some(Pushed::Clip(previous)) = self.stack.pop() {
            self.clip = previous;
        }
    }
    pub(crate) fn push_layer(&mut self, bounds: Rect, opacity: f32) {
        let region = PxRect::from_dips(bounds, self.scale).intersect(self.clip);
        let pixmap = if region.is_empty() {
            None
        } else {
            self.take_pixmap(region.x1.abs_diff(region.x0), region.y1.abs_diff(region.y0))
        };
        let region = if pixmap.is_some() { region } else { PxRect::EMPTY };
        self.stack.push(Pushed::Layer(self.clip));
        self.layers.push(Layer {
            pixmap,
            origin: (region.x0, region.y0),
            opacity,
        });
        self.clip = region;
    }
    pub(crate) fn pop_layer(&mut self) {
        let Some(Pushed::Layer(previous)) = self.stack.pop() else {
            return;
        };
        self.clip = previous;
        let Some(layer) = self.layers.pop() else {
            return;
        };
        let Some(pixmap) = layer.pixmap else {
            return;
        };
        if layer.opacity > 0.0
            && let Some((below, origin)) = target(&mut self.base, &mut self.layers)
        {
            below.draw_pixmap(
                layer.origin.0 - origin.0,
                layer.origin.1 - origin.1,
                pixmap.as_ref(),
                &PixmapPaint {
                    opacity: layer.opacity,
                    blend_mode: BlendMode::SourceOver,
                    quality: FilterQuality::Nearest,
                },
                Transform::identity(),
                None,
            );
        }
        self.recycle(pixmap);
    }
    fn take_pixmap(&mut self, width: u32, height: u32) -> Option<Pixmap> {
        match self
            .pool
            .iter()
            .position(|pixmap| pixmap.width() == width && pixmap.height() == height)
        {
            Some(index) => {
                let mut pixmap = self.pool.swap_remove(index);
                pixmap.fill(tiny_skia::Color::TRANSPARENT);
                Some(pixmap)
            }
            None => Pixmap::new(width, height),
        }
    }
    fn recycle(&mut self, pixmap: Pixmap) {
        if self.pool.len() >= MAX_POOLED_LAYERS {
            self.pool.remove(0);
        }
        self.pool.push(pixmap);
    }
    /// Axis-aligned fills are clipped geometrically, so they never need a mask.
    pub(crate) fn fill_rect(&mut self, r: Rect, color: Color) {
        let (s, clip) = (self.scale, self.clip);
        let left = (r.x * s).max(clip.x0 as f32);
        let top = (r.y * s).max(clip.y0 as f32);
        let right = ((r.x + r.width) * s).min(clip.x1 as f32);
        let bottom = ((r.y + r.height) * s).min(clip.y1 as f32);
        if !(left < right && top < bottom) {
            return;
        }
        let Some((pixmap, origin)) = target(&mut self.base, &mut self.layers) else {
            return;
        };
        let (x, y) = (origin.0 as f32, origin.1 as f32);
        let Some(rect) = tiny_skia::Rect::from_ltrb(left - x, top - y, right - x, bottom - y) else {
            return;
        };
        if straddles_one_pixel_edge(rect.left(), rect.right()) {
            // tiny-skia's antialiased rectangle fill asserts (in debug builds) on
            // a rectangle whose sides both fall inside the two pixels around one
            // pixel edge, such as a thin caret at a fractional position; its path
            // rasterizer covers the same pixels without that assertion.
            pixmap.fill_path(
                &PathBuilder::from_rect(rect),
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        } else {
            pixmap.fill_rect(rect, &paint(color), Transform::identity(), None);
        }
    }
    pub(crate) fn fill_path(&mut self, path: &Path, color: Color) {
        self.clipped(path.bounds(), 0.0, |pixmap, transform, mask| {
            pixmap.fill_path(path, &paint(color), FillRule::Winding, transform, mask);
        });
    }
    /// Stroke centred on the path, `width` DIPs wide, with mitred joins.
    pub(crate) fn stroke_path(&mut self, path: &Path, color: Color, width: f32, cap: LineCap) {
        if !(width.is_finite() && width > 0.0) {
            return;
        }
        let stroke = Stroke {
            width,
            miter_limit: 10.0,
            line_cap: cap,
            line_join: LineJoin::Miter,
            dash: None,
        };
        self.clipped(path.bounds(), width, |pixmap, transform, mask| {
            pixmap.stroke_path(path, &paint(color), &stroke, transform, mask);
        });
    }
    /// Straight-alpha RGBA8 scaled bilinearly into `destination` at `opacity`.
    pub(crate) fn draw_image(&mut self, image: &Image, destination: Rect, opacity: f32) {
        if !(opacity > 0.0 && destination.width > 0.0 && destination.height > 0.0) {
            return;
        }
        let Some(bounds) =
            tiny_skia::Rect::from_xywh(destination.x, destination.y, destination.width, destination.height)
        else {
            return;
        };
        let Some(pixmap) = IntSize::from_wh(image.width(), image.height())
            .and_then(|size| Pixmap::from_vec(premultiplied(image.pixels()), size))
        else {
            return;
        };
        let placement = Transform::from_row(
            destination.width / image.width() as f32,
            0.0,
            0.0,
            destination.height / image.height() as f32,
            destination.x,
            destination.y,
        );
        let paint = PixmapPaint {
            opacity,
            blend_mode: BlendMode::SourceOver,
            quality: FilterQuality::Bilinear,
        };
        self.clipped(bounds, 0.0, |target, transform, mask| {
            target.draw_pixmap(0, 0, pixmap.as_ref(), &paint, transform.pre_concat(placement), mask);
        });
    }
    /// Run `draw` with the DIP-to-target transform, skipping shapes outside the
    /// clip and masking only shapes the clip cuts.
    fn clipped(
        &mut self,
        bounds: tiny_skia::Rect,
        outset: f32,
        draw: impl FnOnce(&mut Pixmap, Transform, Option<&Mask>),
    ) {
        let (s, clip) = (self.scale, self.clip);
        let left = (bounds.left() - outset) * s;
        let top = (bounds.top() - outset) * s;
        let right = (bounds.right() + outset) * s;
        let bottom = (bounds.bottom() + outset) * s;
        if clip.is_empty()
            || right <= clip.x0 as f32
            || bottom <= clip.y0 as f32
            || left >= clip.x1 as f32
            || top >= clip.y1 as f32
        {
            return;
        }
        let inside =
            left >= clip.x0 as f32 && top >= clip.y0 as f32 && right <= clip.x1 as f32 && bottom <= clip.y1 as f32;
        let Some((pixmap, origin)) = target(&mut self.base, &mut self.layers) else {
            return;
        };
        let transform = Transform::from_row(s, 0.0, 0.0, s, -origin.0 as f32, -origin.1 as f32);
        if inside {
            draw(pixmap, transform, None);
            return;
        }
        let size = (pixmap.width(), pixmap.height());
        if self
            .mask
            .as_ref()
            .is_none_or(|mask| mask.clip != clip || mask.origin != origin || mask.size != size)
        {
            self.mask = clip_mask(clip, origin, size);
        }
        if let Some(mask) = &self.mask {
            draw(pixmap, transform, Some(&mask.mask));
        }
    }
    /// Paint `color` through an 8-bit coverage bitmap whose top-left pixel is at
    /// (`x`, `y`) in frame pixels.
    pub(crate) fn blit_mask(&mut self, x: i32, y: i32, width: u32, height: u32, coverage: &[u8], color: Color) {
        let [_, r, g, b] = color.0.to_be_bytes();
        let source = [r, g, b, 255];
        if coverage.len() >= width as usize * height as usize {
            self.blit(x, y, width, height, |pixel, index| {
                blend(pixel, source, coverage[index])
            });
        }
    }
    /// Composite a straight-alpha RGBA8 bitmap (colour glyphs).
    pub(crate) fn blit_rgba(&mut self, x: i32, y: i32, width: u32, height: u32, rgba: &[u8]) {
        if rgba.len() >= width as usize * height as usize * 4 {
            self.blit(x, y, width, height, |pixel, index| {
                let at = index * 4;
                blend(pixel, [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]], 255);
            });
        }
    }
    /// Visit every clipped target pixel of a `width` x `height` bitmap at
    /// (`x`, `y`) with the bitmap's pixel index.
    fn blit(&mut self, x: i32, y: i32, width: u32, height: u32, mut pixel: impl FnMut(&mut [u8], usize)) {
        let clip = self.clip;
        let x0 = x.max(clip.x0);
        let y0 = y.max(clip.y0);
        let x1 = x.saturating_add_unsigned(width).min(clip.x1);
        let y1 = y.saturating_add_unsigned(height).min(clip.y1);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let Some((pixmap, origin)) = target(&mut self.base, &mut self.layers) else {
            return;
        };
        let stride = pixmap.width() as usize * 4;
        let data = pixmap.data_mut();
        // The clip always lies within the target, so every index is in bounds.
        for row in y0..y1 {
            let source = row.abs_diff(y) as usize * width as usize;
            let destination = row.abs_diff(origin.1) as usize * stride;
            for column in x0..x1 {
                let at = destination + column.abs_diff(origin.0) as usize * 4;
                pixel(&mut data[at..at + 4], source + column.abs_diff(x) as usize);
            }
        }
    }
}
fn clip_mask(clip: PxRect, origin: (i32, i32), size: (u32, u32)) -> Option<ClipMask> {
    let mut mask = Mask::new(size.0, size.1)?;
    let local = tiny_skia::Rect::from_ltrb(
        (clip.x0 - origin.0) as f32,
        (clip.y0 - origin.1) as f32,
        (clip.x1 - origin.0) as f32,
        (clip.y1 - origin.1) as f32,
    )?;
    mask.fill_path(
        &PathBuilder::from_rect(local),
        FillRule::Winding,
        false,
        Transform::identity(),
    );
    Some(ClipMask {
        clip,
        origin,
        size,
        mask,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }
    fn rgba(canvas: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let pixel = canvas.pixmap().unwrap().pixel(x, y).unwrap();
        [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
    }
    #[test]
    fn clip_edges_round_to_pixels_at_fractional_scales() {
        assert_eq!(
            PxRect::from_dips(rect(10.2, 0.0, 20.0, 10.0), 1.5),
            PxRect {
                x0: 15,
                y0: 0,
                x1: 45,
                y1: 15
            }
        );
        assert!(PxRect::from_dips(rect(f32::NAN, 0.0, 1.0, 1.0), 1.0).x1 >= 0);
    }
    #[test]
    fn thin_fills_across_one_pixel_edge_cover_both_pixels_partly() {
        assert!(straddles_one_pixel_edge(10.5, 11.7));
        assert!(!straddles_one_pixel_edge(10.0, 11.5));
        assert!(!straddles_one_pixel_edge(10.5, 12.5));
        assert!(!straddles_one_pixel_edge(10.2, 10.8));
        let mut canvas = Canvas::new();
        assert!(canvas.begin((8, 4), 1.0));
        // A 1.2 px caret at x = 2.5: tiny-skia's rectangle fill asserted here
        // in debug builds and aborted the editor.
        canvas.fill_rect(rect(2.5, 0.0, 1.2, 4.0), Color(0xFFFFFF));
        let (left, right) = (rgba(&canvas, 2, 1), rgba(&canvas, 3, 1));
        assert!((100..=155).contains(&left[0]), "{left:?}");
        assert!((150..=205).contains(&right[0]), "{right:?}");
        assert_eq!(rgba(&canvas, 1, 1)[3], 0);
        assert_eq!(rgba(&canvas, 4, 1)[3], 0);
    }
    #[test]
    fn partly_clipped_paths_are_masked_and_hidden_ones_skipped() {
        let mut canvas = Canvas::new();
        assert!(canvas.begin((40, 40), 1.0));
        canvas.push_clip(rect(0.0, 0.0, 20.0, 40.0));
        let circle = rounded_rect_path(rect(10.0, 10.0, 20.0, 20.0), 10.0).unwrap();
        canvas.fill_path(&circle, Color(0xFFFFFF));
        assert_eq!(rgba(&canvas, 15, 20), [255; 4]);
        assert_eq!(rgba(&canvas, 25, 20), [0; 4], "outside the clip");
        canvas.pop_clip();
        canvas.push_clip(rect(0.0, 0.0, 0.0, 0.0));
        canvas.fill_path(&circle, Color(0xFF0000));
        canvas.blit_mask(0, 0, 2, 2, &[255; 4], Color(0xFF0000));
        canvas.pop_clip();
        assert_eq!(rgba(&canvas, 15, 20), [255; 4]);
        assert_eq!(rgba(&canvas, 0, 0), [0; 4]);
    }
    #[test]
    fn coverage_blends_source_over_premultiplied_pixels() {
        let mut pixel = [0, 0, 0, 0];
        blend(&mut pixel, [255, 0, 0, 255], 128);
        assert_eq!(pixel, [128, 0, 0, 128]);
        blend(&mut pixel, [0, 0, 255, 255], 255);
        assert_eq!(pixel, [0, 0, 255, 255]);
        blend(&mut pixel, [255, 255, 255, 255], 0);
        assert_eq!(pixel, [0, 0, 255, 255]);
        assert_eq!(premultiplied(&[200, 100, 50, 128]), [100, 50, 25, 128]);
    }
}
