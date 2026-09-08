// SPDX-License-Identifier: MPL-2.0
//! Native PR-007 menus; all source changes and save policies use Workspace APIs.
use super::*;
use bareline_app::encoding as model;
use bareline_commands::{CommandContext, CommandId};

#[derive(Default)]
pub(super) struct EncodingRuntime {
    warned: Option<(u64, u64)>,
}
impl Shell {
    pub(super) fn encoding_context(&self, context: &mut CommandContext) {
        let state = self.workspace.as_ref().and_then(|workspace| workspace.encoding_state(self.app.active));
        let editor = self.workspace.as_ref().and_then(|workspace| workspace.editors.get(self.app.active));
        model::annotate(context, state.as_ref(), editor.is_none_or(|editor| editor.busy()), editor.is_some_and(|editor| editor.read_only()));
        let failure = self.workspace.as_ref().and_then(|workspace| workspace.encoding_failure(self.app.active));
        let failure_state = match failure {
            Some(failure) => bareline_commands::CommandState {
                label: Some(format!("Show {}", model::failure_description(failure.revision, failure.range, &failure.reason))),
                ..if editor.is_some_and(|editor| !editor.busy()) { bareline_commands::CommandState::default() } else { bareline_commands::CommandState::disabled("Wait for the document operation") }
            },
            None => bareline_commands::CommandState::disabled("No encoding save failure for this document"),
        };
        context.states.insert(CommandId("encoding.failure"), failure_state);
        if let Some(workspace) = &self.workspace
            && let Some(item) = context.states.get_mut(&CommandId("encoding.eol")) {
            item.label = Some(format!("Line endings: {}…", workspace.encoding_eol_label(self.app.active)));
        }
    }
    fn encoding_popup(&mut self, el: &ActiveEventLoop, ids: &[&'static str]) {
        let Some(window) = self.window.as_ref() else { return; };
        let Ok(origin) = window.inner_position() else { return; };
        let size = window.inner_size();
        let scale = window.scale_factor();
        let context = self.command_context();
        let commands: Vec<_> = ids.iter().map(|id| CommandId(id)).collect();
        let Some(platform) = self.platform.as_ref() else { return; };
        let result = platform.context_menu_in(
            origin.x + (size.width as i32 - (230.0 * scale) as i32).max(0),
            origin.y + (size.height as i32 - (24.0 * scale) as i32).max(0),
            &self.app.commands, &context, &self.settings.keymap.keymap, &commands,
        );
        match result {
            Ok(Some(action)) => self.dispatch(el, action),
            Ok(None) => {},
            Err(error) => if let Some(workspace) = &mut self.workspace { workspace.message = Some(format!("Encoding menu unavailable: {error}")); },
        }
    }
    pub(super) fn encoding_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("encoding.") { return false; }
        match id {
            "encoding.choose" => { self.encoding_popup(el, model::ROOT); return true; },
            "encoding.choose_interpret" => { self.encoding_popup(el, &model::CODECS.iter().map(|choice| choice.interpret).collect::<Vec<_>>()); return true; },
            "encoding.choose_convert" => { self.encoding_popup(el, &model::CODECS.iter().map(|choice| choice.convert).collect::<Vec<_>>()); return true; },
            "encoding.eol" => { self.encoding_popup(el, model::EOLS); return true; },
            "encoding.binary" => { self.encoding_popup(el, model::BINARY); return true; },
            _ => {},
        }
        let active = self.app.active;
        let result = (|| {
            if id == "encoding.failure" {
                let workspace = self.workspace.as_mut().ok_or("No document")?;
                let failure = workspace.encoding_failure(active).ok_or("No encoding save failure for this document")?;
                // The workspace owns the captured identity and rejects stale revisions
                // before selecting any bytes, including absolute Paged navigation.
                workspace.encoding_reveal_failure(active)?;
                workspace.message = Some(model::failure_description(failure.revision, failure.range, &failure.reason));
                return Ok(());
            }
            let state = self.workspace.as_ref().and_then(|workspace| workspace.encoding_state(active)).ok_or("No complete document encoding state")?;
            if id == "encoding.info" || id == "encoding.binary.info" { return Ok(()); }
            if let Some(choice) = model::CODECS.iter().find(|choice| choice.interpret == id) {
                let workspace = self.workspace.as_ref().ok_or("No document")?;
                let editor = workspace.editors.get(active).ok_or("Document closed")?;
                if editor.busy() { return Err("Wait for the current document operation".into()); }
                let confirmed = if editor.dirty() {
                    let name = workspace.path(active).map(|path| path.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
                    if !self.platform.as_ref().ok_or("Native confirmation unavailable")?.confirm_encoding_reinterpret(&name, choice.label) { return Ok(()); }
                    true
                } else { false };
                return self.workspace.as_mut().unwrap().encoding_interpret(active, choice.encoding, confirmed);
            }
            let workspace = self.workspace.as_mut().ok_or("No document")?;
            if let Some(choice) = model::CODECS.iter().find(|choice| choice.convert == id) {
                // Preserve deliberate BOM choice only for targets that support it.
                return workspace.encoding_convert(active, choice.encoding, state.bom && !choice.encoding.bom().is_empty());
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
        if let Err(error) = result && let Some(workspace) = &mut self.workspace { workspace.message = Some(error); }
        if let Some(window) = &self.window { window.request_redraw(); }
        true
    }
    pub(super) fn encoding_pump(&mut self, el: &ActiveEventLoop) {
        if let Some(workspace) = &mut self.workspace {
            let eol = workspace.encoding_eol_label(self.app.active);
            if let Some(editor) = workspace.editors.get_mut(self.app.active) {
                editor.set_eol_status_override(Some(eol));
            }
        }
        if let Some(workspace) = &mut self.workspace
            && let Some(state) = workspace.encoding_state(self.app.active)
            && let Some(editor) = workspace.editors.get_mut(self.app.active) {
            editor.encoding_label = if state.bom { format!("{} BOM", model::label(state.save_target)) } else { model::label(state.save_target).into() };
        }
        let Some(workspace) = self.workspace.as_ref() else { return; };
        let Some(editor) = workspace.editors.get(self.app.active) else { return; };
        if editor.busy() || !workspace.binary_warning_pending(self.app.active) { return; }
        let identity = bareline_app::accessibility::source_identity(editor);
        if self.encoding.warned == Some(identity) { return; }
        self.encoding.warned = Some(identity);
        // The workspace keeps this document read-only until an explicit decision.
        // Cancelling the popup therefore preserves the conservative open state.
        self.encoding_popup(el, model::BINARY);
    }
    pub(super) fn encoding_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if !matches!(event, WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }) { return false; }
        let Some(window) = &self.window else { return false; };
        let size = window.inner_size().to_logical::<f32>(window.scale_factor());
        if self.pointer.y < size.height - bareline_ui::STATUS_HEIGHT || self.pointer.y > size.height { return false; }
        if self.pointer.x >= size.width - 155.0 && self.pointer.x < size.width - 50.0 {
            self.encoding_popup(el, model::ROOT); true
        } else if self.pointer.x >= size.width - 240.0 && self.pointer.x < size.width - 155.0 {
            self.encoding_popup(el, model::EOLS); true
        } else { false }
    }
}
