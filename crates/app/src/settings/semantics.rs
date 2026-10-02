// SPDX-License-Identifier: MPL-2.0
//! Settings accessibility: the semantic tree the page exposes, focus order
//! derived from it, and the actions assistive technology can invoke.
use super::*;
impl SettingsController {
    /// Replaces the committed value of an exposed text field without applying a setting.
    pub fn accessibility_set_value(&mut self, id: u64, value: &str) -> bool {
        if !self.open || !matches!(id, 8000 | 8007) || value.len() > 16 * 1024 || value.chars().any(char::is_control) {
            return false;
        }
        if !self.semantics().iter().any(|node| node.id.0 == id && !node.disabled) {
            return false;
        }
        self.accessibility_action(id, false);
        let Some(field) = self.text_field_mut() else {
            return false;
        };
        field.cancel();
        field.select_all();
        field.insert(value);
        self.text_changed();
        true
    }
    pub fn focused_id(&self) -> Option<ViewId> {
        if !self.open {
            return None;
        }
        if self.reset_pending {
            return self.focus.focused();
        }
        if self.value_edit.is_some() {
            return self.focus.focused();
        }
        if let Some(popup) = &self.popup {
            return popup.list.selected.map(|i| ViewId(8500 + i as u64));
        }
        if self.query_focused {
            Some(ViewId(8000))
        } else {
            self.focus.focused()
        }
    }
    pub fn traverse_focus(&mut self, backwards: bool) {
        if self.value_edit.is_some() {
            self.focus.traverse(backwards);
            return;
        }
        if self.reset_pending {
            self.focus.traverse(backwards);
            return;
        }
        if self.popup.is_some() {
            self.popup = None;
        }
        self.refresh_focus();
        if let Some(id) = self.focus.traverse(backwards) {
            self.accessibility_action(id.0, false);
        }
    }
    fn refresh_focus(&mut self) {
        let mut nodes = self.semantics();
        nodes.sort_by_key(|node| match node.id.0 {
            8000 => (0, 0),
            8001 | 8002 | 8006 => (1, node.id.0),
            8100..=8199 => (2, node.id.0),
            2000..=3999 => (3, node.id.0),
            _ => (4, node.id.0),
        });
        let targets = nodes
            .into_iter()
            .filter(|node| node.actions.contains(&SemanticAction::Focus))
            .map(|node| bareline_ui::focus::FocusTarget {
                id: node.id,
                enabled: !node.disabled,
            })
            .collect();
        self.focus.set_targets(targets);
        if self.query_focused {
            self.focus.focus(ViewId(8000));
        }
    }
    pub fn semantics(&self) -> Vec<Semantics> {
        if !self.open {
            return Vec::new();
        }
        if self.reset_pending {
            let dialog = self.reset_dialog_bounds();
            return [
                (8011, "Reset section", dialog.x + 12.0),
                (8012, "Cancel reset", dialog.x + 92.0),
            ]
            .into_iter()
            .map(|(id, name, x)| {
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    &self.label(
                        if id == 8011 {
                            "settings.reset_section"
                        } else {
                            "settings.cancel"
                        },
                        name,
                    ),
                    "settings.reset_section",
                    rect(x, dialog.y + 76.0, 72.0, 28.0),
                    ControlState {
                        focused: self.focus.focused() == Some(ViewId(id)),
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke)
            })
            .collect();
        }
        if let Some(edit) = &self.value_edit {
            let name = config::DEFINITIONS
                .iter()
                .find(|d| d.key == edit.key)
                .map_or(edit.key, |d| d.title);
            let mut nodes = vec![edit.field.semantics(
                ViewId(8007),
                name,
                "settings.edit_value",
                edit.bounds,
                ControlState {
                    focused: self.focus.focused() == Some(ViewId(8007)),
                    ..Default::default()
                },
            )];
            for (id, label, x) in [
                (8009, "Apply value", edit.bounds.x),
                (8010, "Cancel value edit", edit.bounds.x + 80.0),
            ] {
                nodes.push(
                    Semantics::new(
                        ViewId(id),
                        SemanticRole::Button,
                        &self.label(
                            if id == 8009 {
                                "settings.apply"
                            } else {
                                "settings.cancel"
                            },
                            label,
                        ),
                        "settings.edit_value",
                        rect(x, edit.bounds.y + edit.bounds.height + 24.0, 72.0, 28.0),
                        ControlState {
                            focused: self.focus.focused() == Some(ViewId(id)),
                            ..Default::default()
                        },
                    )
                    .action(SemanticAction::Focus)
                    .action(SemanticAction::Invoke),
                );
            }
            return nodes;
        }
        let mut nodes = vec![self.query.semantics(
            ViewId(8000),
            &self.label("settings.search", "Search settings"),
            "settings.search",
            self.search_bounds,
            ControlState {
                focused: self.query_focused && self.popup.is_none(),
                ..Default::default()
            },
        )];
        // The header's pointer-only controls are UIA buttons too (A11Y-05).
        for (id, label, command, bounds) in [
            (8024, "Close settings", "settings.close", self.close_button),
            (8025, "Open settings.toml", "settings.open_toml", self.open_toml),
        ] {
            nodes.push(
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    &self.label(command, label),
                    command,
                    bounds,
                    ControlState {
                        focused: self.focus.focused() == Some(ViewId(id)),
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        for (id, label, command, bounds) in [
            (8001, "User settings", "settings.scope_user", self.scope_user),
            (
                8002,
                "Workspace settings",
                "settings.scope_workspace",
                self.scope_workspace,
            ),
            (8003, "Reset section", "settings.reset_section", self.reset),
            (8004, "Revert changes", "settings.revert", self.revert),
        ] {
            let revert = id == 8004;
            let disabled = revert && !self.can_revert();
            let label = if revert {
                let scope = if self.scope == Scope::Workspace {
                    "Workspace"
                } else {
                    "User"
                };
                if disabled {
                    format!("Revert unavailable: {scope} settings match the values from when Settings opened")
                } else {
                    format!("Revert {scope} settings to the values from when Settings opened")
                }
            } else {
                self.label(command, label)
            };
            nodes.push(
                Semantics::new(
                    ViewId(id),
                    SemanticRole::Button,
                    &label,
                    command,
                    bounds,
                    ControlState {
                        disabled,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        if let Some(change) = &self.external_change {
            let scope = if change.scope == Scope::Workspace {
                "Workspace"
            } else {
                "User"
            };
            for (id, label, command, bounds) in [
                (
                    8013,
                    format!("Reload {scope} settings from disk and discard my current changes"),
                    "settings.external_reload",
                    self.external_reload,
                ),
                (
                    8014,
                    format!("Keep my {scope} settings and replace the changed disk file"),
                    "settings.external_keep",
                    self.external_keep,
                ),
            ] {
                nodes.push(
                    Semantics::new(
                        ViewId(id),
                        SemanticRole::Button,
                        &label,
                        command,
                        bounds,
                        ControlState::default(),
                    )
                    .action(SemanticAction::Focus)
                    .action(SemanticAction::Invoke),
                );
            }
        }
        if matches!(self.current().status, SaveStatus::Failed(_)) {
            nodes.push(
                Semantics::new(
                    ViewId(8005),
                    SemanticRole::Button,
                    &self.label("settings.retry", "Retry saving"),
                    "settings.retry",
                    self.retry,
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        for (index, category) in CATEGORIES.iter().enumerate() {
            let mut node = Semantics::new(
                ViewId(8100 + index as u64),
                SemanticRole::ListItem,
                &self.label(&format!("settings.category.{category}"), category),
                "settings.category",
                rect(
                    self.bounds.x,
                    self.bounds.y + index as f32 * 38.0,
                    164.0_f32.min(self.bounds.width * 0.26),
                    38.0,
                ),
                ControlState::default(),
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Invoke);
            node.selected = self.category == *category;
            nodes.push(node);
        }
        if self.scope == Scope::Workspace {
            nodes.push(
                Semantics::new(
                    ViewId(8006),
                    SemanticRole::Checkbox,
                    &self.label("settings.workspace_opt_in", "Enable workspace preferences"),
                    "settings.workspace_opt_in",
                    self.opt_in,
                    ControlState {
                        checked: self.workspace_opted_in,
                        ..Default::default()
                    },
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        if self.theme_cards_visible() {
            let mode = self.theme_mode_value();
            for (index, (title, value)) in THEME_CARDS.iter().enumerate() {
                let mut node = Semantics::new(
                    ViewId(8020 + index as u64),
                    SemanticRole::ListItem,
                    &self.label(&format!("settings.theme.{value}"), title),
                    "theme.mode",
                    self.theme_cards[index],
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke);
                node.selected = mode == *value;
                nodes.push(node);
            }
        }
        if self.restart_pending {
            nodes.push(
                Semantics::new(
                    ViewId(8023),
                    SemanticRole::Button,
                    &self.label("settings.restart_now", "Restart now"),
                    "settings.restart_now",
                    self.restart_now,
                    ControlState::default(),
                )
                .action(SemanticAction::Focus)
                .action(SemanticAction::Invoke),
            );
        }
        for row in &self.rows {
            let mut value = row.value.semantic().into_settings(row.value.bounds);
            value.name = self.label(&format!("setting.{}.title", row.definition.key), row.definition.title);
            value.focused = !self.query_focused
                && self.popup.is_none()
                && self
                    .rows
                    .iter()
                    .position(|r| r.value.id == row.value.id)
                    .is_some_and(|index| self.first + index == self.selected);
            value.value = Some(self.value_text(row.definition.key));
            // Booleans are switches, not pickers: a check box with its state.
            if matches!(row.definition.kind, SettingKind::Boolean) {
                value.role = SemanticRole::Checkbox;
                value.selected = matches!(
                    self.effective().setting_value(row.definition.key),
                    Some(SettingValue::Bool(true))
                );
                value.value = None;
            }
            nodes.push(value);
            nodes.push(
                Semantics::new(
                    row.copy.id,
                    SemanticRole::Button,
                    &row.copy.label,
                    "settings.copy_key",
                    row.copy.bounds,
                    row.copy.state,
                )
                .action(SemanticAction::Invoke),
            );
        }
        if let Some(popup) = &self.popup {
            nodes.extend(
                bareline_ui::semantics::list(
                    &popup.list,
                    popup,
                    ViewId(1),
                    |i| (ViewId(8500 + i as u64), popup.labels[i].clone()),
                    "settings.choose",
                )
                .into_iter()
                .map(|entry| entry.node),
            );
        }
        nodes
    }
    pub fn accessibility_action(&mut self, id: u64, invoke: bool) -> Option<SettingsEffect> {
        if !self.open {
            return None;
        }
        let node = self
            .semantics()
            .into_iter()
            .find(|node| node.id.0 == id && !node.disabled)?;
        if !invoke {
            self.focus.focus(ViewId(id));
            self.query_focused = id == 8000;
            if let Some(index) = self
                .rows
                .iter()
                .position(|row| row.value.id.0 == id || row.copy.id.0 == id)
            {
                self.selected = self.first + index;
            }
            if let Some(popup) = &mut self.popup {
                if let Some(index) = id.checked_sub(8500).filter(|i| *i < popup.labels.len() as u64) {
                    popup.list.selected = Some(index as usize);
                }
            }
            return None;
        }
        if self.reset_pending {
            self.confirm_reset(id == 8011);
            return Some(SettingsEffect::PreviewChanged);
        }
        if self.value_edit.is_some() {
            return match id {
                8009 => self.finish_value_edit(true),
                8010 => self.finish_value_edit(false),
                _ => None,
            };
        }
        let point = Point {
            x: node.bounds.x + node.bounds.width / 2.0,
            y: node.bounds.y + node.bounds.height / 2.0,
        };
        let down = self.event(UiEvent::PointerDown(point));
        self.event(UiEvent::PointerUp(point)).or(down)
    }
}
trait SettingSemantic {
    fn into_settings(self, bounds: Rect) -> Semantics;
}
impl SettingSemantic for bareline_ui::controls::SemanticNode {
    fn into_settings(self, bounds: Rect) -> Semantics {
        Semantics::new(
            self.id,
            SemanticRole::Combo,
            &self.name,
            "settings.change",
            bounds,
            ControlState {
                disabled: self.disabled,
                focused: self.focused,
                ..Default::default()
            },
        )
        .action(SemanticAction::Focus)
        .action(SemanticAction::Invoke)
    }
}
