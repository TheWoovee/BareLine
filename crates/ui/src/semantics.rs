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
            if bounds.y + bounds.height <= list.bounds.y || bounds.y >= list.bounds.y + list.bounds.height {
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
            node.position_in_set = Some(index + 1);
            node.size_of_set = source.len();
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
        entry.node.actions.retain(|action| *action != SemanticAction::Invoke);
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

/// Every tab is exposed, including tabs scrolled out of the strip, which keep
/// their set position but have no visible bounds.
pub fn tabs(
    strip: &crate::controls::TabStrip,
    parent: ViewId,
    identity: impl Fn(usize) -> (ViewId, String),
    command: &str,
    focused: bool,
) -> Vec<SemanticEntry> {
    (0..strip.count)
        .take(4096)
        .map(|index| {
            let (id, name) = identity(index);
            let mut node = Semantics::new(
                id,
                SemanticRole::Tab,
                &name,
                command,
                strip.bounds(index).unwrap_or_default(),
                ControlState {
                    focused: focused && strip.active == index,
                    ..Default::default()
                },
            )
            .action(SemanticAction::Focus)
            .action(SemanticAction::Select);
            node.selected = strip.active == index;
            node.position_in_set = Some(index + 1);
            node.size_of_set = Some(strip.count);
            SemanticEntry { parent, node }
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
            node.position_in_set = Some(row.index + 1);
            node.size_of_set = source.len();
            SemanticEntry { parent, node }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tabs_expose_every_tab_with_its_set_position() {
        // Two tabs fit; the active last tab scrolls the first three off.
        let strip = crate::controls::TabStrip {
            width: 300.0,
            count: 5,
            active: 4,
        };
        let nodes = tabs(
            &strip,
            ViewId(1),
            |index| (ViewId(100 + index as u64), format!("Tab {index}")),
            "view.tab",
            true,
        );
        assert_eq!(nodes.len(), 5);
        for (index, entry) in nodes.iter().enumerate() {
            assert_eq!(entry.node.position_in_set, Some(index + 1));
            assert_eq!(entry.node.size_of_set, Some(5));
        }
        assert_eq!(nodes[0].node.bounds.width, 0.0);
        assert!(nodes[4].node.bounds.width > 0.0 && nodes[4].node.selected);
    }
}
