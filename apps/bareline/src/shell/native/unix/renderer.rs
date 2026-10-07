// SPDX-License-Identifier: MPL-2.0
//! The portable software renderer (PR-029, ADR-A): `bareline_renderer_soft`
//! paints the frame and softbuffer presents it in the editor window.
//!
//! The renderer is always software, so the hardware hand-over of the Windows
//! renderer (`defer_hardware`, `hardware_pending`, `take_init_failure`) has
//! nothing to do. System fonts are read once, when the renderer is created
//! (tens of milliseconds before the first frame); fonts installed later are
//! seen after a restart.
use super::{
    error::{Error, FAILED, INVALID_ARGUMENT, Result},
    platform::Platform,
};
use bareline_renderer::{
    DrawOp, FrameStatus, LayoutError, LayoutId, Point, Rect, RenderBackend, TextBackend, TextHit, TextStyle,
};
use bareline_renderer_soft::{SoftError, SoftRenderer};
use winit::window::Window;

/// The renderer the shell draws with.
pub struct Renderer {
    /// Always software here; read by the shell's diagnostics.
    pub software: bool,
    backend: SoftRenderer,
    /// `invalidate_device` was called: the next frame reports a lost surface.
    device_lost: bool,
}

/// The one place the shell's renderer is created. With the editor window it
/// presents there; without one it paints offscreen.
///
/// The window must outlive the renderer, as the handle given to
/// [`Platform::new`] must: the shell keeps both in fields that drop the
/// renderer first.
///
/// It is also the first point where the platform sees the window: the portal's
/// dialogs attach to it (Linux, X11) and the menu target joins its responder
/// chain (macOS).
pub fn create_renderer(platform: &Platform, window: Option<&Window>, _software: bool) -> Result<Renderer> {
    if let Some(window) = window {
        platform.attach_window(window);
    }
    renderer_for(window)
}
fn renderer_for(window: Option<&Window>) -> Result<Renderer> {
    let backend = match window {
        // SAFETY: the shell owns `window` in `Shell::window`, declared after
        // `Shell::renderer`, so this renderer is dropped before the window, and
        // it is never moved out of the shell (the contract `Platform::new`
        // documents for the window's handle).
        Some(window) => unsafe { SoftRenderer::for_borrowed_window(window) },
        None => SoftRenderer::offscreen(1, 1, 1.0),
    }
    .map_err(render_error)?;
    Ok(Renderer::new(backend))
}

/// Surface failures count as device failures; everything else is an invalid
/// argument, as Direct2D reports it.
fn render_error(error: SoftError) -> Error {
    let code = match error {
        SoftError::Surface(_) => FAILED,
        _ => INVALID_ARGUMENT,
    };
    Error::with_code(code, error.to_string())
}

impl Renderer {
    fn new(backend: SoftRenderer) -> Self {
        Self {
            software: true,
            backend,
            device_lost: false,
        }
    }
    /// A renderer without a window, for the visual baseline cells.
    #[cfg(test)]
    pub fn offscreen(width: u32, height: u32, scale: f32) -> Result<Self> {
        SoftRenderer::offscreen(width, height, scale)
            .map(Self::new)
            .map_err(render_error)
    }
    /// The last frame as top-down premultiplied BGRA rows, the layout the
    /// Windows renderer's offscreen bitmap has.
    #[cfg(test)]
    pub fn pixels_bgra(&self) -> Result<Vec<u8>> {
        let (_, _, rgba) = self
            .backend
            .frame_rgba()
            .ok_or_else(|| Error::with_code(INVALID_ARGUMENT, "No frame has been rendered"))?;
        Ok(rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[red, green, blue, alpha]| [blue, green, red, alpha])
            .collect())
    }
    pub fn take_init_failure(&mut self) -> Option<(i32, bool)> {
        None
    }
    pub fn defer_hardware(&mut self) {}
    pub fn hardware_pending(&self) -> bool {
        false
    }
    /// Fonts are read when the renderer is created; see the module notes.
    pub fn refresh_fonts(&mut self) {}
    /// The software renderer keeps no brushes.
    pub fn clear_brushes(&mut self) {}
    /// The software renderer bounds its own caches (see `bareline_renderer_soft`).
    pub fn set_layout_budget(&mut self, _editors: usize, _visible_rows: usize) {}
    /// Treat the surface as lost: the next frame reports `Recreate` and draws
    /// nothing, so the shell takes the path a real loss takes.
    pub fn invalidate_device(&mut self) {
        self.device_lost = true;
    }
}
impl RenderBackend for Renderer {
    type Error = Error;
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<()> {
        self.backend.resize(width, height, scale).map_err(render_error)
    }
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus> {
        if std::mem::take(&mut self.device_lost) {
            return Ok(FrameStatus::Recreate);
        }
        self.backend.render(operations).map_err(render_error)
    }
}
impl TextBackend for Renderer {
    fn measure_text(&mut self, text: &str, size: f32) -> std::result::Result<(f32, f32), LayoutError> {
        self.backend.measure_text(text, size)
    }
    fn shape_wrapped(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape_wrapped(text, size, width, family)
    }
    fn layout_size(&self, layout: LayoutId) -> std::result::Result<(f32, f32), LayoutError> {
        self.backend.layout_size(layout)
    }
    fn shape_with_font_family(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
        family: &str,
    ) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape_with_font_family(text, size, width, family)
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> std::result::Result<LayoutId, LayoutError> {
        self.backend.shape(text, size, width)
    }
    fn set_styles(&mut self, layout: LayoutId, styles: &[TextStyle]) -> std::result::Result<(), LayoutError> {
        self.backend.set_styles(layout, styles)
    }
    fn hit_test(&self, layout: LayoutId, point: Point) -> std::result::Result<TextHit, LayoutError> {
        self.backend.hit_test(layout, point)
    }
    fn caret(&self, layout: LayoutId, byte_offset: usize) -> std::result::Result<Rect, LayoutError> {
        self.backend.caret(layout, byte_offset)
    }
    fn range_rects(
        &self,
        layout: LayoutId,
        bytes: std::ops::Range<usize>,
    ) -> std::result::Result<Vec<Rect>, LayoutError> {
        self.backend.range_rects(layout, bytes)
    }
    fn release_layout(&mut self, layout: LayoutId) {
        self.backend.release_layout(layout);
    }
}

/// The editor font a profile uses until the user picks one: the monospace
/// face the renderer resolved from the installed fonts (Cascadia Mono when it
/// is installed, Menlo or SF Mono on macOS, else the bundled DejaVu Sans
/// Mono), so a fresh profile never names a face that is not there (LNX-UI-003).
pub fn default_font_family(renderer: &Renderer) -> Option<String> {
    Some(renderer.backend.monospace_family().to_owned())
}
/// An installed font family for the font picker.
pub struct InstalledFontFamily {
    pub name: String,
    pub monospace: bool,
}
/// The families the renderer can draw. It scans the system fonts, so the shell
/// calls it on a worker.
pub fn installed_font_families() -> Vec<InstalledFontFamily> {
    bareline_renderer_soft::installed_font_families()
        .into_iter()
        .map(|(name, monospace)| InstalledFontFamily { name, monospace })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_renderer::Color;

    #[test]
    fn offscreen_frames_are_captured_as_bgra() {
        let mut renderer = Renderer::offscreen(8, 4, 1.0).unwrap();
        assert!(renderer.pixels_bgra().is_err(), "nothing is captured before a frame");
        let fill = DrawOp::Fill(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 8.0,
                height: 4.0,
            },
            Color(0x11_22_33),
        );
        assert_eq!(renderer.render(&[fill]).unwrap(), FrameStatus::Presented);
        let pixels = renderer.pixels_bgra().unwrap();
        assert_eq!(pixels.len(), 8 * 4 * 4);
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0x33, 0x22, 0x11, 0xff])
        );
    }

    /// The caret is a thin fill at a fractional position; one such frame used
    /// to abort the editor in debug builds (a tiny-skia assertion).
    #[test]
    fn thin_fills_at_fractional_positions_are_drawn() {
        let mut renderer = Renderer::offscreen(8, 4, 1.0).unwrap();
        let caret = DrawOp::Fill(
            Rect {
                x: 2.5,
                y: 0.0,
                width: 1.2,
                height: 4.0,
            },
            Color(0xFF_FF_FF),
        );
        assert_eq!(renderer.render(&[caret]).unwrap(), FrameStatus::Presented);
        let pixels = renderer.pixels_bgra().unwrap();
        let alpha = |x: usize| pixels[(8 + x) * 4 + 3];
        assert!(alpha(2) > 0 && alpha(3) > 0, "both pixels are partly covered");
        assert_eq!((alpha(1), alpha(4)), (0, 0));
    }

    #[test]
    fn renderer_lays_out_text_and_reports_device_loss() {
        let mut renderer = renderer_for(None).unwrap();
        assert!(renderer.software && !renderer.hardware_pending());
        renderer.resize(320, 200, 1.0).unwrap();
        let layout = renderer.shape("Bareline", 14.0, 300.0).unwrap();
        assert!(renderer.layout_size(layout).unwrap().0 > 0.0);
        let caret = renderer.caret(layout, "Bare".len()).unwrap();
        assert!(caret.x > 0.0);
        assert_eq!(renderer.render(&[]).unwrap(), FrameStatus::Presented);
        renderer.invalidate_device();
        assert_eq!(renderer.render(&[]).unwrap(), FrameStatus::Recreate);
        assert_eq!(renderer.render(&[]).unwrap(), FrameStatus::Presented);
        renderer.release_layout(layout);
        assert!(renderer.layout_size(layout).is_err());
        // Invalid sizes are reported with the code Direct2D would give.
        let error = renderer.resize(10, 10, 0.0).unwrap_err();
        assert_eq!(error.code().0, INVALID_ARGUMENT);
    }

    #[test]
    fn the_font_picker_lists_the_bundled_monospace_face() {
        let families = installed_font_families();
        assert!(
            families
                .iter()
                .any(|family| family.name == bareline_renderer_soft::BUNDLED_FONT_FAMILY && family.monospace)
        );
    }
}
