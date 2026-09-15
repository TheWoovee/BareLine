// SPDX-License-Identifier: MPL-2.0
//! Native byte scrollbar capture using the shared PR023 control.
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_renderer::{DrawOp, Rect};
use bareline_ui::controls::{ScrollAction, Scrollbar, ScrollbarInteraction, UiEvent};
#[derive(Default)]
pub(super) struct Runtime {
    bars: [Option<Scrollbar>; 2],
    interactions: [ScrollbarInteraction; 2],
    identities: [Option<(u64, u64)>; 2],
    captured: Option<usize>,
}
impl Runtime {
    pub(super) fn draw(
        &mut self,
        workspace: &bareline_app::workspace::Workspace,
        views: &super::views::ViewsRuntime,
        active: usize,
        bounds: Rect,
        ops: &mut Vec<DrawOp>,
    ) {
        let split = views.secondary.is_some();
        for pane in 0..2 {
            let editor = if pane == 1 {
                views.secondary.as_ref()
            } else {
                workspace.editors.get(views.primary_index(workspace).unwrap_or(active))
            };
            let Some(editor) = editor else {
                self.bars[pane] = None;
                continue;
            };
            let area = if split {
                views.bounds[pane].map(|r| Rect {
                    x: r.x + bounds.x,
                    y: r.y + bounds.y,
                    ..r
                })
            } else if pane == 0 {
                Some(bounds)
            } else {
                None
            };
            let Some(area) = area else {
                self.bars[pane] = None;
                continue;
            };
            let identity = match editor {
                WorkspaceEditor::Paged(editor) => editor.snapshot().identity_token(),
                WorkspaceEditor::Resident(editor) => editor.snapshot().identity_token(),
            };
            if self.identities[pane] != Some(identity) {
                self.interactions[pane] = ScrollbarInteraction::default();
                self.interactions[pane].deferred = true;
                if self.captured == Some(pane) {
                    self.captured = None;
                }
                self.identities[pane] = Some(identity);
            }
            let body_height = (area.height
                - bareline_ui::TAB_HEIGHT
                - editor.viewport().top_inset
                - editor.viewport().bottom_inset
                - if split { 0.0 } else { bareline_ui::STATUS_HEIGHT })
            .max(0.0);
            let geometry = Rect {
                x: area.x + area.width - 12.0,
                y: area.y + bareline_ui::TAB_HEIGHT + editor.viewport().top_inset,
                width: 12.0,
                height: body_height,
            };
            let mut bar = match editor {
                WorkspaceEditor::Paged(editor) => {
                    let metrics = editor.paged_scroll_metrics(body_height);
                    Scrollbar {
                        bounds: geometry,
                        offset: metrics.offset,
                        viewport: metrics.viewport,
                        total: Some(metrics.total),
                    }
                }
                WorkspaceEditor::Resident(editor) => editor.scrollbar(geometry),
            };
            if self.captured == Some(pane) {
                if let Some(previous) = &self.bars[pane] {
                    bar.offset = previous.offset;
                }
            }
            bar.paint_with_theme(workspace.theme, ops);
            self.bars[pane] = Some(bar);
        }
    }
}
impl Shell {
    pub(super) fn scrolling_event(&mut self, event: &WindowEvent) -> bool {
        let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        let ui = match event {
            WindowEvent::CursorMoved { position, .. } => UiEvent::PointerMove(Point {
                x: (position.x / scale) as f32,
                y: (position.y / scale) as f32,
            }),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => UiEvent::PointerDown(self.pointer),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => UiEvent::PointerUp(self.pointer),
            WindowEvent::Focused(false) => UiEvent::Focus(false),
            _ => return false,
        };
        if matches!(ui, UiEvent::Focus(false)) {
            for pane in 0..2 {
                if let Some(bar) = &mut self.scrolling.bars[pane] {
                    self.scrolling.interactions[pane].event(bar, ui, true, false, 1.0);
                }
            }
            self.scrolling.captured = None;
            return false;
        }
        let pane = self.scrolling.captured.or_else(|| match ui {
            UiEvent::PointerDown(p) => self
                .scrolling
                .bars
                .iter()
                .position(|bar| bar.as_ref().is_some_and(|bar| bar.bounds.contains(p))),
            _ => None,
        });
        let Some(pane) = pane else {
            return false;
        };
        if matches!(ui, UiEvent::PointerDown(_)) {
            self.scrolling.captured = Some(pane);
        }
        let Some(bar) = &mut self.scrolling.bars[pane] else {
            return false;
        };
        let action = self.scrolling.interactions[pane].event(bar, ui, true, false, 1.0);
        let fraction = if bar.maximum() > 0.0 {
            bar.offset / bar.maximum()
        } else {
            0.0
        };
        let height = bar.bounds.height;
        if matches!(ui, UiEvent::PointerUp(_)) {
            self.scrolling.captured = None;
        }
        if let Some(ScrollAction::Commit(_)) = action {
            let index = self
                .workspace
                .as_ref()
                .and_then(|w| self.views.primary_index(w))
                .unwrap_or(self.app.active);
            let mut editor = if pane == 1 {
                self.views.secondary.as_mut()
            } else {
                self.workspace.as_mut().and_then(|w| w.editors.get_mut(index))
            };
            if let Some(WorkspaceEditor::Resident(editor)) = &mut editor {
                if self.scrolling.identities[pane] == Some(editor.snapshot().identity_token()) {
                    editor.scroll(
                        bar.offset - editor.scroll_y,
                        height + bareline_ui::TAB_HEIGHT + editor.top_inset + bareline_ui::STATUS_HEIGHT,
                    );
                }
            }
            if let Some(WorkspaceEditor::Paged(editor)) = editor {
                if self.scrolling.identities[pane] == Some(editor.snapshot().identity_token()) {
                    if let Err(error) = editor.request_byte_scroll_in_view(fraction, height) {
                        editor.error = Some(error);
                    }
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_thumb_reaches_distant_source_with_deferred_capture() {
        let mut bar = Scrollbar {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 12.0,
                height: 600.0,
            },
            offset: 0.0,
            viewport: 65_536.0,
            total: Some(1_073_741_824.0),
        };
        let mut interaction = ScrollbarInteraction::default();
        interaction.deferred = true;
        let down = Point {
            x: 6.0,
            y: bar.thumb().y + 2.0,
        };
        assert!(
            interaction
                .event(&mut bar, UiEvent::PointerDown(down), true, false, 1.0)
                .is_none()
        );
        let far = Point { x: 6.0, y: 590.0 };
        assert!(matches!(
            interaction.event(&mut bar, UiEvent::PointerMove(far), true, false, 1.0),
            Some(ScrollAction::Preview(_))
        ));
        let Some(ScrollAction::Commit(value)) = interaction.event(&mut bar, UiEvent::PointerUp(far), true, false, 1.0)
        else {
            panic!("drag did not commit")
        };
        assert!(value / bar.maximum() > 0.95);
        interaction.event(&mut bar, UiEvent::Focus(false), true, false, 1.0);
        assert!(
            interaction
                .event(&mut bar, UiEvent::PointerMove(down), true, false, 1.0)
                .is_none()
        );
    }
    #[test]
    fn paged_thumb_position_tracks_byte_offset() {
        // 106 MiB document, a 64 KiB window resident, scrolled to the halfway byte.
        // The thumb must sit at the same fraction of the track as offset/total, so a
        // thumb dragged to 50% lands around byte 53 MiB regardless of the line total.
        let total = 106.0 * 1024.0 * 1024.0;
        let bar = Scrollbar {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 12.0,
                height: 600.0,
            },
            offset: total / 2.0,
            viewport: 65_536.0,
            total: Some(total),
        };
        let byte_fraction = bar.offset / bar.maximum();
        assert!((byte_fraction - 0.5).abs() < 0.01, "byte fraction {byte_fraction}");
        let thumb = bar.thumb();
        let travel = (bar.bounds.height - thumb.height) as f64;
        let thumb_fraction = (thumb.y - bar.bounds.y) as f64 / travel;
        assert!(
            (thumb_fraction - byte_fraction).abs() < 0.01,
            "thumb fraction {thumb_fraction} vs byte fraction {byte_fraction}"
        );
    }
}
