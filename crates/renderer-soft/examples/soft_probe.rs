// SPDX-License-Identifier: MPL-2.0
//! Software renderer probe: paints the shared shell plus a paragraph of styled,
//! wrapped text with the portable renderer.
//!
//! * `soft_probe --offscreen <path.png>` renders at 1920 x 1080, scale 1, with
//!   no display, prints the frame cost and writes the PNG.
//! * `soft_probe [--frames <n>]` opens a window and paints every frame until
//!   it is closed (or after `n` frames), printing the average frame cost.
use bareline_renderer::{Color, DrawOp, FrameStatus, LayoutId, Point, RenderBackend, TextBackend, TextStyle, squiggle};
use bareline_renderer_soft::{SoftError, SoftRenderer};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowId},
};

const PARAGRAPH: &str = "fn main() {\n    let greeting = \"Hello from the portable renderer\";\n    \
    println!(\"{greeting}\"); // shaped by cosmic-text, drawn by tiny-skia\n}\n\
    Wrapped prose follows the window width: teh misspelt word carries a squiggle, \
    CJK 文字 and Arabic مرحبا fall back to installed faces.";
/// (needle, colour) pairs styled in the paragraph.
const HIGHLIGHTS: [(&str, u32); 5] = [
    ("fn", 0xFF7B72),
    ("let", 0xFF7B72),
    ("\"Hello from the portable renderer\"", 0xA5D6FF),
    ("println!", 0xD2A8FF),
    ("// shaped by cosmic-text, drawn by tiny-skia", 0x8B949E),
];
const TABS: [&str; 2] = ["Untitled", "soft_probe.rs"];

/// The paragraph layout, reshaped only when the available width changes.
#[derive(Default)]
struct Paragraph {
    layout: Option<(LayoutId, f32)>,
}
impl Paragraph {
    fn layout(&mut self, renderer: &mut SoftRenderer, width: f32) -> Result<LayoutId, Box<dyn std::error::Error>> {
        if let Some((id, shaped_width)) = self.layout
            && shaped_width == width
        {
            return Ok(id);
        }
        if let Some((id, _)) = self.layout.take() {
            renderer.release_layout(id);
        }
        let family = renderer.monospace_family().to_owned();
        let id = renderer
            .shape_wrapped(PARAGRAPH, 16.0, width, &family)
            .map_err(SoftError::Layout)?;
        let mut styles: Vec<TextStyle> = HIGHLIGHTS
            .iter()
            .filter_map(|(needle, color)| {
                let start = PARAGRAPH.find(needle)?;
                Some(TextStyle {
                    bytes: start..start + needle.len(),
                    color: Color(*color),
                })
            })
            .collect();
        styles.sort_by_key(|style| style.bytes.start);
        renderer.set_styles(id, &styles).map_err(SoftError::Layout)?;
        self.layout = Some((id, width));
        Ok(id)
    }
}

/// The shell for `width` x `height` DIPs, the paragraph and a spelling squiggle.
fn frame(
    renderer: &mut SoftRenderer,
    paragraph: &mut Paragraph,
    width: f32,
    height: f32,
) -> Result<Vec<DrawOp>, Box<dyn std::error::Error>> {
    let tabs: Vec<String> = TABS.iter().map(|tab| (*tab).to_owned()).collect();
    let mut ops = bareline_ui::shell(width, height, &tabs, 0, false);
    let origin = Point { x: 72.0, y: 76.0 };
    // Narrower than a wide window, so the prose visibly wraps.
    let id = paragraph.layout(renderer, (width - origin.x - 24.0).clamp(80.0, 760.0))?;
    ops.push(DrawOp::Layout {
        origin,
        layout: id,
        color: Color(0xE6E8EA),
    });
    let typo = PARAGRAPH.find("teh").unwrap_or(0);
    let left = renderer.caret(id, typo).map_err(SoftError::Layout)?;
    let right = renderer.caret(id, typo + "teh".len()).map_err(SoftError::Layout)?;
    if left.y == right.y {
        squiggle(
            origin.x + left.x,
            origin.x + right.x,
            origin.y + left.y + left.height - 1.0,
            Color(0xFF5555),
            &mut ops,
        );
    }
    Ok(ops)
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn offscreen(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = (1920, 1080);
    let created = Instant::now();
    let mut renderer = SoftRenderer::offscreen(width, height, 1.0)?;
    println!("fonts loaded in {:.1} ms", milliseconds(created.elapsed()));
    let shell = bareline_ui::shell(1920.0, 1080.0, &["Untitled".into()], 0, false);
    let cold = Instant::now();
    renderer.render(&shell)?;
    let cold = cold.elapsed();
    const FRAMES: u32 = 30;
    let warm = Instant::now();
    for _ in 0..FRAMES {
        renderer.render(&shell)?;
    }
    let warm = warm.elapsed() / FRAMES;
    println!(
        "shell op list ({} ops) at {width}x{height} scale 1.0: first frame {:.2} ms, then {:.2} ms per frame (mean of {FRAMES})",
        shell.len(),
        milliseconds(cold),
        milliseconds(warm),
    );
    let mut paragraph = Paragraph::default();
    let ops = frame(&mut renderer, &mut paragraph, 1920.0, 1080.0)?;
    renderer.render(&ops)?;
    let full = Instant::now();
    for _ in 0..FRAMES {
        renderer.render(&ops)?;
    }
    println!(
        "shell plus styled paragraph ({} ops): {:.2} ms per frame (mean of {FRAMES})",
        ops.len(),
        milliseconds(full.elapsed() / FRAMES),
    );
    renderer.write_png(path)?;
    println!("wrote {path}");
    Ok(())
}

struct Probe {
    window: Option<Arc<Window>>,
    renderer: Option<SoftRenderer>,
    paragraph: Paragraph,
    frame_limit: Option<u64>,
    frames: u64,
    elapsed: Duration,
}
impl Probe {
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let start = Instant::now();
        let result = renderer
            .resize(size.width, size.height, scale)
            .map_err(Box::<dyn std::error::Error>::from)
            .and_then(|()| {
                frame(
                    renderer,
                    &mut self.paragraph,
                    size.width as f32 / scale,
                    size.height as f32 / scale,
                )
            })
            .and_then(|ops| Ok(renderer.render(&ops)?));
        match result {
            Ok(FrameStatus::Presented) => {
                self.frames += 1;
                self.elapsed += start.elapsed();
                if self.frames.is_multiple_of(120) {
                    println!(
                        "{}x{} scale {scale}: {:.2} ms per frame (mean of the last 120)",
                        size.width,
                        size.height,
                        milliseconds(self.elapsed / 120)
                    );
                    self.elapsed = Duration::ZERO;
                }
                if self.frame_limit.is_some_and(|limit| self.frames >= limit) {
                    event_loop.exit();
                    return;
                }
            }
            Ok(FrameStatus::Recreate) => {}
            Err(error) => {
                eprintln!("frame failed: {error}");
                event_loop.exit();
                return;
            }
        }
        // Paint continuously, as a stress test of the CPU path.
        window.request_redraw();
    }
}
impl ApplicationHandler for Probe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes().with_title("Bareline software renderer probe");
        let created = event_loop
            .create_window(attributes)
            .map_err(Box::<dyn std::error::Error>::from)
            .and_then(|window| {
                let window = Arc::new(window);
                let renderer = SoftRenderer::for_window(window.clone())?;
                Ok((window, renderer))
            });
        match created {
            Ok((window, renderer)) => {
                window.request_redraw();
                self.window = Some(window);
                self.renderer = Some(renderer);
            }
            Err(error) => {
                eprintln!("window unavailable: {error}");
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [flag, path] if flag == "--offscreen" => offscreen(path),
        [] => run_window(None),
        [flag, frames] if flag == "--frames" => run_window(Some(frames.parse()?)),
        _ => Err("usage: soft_probe [--offscreen <path.png> | --frames <n>]".into()),
    }
}
fn run_window(frame_limit: Option<u64>) -> Result<(), Box<dyn std::error::Error>> {
    let mut probe = Probe {
        window: None,
        renderer: None,
        paragraph: Paragraph::default(),
        frame_limit,
        frames: 0,
        elapsed: Duration::ZERO,
    };
    EventLoop::new()?.run_app(&mut probe)?;
    Ok(())
}
