// SPDX-License-Identifier: MPL-2.0
//! Compile/run probe only: records shared UI operations; does not paint native pixels.
use bareline_renderer::{Point, RenderBackend};
use bareline_renderer_recording::RecordingBackend;
use bareline_ui::{
    ViewId,
    controls::{Button, ControlState, Key, UiEvent},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key as WinitKey, NamedKey},
    window::{Window, WindowId},
};
struct Probe {
    window: Option<Window>,
    recorder: RecordingBackend,
    button: Button,
    pointer: Point,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            window: None,
            recorder: RecordingBackend::default(),
            button: Button {
                id: ViewId(1),
                label: "Recording probe".into(),
                bounds: bareline_ui::rect(10.0, 10.0, 160.0, 32.0),
                toggle: true,
                state: ControlState::default(),
            },
            pointer: Point::default(),
        }
    }
}
impl ApplicationHandler for Probe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            match event_loop
                .create_window(Window::default_attributes().with_title("Bareline recording probe (no pixel painter)"))
            {
                Ok(window) => {
                    window.request_redraw();
                    self.window = Some(window);
                }
                Err(error) => {
                    eprintln!("Window unavailable: {error}");
                    event_loop.exit();
                }
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = &self.window else {
            return;
        };
        let scale = window.scale_factor();
        let normalized = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                None
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = Point {
                    x: (position.x / scale) as f32,
                    y: (position.y / scale) as f32,
                };
                Some(UiEvent::PointerMove(self.pointer))
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => Some(if state == ElementState::Pressed {
                UiEvent::PointerDown(self.pointer)
            } else {
                UiEvent::PointerUp(self.pointer)
            }),
            WindowEvent::Focused(value) => Some(UiEvent::Focus(value)),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key {
                    WinitKey::Named(NamedKey::Enter) => Some(UiEvent::Key(Key::Enter)),
                    WinitKey::Named(NamedKey::Space) => Some(UiEvent::Key(Key::Space)),
                    _ => None,
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                window.request_redraw();
                None
            }
            WindowEvent::RedrawRequested => {
                let size = window.inner_size();
                let ops = bareline_ui::shell(
                    size.width as f32 / scale as f32,
                    size.height as f32 / scale as f32,
                    &["Recording".into()],
                    0,
                    false,
                );
                if self
                    .recorder
                    .resize(size.width, size.height, scale as f32)
                    .and_then(|_| self.recorder.render(&ops))
                    .is_err()
                {
                    eprintln!("Recording contract failed");
                    event_loop.exit();
                }
                None
            }
            _ => None,
        };
        if let Some(event) = normalized {
            let _action = self.button.event(event);
            window.request_redraw();
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    EventLoop::new()?.run_app(&mut Probe::default())?;
    Ok(())
}
