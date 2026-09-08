// SPDX-License-Identifier: MPL-2.0
use bareline_renderer::{Color, DrawOp, FrameStatus, Rect, RenderBackend};
use bareline_renderer::{
    LayoutError, LayoutId, MAX_LAYOUT_BYTES, MAX_LAYOUTS, Point, TextBackend, TextHit,
    balanced_clips,
};
use std::collections::BTreeMap;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::*,
            Direct3D11::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{Interface, w},
};
fn color(value: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((value.0 >> 16) & 255) as f32 / 255.0,
        g: ((value.0 >> 8) & 255) as f32 / 255.0,
        b: (value.0 & 255) as f32 / 255.0,
        a: 1.0,
    }
}
fn rectangle(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.width,
        bottom: r.y + r.height,
    }
}

pub struct WindowsRenderer {
    hwnd: HWND,
    factory: ID2D1Factory1,
    write: IDWriteFactory,
    target: Option<ID2D1RenderTarget>,
    surface: Option<Surface>,
    formats: BTreeMap<(String, u32), IDWriteTextFormat>,
    font_family: Option<String>,
    brushes: BTreeMap<u32, ID2D1SolidColorBrush>,
    layouts: BTreeMap<LayoutId, ShapedLine>,
    size: (u32, u32),
    scale: f32,
    pub software: bool,
    init_failure: Option<(i32, bool)>,
    // Last field: COM resources above must drop before the apartment guard.
    #[cfg(feature = "offscreen")]
    apartment: Option<Apartment>,
}
impl WindowsRenderer {
    pub(crate) fn new(hwnd: HWND, software: bool) -> windows::core::Result<Self> {
        // SAFETY: single-threaded factories used only by the owning UI thread.
        unsafe {
            Ok(Self {
                hwnd,
                factory: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                write: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
                target: None,
                surface: None,
                formats: BTreeMap::new(),
                font_family: None,
                brushes: BTreeMap::new(),
                layouts: BTreeMap::new(),
                size: (1, 1),
                scale: 1.0,
                software,
                init_failure: None,
                #[cfg(feature = "offscreen")]
                apartment: None,
            })
        }
    }
    fn create_target(&mut self) -> windows::core::Result<()> {
        #[cfg(feature = "offscreen")]
        if self.apartment.is_some() {
            return self.create_bitmap();
        }
        if !self.software {
            match self.create_hardware() {
                Ok(surface) => {
                    self.target = Some(surface.context.cast()?);
                    self.surface = Some(Surface::Hardware(surface));
                    return Ok(());
                }
                Err(error) => {
                    self.init_failure = Some((error.code().0, false));
                    eprintln!("event=hardware_fallback code={}", error.code().0);
                    self.software = true;
                }
            }
        }
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0 * self.scale,
            dpiY: 96.0 * self.scale,
            ..Default::default()
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: D2D_SIZE_U {
                width: self.size.0,
                height: self.size.1,
            },
            ..Default::default()
        };
        // SAFETY: valid HWND and initialized property structures; result owns its resources.
        let target = unsafe {
            self.factory
                .CreateHwndRenderTarget(&properties, &hwnd_properties)
        };
        let target = target?;
        self.target = Some(target.cast()?);
        self.surface = Some(Surface::Software(target));
        Ok(())
    }
    /// Actual HRESULT and attempted software mode, retained across fallback.
    pub fn take_init_failure(&mut self) -> Option<(i32, bool)> {
        self.init_failure.take()
    }
    fn create_hardware(&self) -> windows::core::Result<HardwareSurface> {
        // SAFETY: device/context/swap chain belong to this UI thread and live HWND.
        unsafe {
            let mut device = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: self.size.0,
                Height: self.size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForHwnd(&device, self.hwnd, &desc, None, None)?;
            factory.MakeWindowAssociation(self.hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            let d2d = self.factory.CreateDevice(&dxgi)?;
            let context = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let surface = HardwareSurface { context, swap };
            surface.bind(self.scale)?;
            Ok(surface)
        }
    }
    fn brush(&mut self, value: Color) -> windows::core::Result<ID2D1SolidColorBrush> {
        if let Some(brush) = self.brushes.get(&value.0) {
            return Ok(brush.clone());
        }
        let brush = unsafe {
            self.target
                .as_ref()
                .unwrap()
                .CreateSolidColorBrush(&color(value), None)?
        };
        self.brushes.insert(value.0, brush.clone());
        Ok(brush)
    }
    fn format(&mut self, size: f32) -> windows::core::Result<IDWriteTextFormat> {
        let name = self.font_family.as_deref().unwrap_or(if size >= 16.0 { "Cascadia Mono" } else { "Segoe UI" }).to_owned();
        let key = (name.clone(), size.to_bits());
        if let Some(format) = self.formats.get(&key) {
            return Ok(format.clone());
        }
        if self.formats.len() >= MAX_LAYOUTS {
            return Err(windows::core::Error::from_hresult(E_OUTOFMEMORY));
        }
        let family: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let format = unsafe {
            self.write.CreateTextFormat(
                windows::core::PCWSTR(family.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-US"),
            )?
        };
        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        }
        self.formats.insert(key, format.clone());
        Ok(format)
    }
    pub fn invalidate_device(&mut self) {
        self.brushes.clear();
        self.target = None;
        self.surface = None;
    }
}
impl RenderBackend for WindowsRenderer {
    type Error = windows::core::Error;
    fn resize(&mut self, width: u32, height: u32, scale: f32) -> Result<(), Self::Error> {
        let size = (width.max(1), height.max(1));
        if self.size == size && self.scale == scale {
            return Ok(());
        }
        let size_changed = self.size != size;
        self.size = size;
        self.scale = scale;
        #[cfg(feature = "offscreen")]
        if self.apartment.is_some() && size_changed {
            self.invalidate_device();
            return Ok(());
        }
        if let Some(target) = &self.target {
            unsafe {
                target.SetDpi(96.0 * scale, 96.0 * scale);
                let result = match self.surface.as_ref() {
                    Some(Surface::Software(hwnd)) if size_changed => hwnd.Resize(&D2D_SIZE_U {
                        width: size.0,
                        height: size.1,
                    }),
                    Some(Surface::Hardware(hw)) if size_changed => hw.resize(size, scale),
                    _ => Ok(()),
                };
                if result.is_err() {
                    self.invalidate_device();
                }
            }
        }
        Ok(())
    }
    fn render(&mut self, operations: &[DrawOp]) -> Result<FrameStatus, Self::Error> {
        let _frame_span = bareline_renderer::frame_span();
        if !balanced_clips(operations) {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        if self.target.is_none() {
            self.create_target()?;
        }
        // Resolve fallible resources before BeginDraw so error paths cannot leave an open frame.
        for op in operations {
            match op {
                DrawOp::Fill(_, c)
                | DrawOp::Stroke(_, c, _)
                | DrawOp::FillRounded(_, c, _)
                | DrawOp::StrokeRounded(_, c, _, _) => {
                    self.brush(*c)?;
                }
                DrawOp::Text { color, size, .. } => {
                    self.brush(*color)?;
                    self.format(*size)?;
                }
                DrawOp::Layout { layout, color, .. } => {
                    if !self.layouts.contains_key(layout) {
                        return Err(windows::core::Error::from_hresult(E_INVALIDARG));
                    }
                    self.brush(*color)?;
                    let line = &self.layouts[layout];
                    let native = line.layout.clone();
                    let length = line.boundaries.last().unwrap().1;
                    let styles = line.styles.clone();
                    // Reapply target-owned brushes after device recreation too.
                    unsafe {
                        native.SetDrawingEffect(
                            None::<&windows::core::IUnknown>,
                            DWRITE_TEXT_RANGE {
                                startPosition: 0,
                                length,
                            },
                        )?;
                    }
                    for (start, end, color) in styles {
                        let brush = self.brush(color)?;
                        unsafe {
                            native.SetDrawingEffect(
                                &brush,
                                DWRITE_TEXT_RANGE {
                                    startPosition: start,
                                    length: end - start,
                                },
                            )?;
                        }
                    }
                }
                DrawOp::Line { color, .. } => {
                    self.brush(*color)?;
                }
                _ => {}
            }
        }
        let target = self.target.as_ref().unwrap();
        // Device-dependent images live only for this frame and are recreated after loss.
        let mut images = BTreeMap::new();
        for (index, op) in operations.iter().enumerate() {
            if let DrawOp::Image { image, .. } = op {
                let mut pixels = image.pixels().to_vec();
                for pixel in pixels.chunks_exact_mut(4) {
                    let alpha = u16::from(pixel[3]);
                    for channel in &mut pixel[..3] {
                        *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
                    }
                    pixel.swap(0, 2);
                }
                let bitmap = unsafe { target.CreateBitmap(
                    D2D_SIZE_U { width: image.width(), height: image.height() },
                    Some(pixels.as_ptr().cast()), image.width() * 4,
                    &D2D1_BITMAP_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                        dpiX: 96.0, dpiY: 96.0,
                    },
                )? };
                images.insert(index, bitmap);
            }
        }
        // SAFETY: cached resources belong to this target; all calls occur on its owner thread.
        unsafe {
            target.BeginDraw();
            for (index, op) in operations.iter().enumerate() {
                match op {
                    DrawOp::Fill(r, c) => target.FillRectangle(&rectangle(*r), &self.brushes[&c.0]),
                    DrawOp::FillRounded(r, c, radius) => target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rectangle(*r),
                            radiusX: *radius,
                            radiusY: *radius,
                        },
                        &self.brushes[&c.0],
                    ),
                    DrawOp::StrokeRounded(r, c, radius, width) => target.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rectangle(*r),
                            radiusX: *radius,
                            radiusY: *radius,
                        },
                        &self.brushes[&c.0],
                        *width,
                        None,
                    ),
                    DrawOp::Stroke(r, c, width) => {
                        target.DrawRectangle(&rectangle(*r), &self.brushes[&c.0], *width, None)
                    }
                    DrawOp::Text {
                        origin,
                        text,
                        size,
                        color,
                    } => {
                        let bounds = D2D_RECT_F {
                            left: origin.x,
                            top: origin.y,
                            right: self.size.0 as f32 / self.scale,
                            bottom: origin.y + size * 1.8,
                        };
                        target.DrawText(
                            &text.encode_utf16().collect::<Vec<_>>(),
                            &self.formats[&(if *size >= 16.0 { "Cascadia Mono" } else { "Segoe UI" }.to_owned(), size.to_bits())],
                            &bounds,
                            &self.brushes[&color.0],
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    DrawOp::PushClip(r) => target
                        .PushAxisAlignedClip(&rectangle(*r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE),
                    DrawOp::PopClip => target.PopAxisAlignedClip(),
                    DrawOp::Image { destination, opacity, .. } => target.DrawBitmap(
                        &images[&index], Some(&rectangle(*destination)), *opacity,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None,
                    ),
                    DrawOp::PushLayer { bounds, opacity } => target.PushLayer(
                        &D2D1_LAYER_PARAMETERS {
                            contentBounds: rectangle(*bounds),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            maskTransform: windows_numerics::Matrix3x2::identity(),
                            opacity: *opacity,
                            ..Default::default()
                        }, None::<&ID2D1Layer>,
                    ),
                    DrawOp::PopLayer => target.PopLayer(),
                    DrawOp::Layout {
                        origin,
                        layout,
                        color,
                    } => target.DrawTextLayout(
                        vector(*origin),
                        &self.layouts[layout].layout,
                        &self.brushes[&color.0],
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    ),
                    DrawOp::Line {
                        from,
                        to,
                        color,
                        width,
                    } => target.DrawLine(
                        vector(*from),
                        vector(*to),
                        &self.brushes[&color.0],
                        *width,
                        None,
                    ),
                }
            }
            if let Err(error) = target.EndDraw(None, None) {
                self.invalidate_device();
                if error.code().0 == 0x8899000cu32 as i32 {
                    return Ok(FrameStatus::Recreate);
                }
                return Err(error);
            }
            if let Some(Surface::Hardware(hw)) = &self.surface
                && let Err(error) = hw.swap.Present(1, DXGI_PRESENT(0)).ok()
            {
                self.invalidate_device();
                if [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET].contains(&error.code()) {
                    return Ok(FrameStatus::Recreate);
                }
                return Err(error);
            }
        }
        Ok(FrameStatus::Presented)
    }
}

// DirectWrite positions are UTF-16; the neutral editor contract is always UTF-8 bytes.
struct ShapedLine {
    layout: IDWriteTextLayout,
    boundaries: Vec<(usize, u32)>,
    styles: Vec<(u32, u32, Color)>,
}
impl ShapedLine {
    fn byte_to_utf16(&self, byte: usize) -> Result<u32, LayoutError> {
        self.boundaries
            .binary_search_by_key(&byte, |&(b, _)| b)
            .map(|i| self.boundaries[i].1)
            .map_err(|_| LayoutError::InvalidOffset)
    }
    fn utf16_to_byte(&self, offset: u32) -> usize {
        let i = self
            .boundaries
            .partition_point(|&(_, u)| u <= offset)
            .saturating_sub(1);
        self.boundaries[i].0
    }
}
fn vector(point: Point) -> windows_numerics::Vector2 {
    windows_numerics::Vector2 {
        X: point.x,
        Y: point.y,
    }
}
impl TextBackend for WindowsRenderer {
    fn shape_wrapped(&mut self, text: &str, size: f32, width: f32, family: &str) -> Result<LayoutId, LayoutError> {
        let id = self.shape_with_font_family(text, size, width, family)?;
        let result = unsafe {
            self.layouts[&id].layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)
                .and_then(|_| self.layouts[&id].layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, size*1.2, size*0.9))
                .and_then(|_| self.layouts[&id].layout.SetMaxHeight(1.0e9))
        };
        if result.is_err() { self.release_layout(id); return Err(LayoutError::BackendFailure); }
        Ok(id)
    }
    fn layout_size(&self, id: LayoutId) -> Result<(f32, f32), LayoutError> {
        let line = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { line.layout.GetMetrics(&mut metrics) }.map_err(|_| LayoutError::BackendFailure)?;
        Ok((metrics.widthIncludingTrailingWhitespace, metrics.height))
    }
    fn shape_with_font_family(&mut self, text: &str, size: f32, width: f32, family: &str) -> Result<LayoutId, LayoutError> {
        if !bareline_renderer::valid_font_family(family) { return Err(LayoutError::InvalidOffset); }
        self.font_family = Some(family.to_owned());
        let result = self.shape(text, size, width);
        self.font_family = None;
        result
    }
    fn set_styles(
        &mut self,
        id: LayoutId,
        styles: &[bareline_renderer::TextStyle],
    ) -> Result<(), LayoutError> {
        let line = self
            .layouts
            .get_mut(&id)
            .ok_or(LayoutError::InvalidHandle)?;
        let mut mapped = Vec::with_capacity(styles.len());
        let mut previous = 0;
        for style in styles {
            if style.bytes.start < previous || style.bytes.start >= style.bytes.end {
                return Err(LayoutError::InvalidOffset);
            }
            mapped.push((
                line.byte_to_utf16(style.bytes.start)?,
                line.byte_to_utf16(style.bytes.end)?,
                style.color,
            ));
            previous = style.bytes.end;
        }
        line.styles = mapped;
        Ok(())
    }
    fn shape(&mut self, text: &str, size: f32, width: f32) -> Result<LayoutId, LayoutError> {
        if text.len() > MAX_LAYOUT_BYTES
            || self.layouts.len() >= MAX_LAYOUTS
            || !size.is_finite()
            || size <= 0.0
            || !width.is_finite()
            || width <= 0.0
        {
            return Err(LayoutError::ResourceLimit);
        }
        let format = self.format(size).map_err(|_| LayoutError::BackendFailure)?;
        let utf16: Vec<_> = text.encode_utf16().collect();
        // SAFETY: input slice is valid for the call; DirectWrite owns the resulting text copy.
        let layout = unsafe {
            self.write
                .CreateTextLayout(&utf16, &format, width, size * 2.0)
        }
        .map_err(|_| LayoutError::BackendFailure)?;
        let mut units = 0;
        let mut boundaries = Vec::with_capacity(text.chars().count() + 1);
        for (byte, c) in text.char_indices() {
            boundaries.push((byte, units));
            units += c.len_utf16() as u32;
        }
        boundaries.push((text.len(), units));
        let id = LayoutId::allocate();
        self.layouts.insert(
            id,
            ShapedLine {
                layout,
                boundaries,
                styles: Vec::new(),
            },
        );
        Ok(id)
    }
    fn hit_test(&self, id: LayoutId, point: Point) -> Result<TextHit, LayoutError> {
        let line = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let mut trailing = windows::core::BOOL(0);
        let mut inside = windows::core::BOOL(0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            line.layout
                .HitTestPoint(point.x, point.y, &mut trailing, &mut inside, &mut metrics)
        }
        .map_err(|_| LayoutError::BackendFailure)?;
        let position = metrics.textPosition
            + if trailing.as_bool() {
                metrics.length
            } else {
                0
            };
        Ok(TextHit {
            byte_offset: line.utf16_to_byte(position),
            inside: inside.as_bool(),
            trailing: trailing.as_bool(),
        })
    }
    fn caret(&self, id: LayoutId, byte_offset: usize) -> Result<Rect, LayoutError> {
        let line = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let position = line.byte_to_utf16(byte_offset)?;
        let mut x = 0.0;
        let mut y = 0.0;
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            line.layout
                .HitTestTextPosition(position, false, &mut x, &mut y, &mut metrics)
        }
        .map_err(|_| LayoutError::BackendFailure)?;
        Ok(Rect {
            x,
            y,
            width: 1.5,
            height: metrics.height,
        })
    }
    fn release_layout(&mut self, id: LayoutId) {
        self.layouts.remove(&id);
    }
    fn range_rects(
        &self,
        id: LayoutId,
        bytes: std::ops::Range<usize>,
    ) -> Result<Vec<Rect>, LayoutError> {
        if bytes.start > bytes.end {
            return Err(LayoutError::InvalidOffset);
        }
        let line = self.layouts.get(&id).ok_or(LayoutError::InvalidHandle)?;
        let start = line.byte_to_utf16(bytes.start)?;
        let end = line.byte_to_utf16(bytes.end)?;
        if start == end {
            return Ok(Vec::new());
        }
        let mut count = 0;
        // First call reports the bounded count even when the empty buffer is insufficient.
        let _ = unsafe {
            line.layout
                .HitTestTextRange(start, end - start, 0.0, 0.0, None, &mut count)
        };
        if count == 0 || count > MAX_LAYOUT_BYTES as u32 {
            return Err(LayoutError::BackendFailure);
        }
        let mut metrics = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        unsafe {
            line.layout.HitTestTextRange(
                start,
                end - start,
                0.0,
                0.0,
                Some(&mut metrics),
                &mut count,
            )
        }
        .map_err(|_| LayoutError::BackendFailure)?;
        Ok(metrics
            .into_iter()
            .take(count as usize)
            .map(|m| Rect {
                x: m.left,
                y: m.top,
                width: m.width,
                height: m.height,
            })
            .collect())
    }
}

enum Surface {
    #[cfg(feature = "offscreen")]
    Bitmap(windows::Win32::Graphics::Imaging::IWICBitmap),
    Software(ID2D1HwndRenderTarget),
    Hardware(HardwareSurface),
}
#[cfg(feature = "offscreen")]
struct Apartment(std::marker::PhantomData<std::rc::Rc<()>>);
#[cfg(feature = "offscreen")]
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
#[cfg(feature = "offscreen")]
impl WindowsRenderer {
    /// WIC software surface: creates no HWND and performs no desktop capture or input.
    pub fn offscreen(width: u32, height: u32, scale: f32) -> windows::core::Result<Self> {
        use windows::Win32::System::Com::*;
        if width == 0
            || height == 0
            || width as u64 * height as u64 > 16 * 1024 * 1024
            || !scale.is_finite()
            || !(0.5..=4.0).contains(&scale)
        {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let apartment = Apartment(std::marker::PhantomData);
        let mut renderer = Self::new(HWND::default(), true)?;
        renderer.apartment = Some(apartment);
        renderer.size = (width, height);
        renderer.scale = scale;
        renderer.create_bitmap()?;
        Ok(renderer)
    }
    fn create_bitmap(&mut self) -> windows::core::Result<()> {
        use windows::Win32::{Graphics::Imaging::*, System::Com::*};
        if self.size.0 as u64 * self.size.1 as u64 > 16 * 1024 * 1024 {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let bitmap = factory.CreateBitmap(
                self.size.0,
                self.size.1,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )?;
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0 * self.scale,
                dpiY: 96.0 * self.scale,
                ..Default::default()
            };
            self.target = Some(
                self.factory
                    .CreateWicBitmapRenderTarget(&bitmap, &properties)?,
            );
            self.surface = Some(Surface::Bitmap(bitmap));
        }
        Ok(())
    }
    pub fn pixels_bgra(&self) -> windows::core::Result<Vec<u8>> {
        let Some(Surface::Bitmap(bitmap)) = &self.surface else {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        };
        let mut pixels = vec![0; self.size.0 as usize * self.size.1 as usize * 4];
        unsafe {
            bitmap.CopyPixels(std::ptr::null(), self.size.0 * 4, &mut pixels)?;
        }
        Ok(pixels)
    }
}
struct HardwareSurface {
    context: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
}
impl HardwareSurface {
    fn bind(&self, scale: f32) -> windows::core::Result<()> {
        // SAFETY: buffer and target share this device; context retains the bitmap reference.
        unsafe {
            let buffer: IDXGISurface = self.swap.GetBuffer(0)?;
            let properties = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                dpiX: scale * 96.0,
                dpiY: scale * 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self
                .context
                .CreateBitmapFromDxgiSurface(&buffer, Some(&properties))?;
            self.context.SetTarget(&bitmap);
            self.context.SetDpi(scale * 96.0, scale * 96.0);
            Ok(())
        }
    }
    fn resize(&self, size: (u32, u32), scale: f32) -> windows::core::Result<()> {
        // Release the context's last back-buffer reference before ResizeBuffers.
        unsafe {
            self.context.SetTarget(None);
            self.swap.ResizeBuffers(
                0,
                size.0,
                size.1,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.bind(scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_context_keeps_arabic_joining_and_combining_cluster_metrics() {
        let mut renderer=WindowsRenderer::new(HWND::default(),true).unwrap();
        let text=format!("{}مرحبا a\u{301} 👩🏽‍💻 தமிழ்{}","x".repeat(4088),"z".repeat(100));
        let full=renderer.shape_with_font_family(&text,16.0,1.0e6,"Segoe UI").unwrap();
        let start=2048;
        let retained=renderer.shape_with_font_family(&text[start..],16.0,1.0e6,"Segoe UI").unwrap();
        let word=text.find("مرحبا").unwrap();
        let a=renderer.range_rects(full,word..word+"مرحبا".len()).unwrap();
        let b=renderer.range_rects(retained,word-start..word-start+"مرحبا".len()).unwrap();
        assert_eq!(a.len(),b.len());
        for (a,b) in a.iter().zip(&b){assert!((a.width-b.width).abs()<0.1);}
        let mark=text.find("a\u{301}").unwrap();
        assert!(renderer.range_rects(retained,mark-start..mark-start+"a\u{301}".len()).unwrap().iter().all(|r|r.width>=0.0));
        let leading=renderer.caret(full,word).unwrap();let next=renderer.caret(full,word+"م".len()).unwrap();
        assert!(leading.x>next.x,"RTL visual-left follows the next logical Arabic cluster");
    }
    #[cfg(feature = "offscreen")]
    #[test]
    fn offscreen_styles_color_utf8_ranges_without_changing_geometry() {
        let mut renderer = WindowsRenderer::offscreen(400, 80, 1.0).unwrap();
        let value = "let 🦀 = 1;";
        let id = renderer.shape(value, 24.0, 380.0).unwrap();
        let before = renderer.caret(id, 8).unwrap();
        let operations = [
            DrawOp::Fill(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 400.0,
                    height: 80.0,
                },
                Color(0x202020),
            ),
            DrawOp::Layout {
                origin: Point { x: 5.0, y: 5.0 },
                layout: id,
                color: Color(0xFFFFFF),
            },
        ];
        renderer.render(&operations).unwrap();
        let plain = renderer.pixels_bgra().unwrap();
        renderer
            .set_styles(
                id,
                &[
                    bareline_renderer::TextStyle {
                        bytes: 0..3,
                        color: Color(0xFF0000),
                    },
                    bareline_renderer::TextStyle {
                        bytes: 4..8,
                        color: Color(0x00FF00),
                    },
                ],
            )
            .unwrap();
        assert_eq!(renderer.caret(id, 8).unwrap(), before);
        assert_eq!(
            renderer.set_styles(
                id,
                &[bareline_renderer::TextStyle {
                    bytes: 5..8,
                    color: Color(0)
                }]
            ),
            Err(LayoutError::InvalidOffset)
        );
        renderer.render(&operations).unwrap();
        let styled = renderer.pixels_bgra().unwrap();
        assert_ne!(plain, styled);
        assert!(
            styled
                .chunks_exact(4)
                .any(|pixel| pixel[2] > 180 && pixel[1] < 80 && pixel[0] < 80)
        );
        renderer.set_styles(id, &[]).unwrap();
        renderer.render(&operations).unwrap();
        assert_eq!(renderer.pixels_bgra().unwrap(), plain);
    }
    use windows::Win32::UI::WindowsAndMessaging::*;
    struct WindowGuard(HWND);
    impl Drop for WindowGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    #[test]
    fn mixed_script_layout_survives_native_device_recreation_at_three_scales() {
        let window = WindowGuard(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Bareline renderer verification"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                900,
                600,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let text = "Latin مرحبا 👩🏽‍💻 e\u{301} end";
        for software in [false, true] {
            // A new HWND target must release its swap chain before the next mode uses it.
            let mut renderer = WindowsRenderer::new(window.0, software).unwrap();
            for scale in [1.0, 1.5, 2.0] {
                renderer
                    .resize((900.0 * scale) as u32, (600.0 * scale) as u32, scale)
                    .unwrap();
                let id = renderer.shape(text, 16.0, 700.0).unwrap();
                let arabic = text.find('م').unwrap();
                let after_first = arabic + 'م'.len_utf8();
                let a = renderer.caret(id, arabic).unwrap();
                let b = renderer.caret(id, after_first).unwrap();
                assert!(b.x < a.x, "Arabic logical advance must move visually left");
                assert_eq!(
                    renderer.caret(id, arabic + 1),
                    Err(LayoutError::InvalidOffset)
                );
                for x in (0..400).step_by(3) {
                    let hit = renderer
                        .hit_test(
                            id,
                            Point {
                                x: x as f32,
                                y: 8.0,
                            },
                        )
                        .unwrap();
                    assert!(text.is_char_boundary(hit.byte_offset));
                }
                let ranges = renderer.range_rects(id, arabic..text.len()).unwrap();
                assert!(!ranges.is_empty());
                let operations = [
                    DrawOp::Fill(
                        Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 900.0,
                            height: 600.0,
                        },
                        Color(0x1F2328),
                    ),
                    DrawOp::Layout {
                        origin: Point { x: 16.0, y: 16.0 },
                        layout: id,
                        color: Color(0xE6E8EA),
                    },
                ];
                assert_eq!(
                    renderer.render(&operations).unwrap(),
                    FrameStatus::Presented
                );
                assert_eq!(
                    renderer.software, software,
                    "this run must exercise the requested renderer"
                );
                renderer.invalidate_device();
                assert_eq!(
                    renderer.caret(id, arabic).unwrap(),
                    a,
                    "device loss must preserve text geometry"
                );
                assert_eq!(
                    renderer.render(&operations).unwrap(),
                    FrameStatus::Presented
                );
                renderer.release_layout(id);
                assert_eq!(renderer.caret(id, 0), Err(LayoutError::InvalidHandle));
            }
        }
    }
}
