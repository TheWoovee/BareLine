// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_commands::{CommandId, CommandRegistry, CommandSpec, CommandState};
use bareline_ui::controls::Key as UiKey;

#[derive(Default)]
pub(super) struct ToolbarRuntime {
    pub controller: bareline_app::toolbar::ToolbarController,
    applied: Option<Vec<String>>,
    theme: bareline_ui::theme::UiTheme,
}
pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("view.toolbar_toggle", "Show Toolbar"),
        ("view.toolbar_customize", "Customize Toolbar…"),
        ("view.toolbar_focus", "Focus Toolbar"),
    ] {
        registry
            .register(CommandSpec {
                id: CommandId(id),
                title,
                category: "View",
                shortcut: "",
                action: Action::Contributed(CommandId(id)),
            })
            .expect("unique toolbar command");
    }
}
impl ToolbarRuntime {
    pub fn draw(
        &mut self,
        _renderer: &mut WindowsRenderer,
        width: f32,
        height: f32,
        ops: &mut Vec<bareline_renderer::DrawOp>,
    ) -> Result<(), bareline_renderer::LayoutError> {
        self.controller.draw(width, height, self.theme, ops);
        Ok(())
    }
    pub fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        context.states.insert(
            CommandId("view.toolbar_toggle"),
            CommandState {
                checked: self.controller.model.visible,
                ..Default::default()
            },
        );
        if !self.controller.model.visible {
            context.states.insert(
                CommandId("view.toolbar_focus"),
                CommandState::disabled("Show the toolbar first"),
            );
        }
    }
}
impl Shell {
    pub(super) fn toolbar_refresh(&mut self) {
        let settings = self.settings.effective();
        self.toolbar.theme = self.settings.ui_theme();
        self.toolbar.controller.model.visible = settings.toolbar_visible;
        if self.toolbar.applied.as_ref() != Some(&settings.toolbar_commands) {
            let ids: Option<Vec<_>> = settings
                .toolbar_commands
                .iter()
                .map(|id| self.app.commands.lookup(id))
                .collect();
            if let Some(ids) = ids {
                let _ = self
                    .toolbar
                    .controller
                    .model
                    .set_commands(ids, &self.app.commands);
                self.toolbar.applied = Some(settings.toolbar_commands);
            } else {
                self.settings.controller.error = Some(
                    "Toolbar contains an unknown command. Correct toolbar.commands in settings."
                        .into(),
                );
            }
        }
        let context = self.command_context();
        let width = self
            .window
            .as_ref()
            .map(|w| w.inner_size().to_logical::<f32>(w.scale_factor()).width)
            .unwrap_or(800.);
        self.toolbar.controller.refresh(
            &self.app.commands,
            &context,
            &self.settings.keymap.keymap,
            width,
        );
    }
    pub(super) fn toolbar_save(&mut self) {
        let commands: Vec<_> = self
            .toolbar
            .controller
            .model
            .commands
            .iter()
            .map(|id| id.0.to_string())
            .collect();
        if self.toolbar.applied.as_ref() == Some(&commands) {
            return;
        }
        let scope = self.settings.controller.scope;
        self.settings.controller.scope = bareline_settings::Scope::User;
        let result = self.settings.controller.edit(
            "toolbar.commands",
            bareline_settings::SettingValue::Strings(commands.clone()),
        );
        self.settings.controller.scope = scope;
        match result {
            Ok(()) => {
                self.toolbar.applied = Some(commands);
                self.toolbar.controller.status = None;
            }
            Err(error) => {
                self.toolbar.controller.status = Some(error.clone());
                self.settings.controller.error = Some(error);
            }
        }
    }
    pub(super) fn toolbar_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "view.toolbar_toggle" => {
                let visible = !self.settings.effective().toolbar_visible;
                let scope = self.settings.controller.scope;
                self.settings.controller.scope = bareline_settings::Scope::User;
                if let Err(error) = self.settings.controller.edit(
                    "toolbar.visible",
                    bareline_settings::SettingValue::Bool(visible),
                ) {
                    self.settings.controller.error = Some(error);
                }
                self.settings.controller.scope = scope;
            }
            "view.toolbar_customize" => {
                self.palette.dismiss();
                self.toolbar.controller.customize(&self.app.commands);
            }
            "view.toolbar_focus" => self.toolbar.controller.focus(),
            _ => return false,
        }
        self.toolbar_refresh();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
        true
    }
    pub(super) fn toolbar_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        // Higher transient layers keep their own input and Escape behavior.
        if self.palette.open
            || (self.settings.controller.open && !self.toolbar.controller.customizing)
        {
            return false;
        }
        let mut command = None;
        let previous_commands = self.toolbar.controller.model.commands.clone();
        let consumed = match event {
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(w) = &self.window {
                    let p = position.to_logical::<f32>(w.scale_factor());
                    self.pointer = Point { x: p.x, y: p.y };
                }
                self.toolbar.controller.customizing
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if self.toolbar.controller.customizing
                    || self.pointer.y < self.toolbar.controller.height()
                {
                    command = self
                        .toolbar
                        .controller
                        .pointer(self.pointer, *state == ElementState::Pressed);
                    true
                } else {
                    if self.toolbar.controller.focused() {
                        self.toolbar.controller.dismiss();
                    }
                    false
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if self.toolbar.controller.focused() && event.state == ElementState::Pressed =>
            {
                let key = match event.logical_key {
                    Key::Named(NamedKey::Escape) => Some(UiKey::Escape),
                    Key::Named(NamedKey::Enter) => Some(UiKey::Enter),
                    Key::Named(NamedKey::Space) => Some(UiKey::Space),
                    Key::Named(NamedKey::Tab) => Some(UiKey::Tab),
                    Key::Named(NamedKey::ArrowLeft) => Some(UiKey::Left),
                    Key::Named(NamedKey::ArrowRight) => Some(UiKey::Right),
                    Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up),
                    Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down),
                    Key::Named(NamedKey::Home) => Some(UiKey::Home),
                    Key::Named(NamedKey::End) => Some(UiKey::End),
                    _ => None,
                };
                if let Some(key) = key {
                    command = self.toolbar.controller.key(key, self.modifiers.shift_key());
                    true
                } else {
                    // Unowned global shortcuts keep the normal registry routing.
                    !self.modifiers.control_key() && !self.modifiers.alt_key()
                }
            }
            WindowEvent::Ime(_) if self.toolbar.controller.focused() => true,
            WindowEvent::Focused(false) => {
                self.toolbar.controller.dismiss();
                false
            }
            _ => false,
        };
        if consumed {
            if previous_commands != self.toolbar.controller.model.commands {
                self.toolbar_save();
            }
            self.toolbar_refresh();
            if let Some(id) = command
                && let Ok(action) = self.app.commands.dispatch_in(id, &self.command_context())
            {
                self.toolbar.controller.dismiss();
                self.dispatch(el, action);
            }
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        consumed
    }
}
