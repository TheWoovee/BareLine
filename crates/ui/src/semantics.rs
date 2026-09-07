// SPDX-License-Identifier: MPL-2.0
//! Bounded owner-composed semantic hierarchy, shared by native adapters and tests.
use crate::{
    ViewId,
    controls::ControlState,
    variable_list::{VariableItemSource, VariableList},
    widgets::{SemanticAction, SemanticRole, Semantics},
};
pub struct SemanticEntry {
    pub parent: ViewId,
    pub node: Semantics,
}
/// Emit only visible rows. Names and durable IDs are supplied independently of
/// painted labels, allowing localization and stable accessibility identities.
pub fn list(
    list: &crate::widgets::List,
    source: &impl crate::widgets::ItemSource,
    parent: ViewId,
    identity: impl Fn(usize) -> (ViewId, String),
    command: &str,
) -> Vec<SemanticEntry> {
    list.visible(source)
        .take(4096)
        .filter_map(|index| {
            let bounds = list.row_bounds(index);
            if bounds.y + bounds.height <= list.bounds.y
                || bounds.y >= list.bounds.y + list.bounds.height
            {
                return None;
            }
            let (id, name) = identity(index);
            let selected = list.selected == Some(index);
            let mut node = Semantics::new(
                id,
                SemanticRole::ListItem,
                &name,
                command,
                bounds,
                ControlState {
                    disabled: list.state.disabled || !source.enabled(index),
                    focused: list.state.focused && selected,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select)
            .action(SemanticAction::Invoke);
            node.selected = selected;
            Some(SemanticEntry { parent, node })
        })
        .collect()
}

pub fn radio_group(
    group: &crate::widgets::RadioGroup,
    source: &impl crate::widgets::ItemSource,
    parent: ViewId,
    identity: impl Fn(usize) -> (ViewId, String),
    command: &str,
) -> Vec<SemanticEntry> {
    let mut nodes = list(&group.list, source, parent, identity, command);
    for entry in &mut nodes {
        entry.node.role = SemanticRole::Radio;
        entry
            .node
            .actions
            .retain(|action| *action != SemanticAction::Invoke);
    }
    nodes
}

pub fn combo(
    combo: &crate::widgets::Combo,
    source: &impl crate::widgets::ItemSource,
    parent: ViewId,
    identity: impl Fn(usize) -> (ViewId, String),
    command: &str,
) -> Vec<SemanticEntry> {
    if combo.open {
        list(&combo.list, source, parent, identity, command)
    } else {
        Vec::new()
    }
}

pub fn tabs(
    strip: &crate::controls::TabStrip,
    parent: ViewId,
    identity: impl Fn(usize) -> (ViewId, String),
    command: &str,
    focused: bool,
) -> Vec<SemanticEntry> {
    strip
        .visible()
        .take(4096)
        .filter_map(|index| {
            let (id, name) = identity(index);
            let mut node = Semantics::new(
                id,
                SemanticRole::Tab,
                &name,
                command,
                strip.bounds(index)?,
                ControlState {
                    focused: focused && strip.active == index,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select);
            node.selected = strip.active == index;
            Some(SemanticEntry { parent, node })
        })
        .collect()
}
/// The caller supplies durable item IDs; virtual row indices are not identities.
pub fn variable_list(
    list: &VariableList,
    source: &impl VariableItemSource,
    parent: ViewId,
    id: impl Fn(usize) -> ViewId,
    command: &str,
    focused: bool,
) -> Vec<SemanticEntry> {
    list.visible(source, 0)
        .into_iter()
        .map(|row| {
            let selected = list.selected == Some(row.index);
            let mut node = Semantics::new(
                id(row.index),
                SemanticRole::ListItem,
                source.label(row.index),
                command,
                row.bounds,
                ControlState {
                    focused: focused && selected,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Select)
            .action(SemanticAction::Invoke);
            node.selected = selected;
            SemanticEntry { parent, node }
        })
        .collect()
}
