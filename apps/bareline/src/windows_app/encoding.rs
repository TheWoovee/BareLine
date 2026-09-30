// SPDX-License-Identifier: MPL-2.0
//! Native PR-007 menus; all source changes and save policies use Workspace APIs.
use super::*;
use bareline_app::encoding as model;
use bareline_commands::{CommandContext, CommandId};

/// Semantic ids of the binary notice status region; its actions follow it.
pub(super) const BINARY_NOTICE_ID: u64 = 90_000_050;

/// Notice strip and its [Edit as text, Close] targets inside one document view.
/// It fills the band the view reserves above its text while the notice is
/// pending (`top_inset` then includes BINARY_NOTICE_HEIGHT), under the tab strip
/// and any Find bar, and stacks below a watch banner ending at `floor`.
pub(super) fn binary_notice_layout(
    bounds: bareline_renderer::Rect,
    top_inset: f32,
    floor: f32,
) -> (bareline_renderer::Rect, [bareline_renderer::Rect; 2]) {
    use bareline_ui::rect;
    let band = bounds.y + bareline_ui::TAB_HEIGHT + (top_inset - model::BINARY_NOTICE_HEIGHT).max(0.0);
    let banner = rect(
        bounds.x + 8.0,
        band.max(floor) + 4.0,
        (bounds.width - 16.0).max(0.0),
        32.0,
    );
    let action_width = 110.0_f32.min(((banner.width - 16.0) / 2.0).max(0.0));
    let actions_x = (banner.x + banner.width - action_width * 2.0 - 8.0).max(banner.x + 8.0);
    let action = |slot: f32| rect(actions_x + slot * action_width, banner.y, action_width, banner.height);
    (banner, [action(0.0), action(1.0)])
}
/// Lowest edge of the watch banner targets already drawn in a view whose top
/// is `top`, so the binary notice stacks under that banner instead of hiding.
pub(super) fn watch_banner_floor(top: f32, hits: impl IntoIterator<Item = bareline_renderer::Rect>) -> f32 {
    hits.into_iter().map(|hit| hit.y + hit.height).fold(top, f32::max)
}
/// Draws the non-modal binary notice for `index` (UI-01) and returns its
/// click targets; the watch hit path dispatches them as ordinary commands.
pub(super) fn draw_binary_notice(
    workspace: &bareline_app::workspace::Workspace,
    index: usize,
    bounds: bareline_renderer::Rect,
    top_inset: f32,
    floor: f32,
    ops: &mut Vec<bareline_renderer::DrawOp>,
) -> Vec<(bareline_renderer::Rect, CommandId)> {
    use bareline_renderer::DrawOp;
    use bareline_ui::{ACCENT, CHROME, TEXT, rect, text};
    let Some(notice) = workspace.binary_notice(index) else {
        return Vec::new();
    };
    let (banner, actions) = binary_notice_layout(bounds, top_inset, floor);
    ops.push(DrawOp::FillRounded(banner, CHROME, 4.0));
    ops.push(DrawOp::StrokeRounded(banner, ACCENT, 4.0, 1.0));
    ops.push(DrawOp::PushClip(rect(
        banner.x + 12.0,
        banner.y,
        (actions[0].x - banner.x - 20.0).max(0.0),
        banner.height,
    )));
    text(ops, banner.x + 12.0, banner.y + 7.0, notice, 14.0, TEXT);
    ops.push(DrawOp::PopClip);
    let mut hits = Vec::new();
    for ((label, command), bounds) in model::BINARY_NOTICE_ACTIONS.into_iter().zip(actions) {
        ops.push(DrawOp::PushClip(bounds));
        text(ops, bounds.x + 6.0, bounds.y + 7.0, label, 14.0, ACCENT);
        ops.push(DrawOp::PopClip);
        hits.push((bounds, CommandId(command)));
    }
    hits
}
/// Maps a notice action node to the command its button dispatches.
pub(super) fn binary_notice_command(id: u64) -> Option<&'static str> {
    let slot = id.checked_sub(BINARY_NOTICE_ID + 1)?;
    model::BINARY_NOTICE_ACTIONS
        .get(usize::try_from(slot).ok()?)
        .map(|(_, command)| *command)
}
impl Shell {
    /// The active document's binary notice as a status region with invokable
    /// actions, so assistive technology reaches the same non-modal choices.
    pub(super) fn encoding_accessibility_nodes(
        &self,
        editor_bounds: bareline_renderer::Rect,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let Some(workspace) = &self.workspace else {
            return Vec::new();
        };
        let Some(notice) = workspace.binary_notice(self.app.active) else {
            return Vec::new();
        };
        let (pane, editor) = match &self.views.secondary {
            Some(editor) if self.views.pane() == 1 => (1, Some(editor)),
            _ => (0, workspace.editors.get(self.app.active)),
        };
        let bounds = self.views.bounds[pane].map_or(editor_bounds, |mut bounds| {
            bounds.x += editor_bounds.x;
            bounds.y += editor_bounds.y;
            bounds
        });
        let top_inset = editor.map_or(0.0, |editor| editor.viewport().top_inset);
        // Same stacking as the last drawn frame: below that pane's watch banner.
        let floor = watch_banner_floor(
            bounds.y,
            self.watch
                .hits
                .iter()
                .filter(|(_, hit_pane, _, id)| {
                    *hit_pane as usize == pane
                        && !model::BINARY_NOTICE_ACTIONS.iter().any(|(_, command)| id.0 == *command)
                })
                .map(|(rect, ..)| *rect),
        );
        let (banner, actions) = binary_notice_layout(bounds, top_inset, floor);
        let area = |r: bareline_renderer::Rect| [r.x as f64, r.y as f64, r.width as f64, r.height as f64];
        let node = |id, parent, role, name: String, bounds, invokable| AccessibilityNode {
            id,
            parent,
            role,
            name,
            value: None,
            bounds,
            disabled: false,
            selected: false,
            expanded: None,
            // Not in the Tab order: keyboard users make the same decision
            // through Encoding > Binary (encoding.binary.*) or the palette.
            focusable: false,
            invokable,
            position_in_set: None,
            size_of_set: None,
        };
        let mut nodes = vec![node(
            BINARY_NOTICE_ID,
            1,
            AccessibilityRole::Status,
            notice,
            area(banner),
            false,
        )];
        for (slot, ((label, _), bounds)) in model::BINARY_NOTICE_ACTIONS.into_iter().zip(actions).enumerate() {
            nodes.push(node(
                BINARY_NOTICE_ID + 1 + slot as u64,
                BINARY_NOTICE_ID,
                AccessibilityRole::Button,
                label.into(),
                area(bounds),
                true,
            ));
        }
        nodes
    }
    pub(super) fn encoding_accessibility(
        &mut self,
        el: &ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        let bareline_platform::accessibility::AccessibilityAction::Invoke(id) = action else {
            return false;
        };
        let Some(command) = binary_notice_command(*id) else {
            return false;
        };
        if !self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.binary_notice(self.app.active).is_some())
        {
            return false;
        }
        if let Ok(action) = self
            .app
            .commands
            .dispatch_in(CommandId(command), &self.command_context())
        {
            self.dispatch(el, action);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn encoding_context(&self, context: &mut CommandContext) {
        let state = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.encoding_state(self.app.active));
        let editor = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.editors.get(self.app.active));
        model::annotate(
            context,
            state.as_ref(),
            editor.is_none_or(|editor| editor.busy()),
            editor.is_some_and(|editor| editor.read_only()),
        );
        let failure = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.encoding_failure(self.app.active));
        let failure_state = match failure {
            Some(failure) => bareline_commands::CommandState {
                label: Some(format!(
                    "Show {}",
                    model::failure_description(failure.revision, failure.range, &failure.reason)
                )),
                ..if editor.is_some_and(|editor| !editor.busy()) {
                    bareline_commands::CommandState::default()
                } else {
                    bareline_commands::CommandState::disabled("Wait for the document operation")
                }
            },
            None => bareline_commands::CommandState::disabled("No encoding save failure for this document"),
        };
        context.states.insert(CommandId("encoding.failure"), failure_state);
        if let Some(workspace) = &self.workspace
            && let Some(item) = context.states.get_mut(&CommandId("encoding.eol"))
        {
            item.label = Some(format!(
                "Line endings: {}…",
                workspace.encoding_eol_label(self.app.active)
            ));
        }
    }
    fn encoding_popup(&mut self, el: &ActiveEventLoop, ids: &[&'static str]) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Ok(origin) = window.inner_position() else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor();
        let context = self.command_context();
        let commands: Vec<_> = ids.iter().map(|id| CommandId(id)).collect();
        let Some(platform) = self.platform.as_ref() else {
            return;
        };
        let result = platform.context_menu_in(
            origin.x + (size.width as i32 - (230.0 * scale) as i32).max(0),
            origin.y + (size.height as i32 - (24.0 * scale) as i32).max(0),
            &self.app.commands,
            &context,
            &self.settings.keymap.keymap,
            &commands,
        );
        match result {
            Ok(Some(action)) => self.dispatch(el, action),
            Ok(None) => {}
            Err(error) => {
                if let Some(workspace) = &mut self.workspace {
                    workspace.message = Some(format!("Encoding menu unavailable: {error}"));
                }
            }
        }
    }
    pub(super) fn encoding_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("encoding.") {
            return false;
        }
        match id {
            "encoding.choose" => {
                self.encoding_popup(el, model::ROOT);
                return true;
            }
            "encoding.choose_interpret" => {
                self.encoding_popup(
                    el,
                    &model::CODECS.iter().map(|choice| choice.interpret).collect::<Vec<_>>(),
                );
                return true;
            }
            "encoding.choose_convert" => {
                self.encoding_popup(
                    el,
                    &model::CODECS.iter().map(|choice| choice.convert).collect::<Vec<_>>(),
                );
                return true;
            }
            "encoding.eol" => {
                self.encoding_popup(el, model::EOLS);
                return true;
            }
            "encoding.binary" => {
                self.encoding_popup(el, model::BINARY);
                return true;
            }
            "encoding.charsets" => {
                self.charsets.open();
                self.palette.dismiss();
                self.app.palette = false;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
            _ => {}
        }
        let active = self.app.active;
        let result = (|| {
            if id == "encoding.failure" {
                let workspace = self.workspace.as_mut().ok_or("No document")?;
                let failure = workspace
                    .encoding_failure(active)
                    .ok_or("No encoding save failure for this document")?;
                // The workspace owns the captured identity and rejects stale revisions
                // before selecting any bytes, including absolute Paged navigation.
                workspace.encoding_reveal_failure(active)?;
                workspace.message = Some(model::failure_description(
                    failure.revision,
                    failure.range,
                    &failure.reason,
                ));
                return Ok(());
            }
            let state = self
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.encoding_state(active))
                .ok_or("No complete document encoding state")?;
            if id == "encoding.info" || id == "encoding.binary.info" {
                return Ok(());
            }
            if let Some(choice) = model::CODECS.iter().find(|choice| choice.interpret == id) {
                let workspace = self.workspace.as_ref().ok_or("No document")?;
                let editor = workspace.editors.get(active).ok_or("Document closed")?;
                if editor.busy() {
                    return Err("Wait for the current document operation".into());
                }
                let confirmed = if editor.dirty() {
                    let name = workspace
                        .path(active)
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled".into());
                    if !self
                        .platform
                        .as_ref()
                        .ok_or("Native confirmation unavailable")?
                        .confirm_encoding_reinterpret(&name, choice.label)
                    {
                        return Ok(());
                    }
                    true
                } else {
                    false
                };
                return self
                    .workspace
                    .as_mut()
                    .unwrap()
                    .encoding_interpret(active, choice.encoding, confirmed);
            }
            let workspace = self.workspace.as_mut().ok_or("No document")?;
            if let Some(choice) = model::CODECS.iter().find(|choice| choice.convert == id) {
                // Preserve deliberate BOM choice only for targets that support it.
                return workspace.encoding_convert(
                    active,
                    choice.encoding,
                    state.bom && !choice.encoding.bom().is_empty(),
                );
            }
            if let Some((eol, selection_only)) = model::eol_action(id) {
                return workspace.encoding_eol(active, eol, selection_only);
            }
            match id {
                "encoding.bom_on" => workspace.encoding_convert(active, state.save_target, true),
                "encoding.bom_off" => workspace.encoding_convert(active, state.save_target, false),
                "encoding.binary.readonly" => workspace.encoding_accept_binary(active, true),
                "encoding.binary.edit" => workspace.encoding_accept_binary(active, false),
                _ => Err("Unknown encoding command".into()),
            }
        })();
        if let Err(error) = result
            && let Some(workspace) = &mut self.workspace
        {
            workspace.message = Some(error);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn encoding_pump(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            let eol = workspace.encoding_eol_label(self.app.active);
            if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                editor.set_eol_status_override(Some(eol));
            }
        }
        if let Some(workspace) = &mut self.workspace
            && let Some(state) = workspace.encoding_state(self.app.active)
            && let Some(editor) = workspace.editors.get_mut(self.app.active)
        {
            editor.viewport_mut().encoding_label = state.save_target.status_label(state.bom);
        }
        // A binary-like document stays read-only until an explicit decision. That
        // choice is offered by the in-view notice (UI-01), never by a modal popup
        // here, so queued opens and commands continue meanwhile.
    }
    pub(super) fn encoding_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            }
        ) {
            return false;
        }
        let Some(window) = &self.window else {
            return false;
        };
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        if self.pointer.y < size.height - bareline_ui::STATUS_HEIGHT || self.pointer.y > size.height {
            return false;
        }
        if self.pointer.x >= size.width - 155.0 && self.pointer.x < size.width - 50.0 {
            self.encoding_popup(el, model::ROOT);
            true
        } else if self.pointer.x >= size.width - 240.0 && self.pointer.x < size.width - 155.0 {
            self.encoding_popup(el, model::EOLS);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_notice_actions_map_to_encoding_commands_inside_the_view() {
        assert_eq!(
            binary_notice_command(BINARY_NOTICE_ID + 1),
            Some("encoding.binary.edit")
        );
        assert_eq!(
            binary_notice_command(BINARY_NOTICE_ID + 2),
            Some("encoding.binary.readonly")
        );
        assert_eq!(binary_notice_command(BINARY_NOTICE_ID), None);
        assert_eq!(binary_notice_command(BINARY_NOTICE_ID + 3), None);
        // The strip stays inside the document view, below its tab strip and any
        // Find bar, entirely inside the band reserved above the text, and its
        // two actions sit inside it without overlapping.
        let view = bareline_ui::rect(100.0, 50.0, 800.0, 600.0);
        for find_height in [0.0, bareline_app::find::HEIGHT] {
            let top_inset = find_height + model::BINARY_NOTICE_HEIGHT;
            let text_top = view.y + bareline_ui::TAB_HEIGHT + top_inset;
            let (banner, _) = binary_notice_layout(view, top_inset, view.y);
            assert!(banner.x >= view.x && banner.x + banner.width <= view.x + view.width);
            assert!(banner.y >= view.y + bareline_ui::TAB_HEIGHT + find_height);
            assert!(banner.y + banner.height <= text_top, "{find_height}");
        }
        // A follow banner over the tab strip keeps the notice visible below it
        // and still clear of the first text line.
        let follow = bareline_ui::rect(view.x + 8.0, view.y + 4.0, view.width - 16.0, 34.0);
        let floor = watch_banner_floor(view.y, [follow]);
        assert_eq!(floor, follow.y + follow.height);
        let (banner, [edit, close]) = binary_notice_layout(view, model::BINARY_NOTICE_HEIGHT, floor);
        assert!(banner.y >= follow.y + follow.height);
        assert!(banner.y + banner.height <= view.y + bareline_ui::TAB_HEIGHT + model::BINARY_NOTICE_HEIGHT);
        for action in [edit, close] {
            assert!(action.x >= banner.x && action.x + action.width <= banner.x + banner.width);
        }
        assert!(edit.x + edit.width <= close.x);
    }
}
