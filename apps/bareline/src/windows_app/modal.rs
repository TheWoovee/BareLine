// SPDX-License-Identifier: MPL-2.0
//! One owner for custom modal input, focus, dismissal, and accessibility.
use super::*;

pub(super) const RUN_GROUP_ID: u64 = 91_000_000;
pub(super) const RUN_FIELD_ID: u64 = 91_000_001;
pub(super) const RUN_STATUS_ID: u64 = 91_000_002;
pub(super) const RUN_SUBMIT_ID: u64 = 91_000_003;
pub(super) const RUN_CANCEL_ID: u64 = 91_000_004;
pub(super) const GOTO_GROUP_ID: u64 = 91_000_100;
pub(super) const GOTO_FIELD_ID: u64 = 91_000_101;
pub(super) const GOTO_STATUS_ID: u64 = 91_000_102;
pub(super) const GOTO_SUBMIT_ID: u64 = 91_000_103;
pub(super) const GOTO_CANCEL_ID: u64 = 91_000_104;
pub(super) const COMPARE_GROUP_ID: u64 = 90_000_025;
pub(super) const COMPARE_GENERAL_ID: u64 = 91_000_200;
pub(super) const COMPARE_COLORS_ID: u64 = 91_000_201;
pub(super) const COMPARE_DONE_ID: u64 = 91_000_202;
pub(super) const RECOVERY_GROUP_ID: u64 = 100_900;
pub(super) const RECOVERY_CLOSE_ID: u64 = 100_504;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModalSurface {
    Run,
    Goto,
    CompareOptions,
    Recovery,
    NotificationDetails,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutsideDismissal {
    Dismiss,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModalSemanticIds {
    pub group: u64,
    pub primary: u64,
    pub submit: u64,
    pub cancel: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ModalDescriptor {
    pub surface: ModalSurface,
    pub semantics: ModalSemanticIds,
    pub focused: u64,
    pub active_text_owner: Option<u64>,
    pub invoker: u64,
    pub outside_dismissal: OutsideDismissal,
    pub inert_background: bool,
    pub generation: u64,
    pub revision: u64,
}

impl ModalDescriptor {
    fn new(surface: ModalSurface, invoker: u64, generation: u64) -> Self {
        let (semantics, active_text_owner, outside_dismissal) = match surface {
            ModalSurface::Run => (
                ModalSemanticIds {
                    group: RUN_GROUP_ID,
                    primary: RUN_FIELD_ID,
                    submit: RUN_SUBMIT_ID,
                    cancel: RUN_CANCEL_ID,
                },
                Some(RUN_FIELD_ID),
                OutsideDismissal::Dismiss,
            ),
            ModalSurface::Goto => (
                ModalSemanticIds {
                    group: GOTO_GROUP_ID,
                    primary: GOTO_FIELD_ID,
                    submit: GOTO_SUBMIT_ID,
                    cancel: GOTO_CANCEL_ID,
                },
                Some(GOTO_FIELD_ID),
                OutsideDismissal::Dismiss,
            ),
            ModalSurface::CompareOptions => (
                ModalSemanticIds {
                    group: COMPARE_GROUP_ID,
                    primary: COMPARE_GENERAL_ID,
                    submit: COMPARE_DONE_ID,
                    cancel: COMPARE_DONE_ID,
                },
                None,
                OutsideDismissal::Block,
            ),
            ModalSurface::Recovery => (
                ModalSemanticIds {
                    group: RECOVERY_GROUP_ID,
                    primary: RECOVERY_CLOSE_ID,
                    submit: 100_500,
                    cancel: RECOVERY_CLOSE_ID,
                },
                None,
                OutsideDismissal::Block,
            ),
            ModalSurface::NotificationDetails => (
                ModalSemanticIds {
                    group: toast::DETAILS_GROUP_ID,
                    primary: toast::DETAILS_CLOSE_ID,
                    submit: toast::DETAILS_CLOSE_ID,
                    cancel: toast::DETAILS_CLOSE_ID,
                },
                None,
                OutsideDismissal::Block,
            ),
        };
        Self {
            surface,
            semantics,
            focused: semantics.primary,
            active_text_owner,
            invoker,
            outside_dismissal,
            inert_background: true,
            generation,
            revision: generation,
        }
    }
}

fn capture_modifiers(current: &mut ModifiersState, next: ModifiersState) {
    *current = next;
}

impl Shell {
    fn next_modal_identity(&mut self) -> u64 {
        self.modal_identity_serial = self
            .modal_identity_serial
            .checked_add(1)
            .expect("modal text identity space exhausted");
        self.modal_identity_serial
    }

    fn modal_invoker(&self) -> u64 {
        if let Some(id) = self.search_folder_accessibility_focus() {
            return id;
        }
        if let Some(workspace) = &self.workspace {
            if workspace.find.has_focus() {
                return workspace
                    .find
                    .semantics(1200.0)
                    .into_iter()
                    .find(|node| node.focused)
                    .map_or(6000, |node| node.id.0);
            }
            if workspace.search_focus && workspace.search_panel.open {
                return workspace.search_panel.accessibility_focus_id().unwrap_or(7000);
            }
        }
        self.ui_focus.focused().map_or_else(
            || {
                self.workspace.as_ref().map_or(2, |workspace| {
                    if workspace.find.has_focus() {
                        workspace
                            .find
                            .semantics(1200.0)
                            .into_iter()
                            .find(|node| node.focused)
                            .map_or(6000, |node| node.id.0)
                    } else if workspace.search_focus {
                        20000
                    } else {
                        2
                    }
                })
            },
            |id| id.0,
        )
    }

    pub(super) fn activate_modal(&mut self, surface: ModalSurface) {
        self.retire_partial_unicode_input();
        if self.modal.is_some() {
            self.dismiss_active_modal();
        }
        let invoker = self.modal_invoker();
        let identity = self.next_modal_identity();
        let descriptor = ModalDescriptor::new(surface, invoker, identity);
        self.ui_focus.set_targets(
            [invoker, 2]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|id| bareline_ui::focus::FocusTarget {
                    id: bareline_ui::ViewId(id),
                    enabled: true,
                })
                .collect(),
        );
        self.ui_focus.focus(bareline_ui::ViewId(invoker));
        self.ui_focus.open_layer(
            bareline_ui::ViewId(invoker),
            vec![bareline_ui::focus::FocusTarget {
                id: bareline_ui::ViewId(descriptor.semantics.primary),
                enabled: true,
            }],
        );
        self.modal = Some(descriptor);
    }

    pub(super) fn dismiss_modal(&mut self, surface: ModalSurface) -> bool {
        if self.modal.is_none_or(|modal| modal.surface != surface) {
            return false;
        }
        self.dismiss_active_modal();
        true
    }

    fn dismiss_active_modal(&mut self) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match modal.surface {
            ModalSurface::Run => {
                self.run_prompt.open = false;
                self.run_prompt.field.cancel();
                self.run_prompt.status.clear();
            }
            ModalSurface::Goto => {
                self.goto.open = false;
                self.goto.field.cancel();
                self.goto.status.clear();
            }
            ModalSurface::CompareOptions => {
                self.compare.options_open = false;
                self.compare.active_color = None;
                self.compare.color_focus = false;
                self.compare.color_field.cancel();
                self.compare.color_bounds = None;
            }
            ModalSurface::Recovery => {
                self.recovery.dismiss();
            }
            ModalSurface::NotificationDetails => self.toasts.close_details(),
        }
        if let Some(renderer) = &mut self.renderer {
            match modal.surface {
                ModalSurface::Run => self.run_prompt.field.release(renderer),
                ModalSurface::Goto => self.goto.field.release(renderer),
                ModalSurface::CompareOptions => self.compare.color_field.release(renderer),
                ModalSurface::Recovery => {}
                ModalSurface::NotificationDetails => {}
            }
        }
        self.ui_focus.close_layer();
        let restored = self.ui_focus.focused().map(|id| id.0).unwrap_or(modal.invoker);
        let mut restored_owner =
            (77_000..77_007).contains(&restored) && self.search_folder_accessibility(restored, false, None);
        if let Some(workspace) = &mut self.workspace {
            match restored {
                id if workspace.find.open
                    && workspace.find.semantics(1200.0).iter().any(|node| {
                        node.id.0 == id
                            && !node.disabled
                            && node.actions.contains(&bareline_ui::widgets::SemanticAction::Focus)
                    }) =>
                {
                    workspace.find.accessibility_action(restored, true);
                    workspace.search_focus = false;
                    restored_owner = true;
                }
                20000 if workspace.search_panel.open => {
                    workspace.find.blur();
                    workspace.search_focus = true;
                    workspace.search_panel.accessibility_focus(7000);
                    restored_owner = true;
                }
                id if workspace.search_panel.owns_accessibility_id(id) => {
                    workspace.find.blur();
                    workspace.search_focus = true;
                    workspace.search_panel.accessibility_focus(id);
                    restored_owner = true;
                }
                _ => {
                    workspace.find.blur();
                    workspace.search_focus = false;
                }
            }
        }
        if !restored_owner {
            self.ui_focus.focus(bareline_ui::ViewId(2));
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn modal_changed(&mut self, surface: ModalSurface) {
        let identity = self.next_modal_identity();
        if let Some(modal) = &mut self.modal
            && modal.surface == surface
        {
            modal.revision = identity;
        }
    }

    pub(super) fn modal_text_owner(&mut self, surface: ModalSurface, owner: Option<u64>) {
        let identity = self.next_modal_identity();
        if let Some(modal) = &mut self.modal
            && modal.surface == surface
        {
            modal.active_text_owner = owner;
            if let Some(owner) = owner {
                modal.focused = owner;
            }
            modal.revision = identity;
        }
    }

    pub(super) fn modal_focus(&mut self, surface: ModalSurface, id: u64) {
        if let Some(modal) = &mut self.modal
            && modal.surface == surface
        {
            modal.focused = id;
            modal.active_text_owner = match surface {
                ModalSurface::Run if id == RUN_FIELD_ID => Some(id),
                ModalSurface::Goto if id == GOTO_FIELD_ID => Some(id),
                ModalSurface::CompareOptions if id == 59_999 => Some(id),
                ModalSurface::Recovery => None,
                ModalSurface::NotificationDetails => None,
                _ => None,
            };
        }
    }

    pub(super) fn set_modal_accessibility_value(&mut self, id: u64, value: &str) -> bool {
        let Some(modal) = self.modal else {
            return false;
        };
        let changed = match modal.surface {
            ModalSurface::Run if self.run_prompt.open && id == RUN_FIELD_ID => {
                self.run_prompt.field.select_all();
                let changed = self.run_prompt.field.insert(value);
                self.run_prompt.status.clear();
                Some(changed)
            }
            ModalSurface::Goto if self.goto.open && id == GOTO_FIELD_ID => {
                self.goto.field.select_all();
                let changed = self.goto.field.insert(value);
                self.goto.status.clear();
                Some(changed)
            }
            _ => None,
        };
        if changed == Some(true) {
            self.modal_changed(modal.surface);
        }
        changed.is_some()
    }

    pub(super) fn modal_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        let Some(modal) = self.modal else {
            return false;
        };
        if matches!(event, WindowEvent::CloseRequested) {
            self.dismiss_modal(modal.surface);
            return false;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = event {
            capture_modifiers(&mut self.modifiers, modifiers.state());
        }
        if let WindowEvent::CursorMoved { position, .. } = event {
            let scale = self.window.as_ref().map_or(1.0, Window::scale_factor);
            let logical = position.to_logical::<f32>(scale);
            self.pointer = Point {
                x: logical.x,
                y: logical.y,
            };
        }
        if matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            }
        ) {
            let inside = match modal.surface {
                ModalSurface::Run => self.run_prompt.bounds.contains(self.pointer),
                ModalSurface::Goto => self.goto.bounds.contains(self.pointer),
                ModalSurface::CompareOptions => true,
                ModalSurface::Recovery => true,
                ModalSurface::NotificationDetails => self.toasts.details_contains(self.pointer),
            };
            if !inside {
                if modal.outside_dismissal == OutsideDismissal::Dismiss {
                    self.dismiss_modal(modal.surface);
                }
                return true;
            }
        }
        let handled = match modal.surface {
            ModalSurface::Run => self.run_prompt_event(el, event),
            ModalSurface::Goto => self.goto_event(el, event),
            ModalSurface::CompareOptions => self.compare_event(el, event),
            ModalSurface::Recovery => self.recovery_event(el, event),
            ModalSurface::NotificationDetails => {
                match event {
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && event.logical_key == Key::Named(NamedKey::Escape) =>
                    {
                        self.dismiss_modal(modal.surface);
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && (event.logical_key == Key::Named(NamedKey::Enter)
                                || matches!(&event.logical_key, Key::Character(value) if value == " ")) =>
                    {
                        self.dismiss_modal(modal.surface);
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(&event.logical_key, Key::Named(NamedKey::PageDown | NamedKey::ArrowDown)) =>
                    {
                        self.toasts.scroll_details(true);
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(&event.logical_key, Key::Named(NamedKey::PageUp | NamedKey::ArrowUp)) =>
                    {
                        self.toasts.scroll_details(false);
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let down = match delta {
                            MouseScrollDelta::LineDelta(_, y) => *y < 0.0,
                            MouseScrollDelta::PixelDelta(point) => point.y < 0.0,
                        };
                        self.toasts.scroll_details(down);
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if self.toasts.details_pointer_close(self.pointer) => {
                        self.dismiss_modal(modal.surface);
                    }
                    _ => {}
                }
                true
            }
        };
        if handled
            && let Some(focus) = match modal.surface {
                ModalSurface::CompareOptions => self.compare_accessibility_focus(),
                ModalSurface::Recovery => self.recovery_accessibility_focus(),
                ModalSurface::NotificationDetails => Some(toast::DETAILS_CLOSE_ID),
                _ => None,
            }
        {
            self.modal_focus(modal.surface, focus);
        }
        if handled
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_) | WindowEvent::MouseInput { .. }
            )
        {
            self.modal_changed(modal.surface);
        }
        handled
            || matches!(
                event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::Ime(_)
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::DroppedFile(_)
                    | WindowEvent::CursorMoved { .. }
                    | WindowEvent::ModifiersChanged(_)
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_modifier_capture_tracks_press_and_release() {
        let mut current = ModifiersState::empty();
        capture_modifiers(&mut current, ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert!(current.control_key());
        assert!(current.shift_key());
        capture_modifiers(&mut current, ModifiersState::empty());
        assert!(!current.control_key());
        assert!(!current.shift_key());
    }

    #[test]
    fn modal_instances_have_distinct_generations() {
        let first = ModalDescriptor::new(ModalSurface::Run, 2, 7);
        let second = ModalDescriptor::new(ModalSurface::Run, 2, 8);
        assert_ne!((first.generation, first.revision), (second.generation, second.revision));
    }

    #[test]
    fn notification_details_uses_the_shared_inert_modal_contract() {
        let modal = ModalDescriptor::new(ModalSurface::NotificationDetails, 2, 41);
        assert!(modal.inert_background);
        assert_eq!(modal.outside_dismissal, OutsideDismissal::Block);
        assert_eq!(modal.focused, toast::DETAILS_CLOSE_ID);
        assert_eq!(modal.semantics.group, toast::DETAILS_GROUP_ID);
        assert_eq!(modal.semantics.cancel, toast::DETAILS_CLOSE_ID);
    }

    #[test]
    fn notification_modal_close_returns_next_key_to_the_shell_owner() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.toasts.enqueue(
            toast::Notification::new(
                "modal-contract",
                1,
                bareline_ui::theme::ToastLevel::Error,
                toast::NotificationKind::Outcome,
                "Complete message",
                None,
                None,
                toast::NotificationLifetime::Persistent,
            ),
            std::time::Instant::now(),
        );
        shell.toasts.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            800.0,
            600.0,
            Default::default(),
            &mut Vec::new(),
        );
        let details = shell.toasts.accessibility()[0].id;
        assert!(shell.toasts.open_details(details));
        shell.activate_modal(ModalSurface::NotificationDetails);
        assert!(shell.dismiss_modal(ModalSurface::NotificationDetails));
        assert!(shell.modal.is_none());
        assert_eq!(shell.toasts.accessibility_focus(), None);
        assert!(
            shell.toasts.key(&Key::Named(NamedKey::Enter), false).is_none(),
            "the first editor key after modal close must not be captured by notifications"
        );
    }
}
