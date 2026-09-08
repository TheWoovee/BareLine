// SPDX-License-Identifier: MPL-2.0
pub mod find;
pub mod language;
pub mod palette;
pub mod toolbar;
pub mod search_panel;
pub mod session_ui;
pub mod session_service;
pub mod workspace_panel;
pub mod views;
pub mod settings;
pub mod macros;
pub mod compare;
pub mod utilities;
pub mod text_prototype;
pub mod workspace;
mod styling;
pub use styling::Styling as ViewStyling;
use bareline_commands::{Action, CommandRegistry, shell_commands};
use bareline_renderer::DrawOp;
pub struct App {
    pub commands: CommandRegistry,
    pub tabs: Vec<String>,
    pub active: usize,
    pub palette: bool,
}
impl Default for App {
    fn default() -> Self {
        let mut commands = shell_commands();
        crate::search_panel::register_commands(&mut commands).expect("unique search commands");
        bareline_editor_surface::power::register_commands(&mut commands);
        Self {
            commands,
            tabs: Vec::new(),
            active: 0,
            palette: false,
        }
    }
}
impl App {
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::New => {
                self.tabs.push(format!("Untitled {}", self.tabs.len() + 1));
                self.active = self.tabs.len() - 1;
            }
            Action::Palette => self.palette = !self.palette,
            Action::Quit
            | Action::About
            | Action::Undo
            | Action::Redo
            | Action::SelectAll
            | Action::Open
            | Action::Save
            | Action::Copy
            | Action::Cut
            | Action::Paste
            | Action::SaveAs
            | Action::CancelFileOperations
            | Action::Close
            | Action::Find
            | Action::FindNext
            | Action::FindPrevious
            | Action::FindClose
            | Action::FindMatchCase
            | Action::Replace
            | Action::ReplaceOne
            | Action::ReplaceAll
            | Action::FindMode
            | Action::FindCancel
            | Action::FindWholeWord => {}
            Action::Contributed(_) => {}
        }
    }
    pub fn draw(&self, width: f32, height: f32) -> Vec<DrawOp> {
        bareline_ui::shell(width, height, &self.tabs, self.active, false)
    }
    pub fn overlay(&self, width: f32, operations: &mut Vec<DrawOp>) {
        if self.palette {
            bareline_ui::palette(width, operations);
        }
    }
    pub fn click_tab(&mut self, width: f32, point: bareline_renderer::Point) -> bool {
        let strip = bareline_ui::controls::TabStrip {
            width,
            count: self.tabs.len(),
            active: self.active,
        };
        if let Some(active) = strip.hit_test(point) {
            self.active = active;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_renderer::{FrameStatus, RenderBackend};
    use bareline_renderer_recording::RecordingBackend;
    #[test]
    fn lazy_tabs_survive_renderer_loss_without_document_io() {
        let mut app = App::default();
        for _ in 0..5_000 {
            app.apply(Action::New);
        }
        let frame = app.draw(1200.0, 800.0);
        assert!(frame.len() < 100, "only visible tabs generate draw work");
        let mut backend = RecordingBackend::default();
        backend.simulate_device_loss();
        assert_eq!(backend.render(&frame).unwrap(), FrameStatus::Recreate);
        assert_eq!(backend.render(&frame).unwrap(), FrameStatus::Presented);
        assert_eq!(app.tabs.len(), 5_000);
        assert_eq!(backend.operations, frame);
    }
}

pub mod accessibility;

pub mod extensions;

pub mod encoding;
