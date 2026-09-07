// SPDX-License-Identifier: MPL-2.0
//! UI-thread projection; UIA owns copies and cannot perform document I/O.
use bareline_editor_surface::EditorSurface;
use bareline_platform::accessibility::*;
use bareline_ui::widgets::{SemanticAction, SemanticRole, Semantics};
use unicode_segmentation::UnicodeSegmentation;
pub const MAX_ACCESSIBLE_TEXT_BYTES: usize = 64 * 1024;
pub const WINDOW_ID: u64 = 1;
pub const EDITOR_ID: u64 = 2;
pub const TEXT_RUN_ID: u64 = u64::MAX;
pub const PAGE_PREVIOUS_ID: u64 = u64::MAX - 1;
pub const PAGE_NEXT_ID: u64 = u64::MAX - 2;
pub const COMPOSITION_ID: u64 = u64::MAX - 3;
pub const EDITOR_ERROR_ID: u64 = u64::MAX - 4;
pub const TAB_ID_BASE: u64 = 1_000_000;
pub fn selection_valid(editor: &EditorSurface, anchor: usize, caret: usize) -> bool {
    editor
        .snapshot()
        .is_boundary(bareline_document::TextOffset(anchor))
        && editor
            .snapshot()
            .is_boundary(bareline_document::TextOffset(caret))
}

pub fn tabs(app: &crate::App, width: f32) -> Vec<AccessibilityNode> {
    let strip = bareline_ui::controls::TabStrip {
        width,
        count: app.tabs.len(),
        active: app.active,
    };
    strip
        .visible()
        .filter_map(|index| {
            let bounds = strip.bounds(index)?;
            let label = &app.tabs[index];
            let name = label
                .strip_suffix(" •")
                .map_or_else(|| label.clone(), |name| format!("{name}, modified"));
            Some(AccessibilityNode {
                id: TAB_ID_BASE + index as u64,
                parent: WINDOW_ID,
                role: AccessibilityRole::Tab,
                name,
                value: None,
                bounds: [
                    bounds.x as f64,
                    bounds.y as f64,
                    bounds.width as f64,
                    bounds.height as f64,
                ],
                disabled: false,
                selected: index == app.active,
                expanded: None,
                focusable: true,
                invokable: true,
            })
        })
        .collect()
}

pub fn semantic_node(value: &Semantics, parent: u64) -> AccessibilityNode {
    let role = match value.role {
        SemanticRole::Tree => AccessibilityRole::Tree,
        SemanticRole::TreeItem => AccessibilityRole::TreeItem,
        SemanticRole::Button => AccessibilityRole::Button,
        SemanticRole::Checkbox => AccessibilityRole::Checkbox,
        SemanticRole::Radio => AccessibilityRole::Radio,
        SemanticRole::Combo => AccessibilityRole::Combo,
        SemanticRole::List => AccessibilityRole::List,
        SemanticRole::ListItem => AccessibilityRole::ListItem,
        SemanticRole::TextField => AccessibilityRole::TextField,
        SemanticRole::Slider => AccessibilityRole::Slider,
        SemanticRole::SpinButton => AccessibilityRole::SpinButton,
        SemanticRole::Separator => AccessibilityRole::Separator,
        SemanticRole::Tooltip => AccessibilityRole::Tooltip,
        SemanticRole::Alert => AccessibilityRole::Alert,
        SemanticRole::Tab => AccessibilityRole::Tab,
        SemanticRole::Scrollbar => AccessibilityRole::Scrollbar,
        SemanticRole::Group => AccessibilityRole::Group,
    };
    AccessibilityNode {
        id: value.id.0,
        parent,
        role,
        name: value.name.clone(),
        value: value.value.clone(),
        bounds: [
            value.bounds.x as f64,
            value.bounds.y as f64,
            value.bounds.width as f64,
            value.bounds.height as f64,
        ],
        disabled: value.disabled,
        selected: value.selected,
        expanded: value.expanded,
        focusable: value.actions.contains(&SemanticAction::Focus),
        invokable: value.actions.iter().any(|a| {
            matches!(
                a,
                SemanticAction::Invoke | SemanticAction::Toggle | SemanticAction::Select
            )
        }),
    }
}
/// Copies only the range already established by editor layout. No line scan,
/// selection-wide read or access to source outside that range is possible here.
pub fn editor_text(editor: &EditorSurface) -> Option<AccessibilityText> {
    let range = editor.visible_text.clone();
    if range.end.0.saturating_sub(range.start.0) > MAX_ACCESSIBLE_TEXT_BYTES {
        return None;
    }
    let value = editor
        .snapshot()
        .read(range.clone(), MAX_ACCESSIBLE_TEXT_BYTES)
        .ok()?;
    bounded_text(
        value,
        range.start.0,
        editor.selection.anchor,
        editor.selection.caret,
    )
}
fn bounded_text(
    value: String,
    start_byte: usize,
    anchor: usize,
    caret: usize,
) -> Option<AccessibilityText> {
    if value.len() > MAX_ACCESSIBLE_TEXT_BYTES {
        return None;
    }
    // AccessKit lengths are u8; exceptionally long grapheme clusters cannot be
    // represented faithfully. Omit the range instead of splitting a grapheme.
    let character_lengths: Vec<u8> = value
        .graphemes(true)
        .map(|g| u8::try_from(g.len()).ok())
        .collect::<Option<_>>()?;
    let position = |byte: usize| {
        let relative = byte.checked_sub(start_byte)?;
        if relative > value.len() {
            return None;
        }
        if relative == value.len() {
            return Some(character_lengths.len());
        }
        value
            .grapheme_indices(true)
            .position(|(offset, _)| offset == relative)
    };
    let selection = position(anchor).zip(position(caret));
    Some(AccessibilityText {
        editor_id: EDITOR_ID,
        run_id: TEXT_RUN_ID,
        start_byte,
        value,
        character_lengths,
        selection,
    })
}
pub fn snapshot(
    title: &str,
    width: f64,
    height: f64,
    editor: Option<&EditorSurface>,
    mut chrome: Vec<AccessibilityNode>,
    focus: u64,
) -> AccessibilitySnapshot {
    let mut nodes = vec![AccessibilityNode {
        id: WINDOW_ID,
        parent: 0,
        role: AccessibilityRole::Window,
        name: title.into(),
        value: None,
        bounds: [0., 0., width, height],
        disabled: false,
        selected: false,
        expanded: None,
        focusable: false,
        invokable: false,
    }];
    let text = editor.and_then(editor_text);
    if let Some(editor) = editor {
        nodes.push(AccessibilityNode {
            id: EDITOR_ID,
            parent: WINDOW_ID,
            role: AccessibilityRole::Editor,
            name: "Editor".into(),
            value: None,
            bounds: [
                0.,
                editor.top_inset as f64,
                width,
                height - editor.top_inset as f64,
            ],
            disabled: false,
            selected: false,
            expanded: None,
            focusable: true,
            invokable: false,
        });
    }
    if let Some(editor) = editor {
        if let Some(error) = editor.error.as_ref() {
            nodes.push(AccessibilityNode {
                id: EDITOR_ERROR_ID,
                parent: WINDOW_ID,
                role: AccessibilityRole::Alert,
                name: error.clone(),
                value: None,
                bounds: [0., 0., 0., 0.],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            });
        }
        // Accessible viewport navigation is explicit: TextPattern describes
        // the present viewport, while Invoke requests the next bounded page.
        for (id, name) in [
            (PAGE_PREVIOUS_ID, "Previous editor viewport"),
            (PAGE_NEXT_ID, "Next editor viewport"),
        ] {
            nodes.push(AccessibilityNode {
                id,
                parent: WINDOW_ID,
                role: AccessibilityRole::Button,
                name: name.into(),
                value: None,
                bounds: [0., 0., 0., 0.],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: true,
            });
        }
        if let Some(preedit) = editor
            .composition_text()
            .filter(|s| s.len() <= MAX_ACCESSIBLE_TEXT_BYTES)
        {
            nodes.push(AccessibilityNode {
                id: COMPOSITION_ID,
                parent: WINDOW_ID,
                role: AccessibilityRole::Status,
                name: "IME composition".into(),
                value: Some(preedit.into()),
                bounds: [0., 0., 0., 0.],
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
            });
        }
    }
    nodes.append(&mut chrome);
    let focus = if nodes.iter().any(|n| n.id == focus) {
        focus
    } else {
        WINDOW_ID
    };
    AccessibilitySnapshot {
        root: WINDOW_ID,
        focus,
        nodes,
        text,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn find_semantics_follow_focus_toggle_and_field_value() {
        let mut find = crate::find::FindController::default();
        assert!(find.semantics(1200.0).is_empty());
        find.show_replace();
        find.field.insert("needle");
        find.case_sensitive = true;
        let nodes = find.semantics(1200.0);
        let field = nodes.iter().find(|n| n.id.0 == 6000).unwrap();
        assert!(field.focused);
        assert!(semantic_node(field, 1).focusable);
        assert_eq!(field.value.as_deref(), Some("needle"));
        assert!(
            nodes
                .iter()
                .any(|n| n.command_id == "search.match_case" && n.selected)
        );
        find.accessibility_action(6001, true);
        assert!(
            find.semantics(1200.0)
                .iter()
                .any(|n| n.id.0 == 6001 && n.focused)
        );
        find.hide();
        assert!(find.accessibility_action(6000, true).is_none());
        assert!(!find.has_focus());
    }
    #[test]
    fn grapheme_offsets_and_offscreen_selection_are_bounded() {
        let t = bounded_text("a👩‍💻e\u{301}\r\n".into(), 1_000, 1_001, 1_012).unwrap();
        assert_eq!(t.character_lengths, vec![1, 11, 3, 2]);
        assert_eq!(t.selection, Some((1, 2)));
        assert!(
            bounded_text("abc".into(), 1_000, 0, 2_000)
                .unwrap()
                .selection
                .is_none()
        );
        assert!(bounded_text("x".repeat(MAX_ACCESSIBLE_TEXT_BYTES + 1), 0, 0, 0).is_none());
    }
    #[test]
    fn find_replace_dynamic_tree_keeps_valid_focus_and_values() {
        let mut find = crate::find::FindController::default();
        find.show();
        for replacing in [false, true] {
            if replacing {
                find.show_replace();
            }
            find.field.insert("TODO");
            find.replacement.insert("DONE");
            let nodes = find.semantics(900.0);
            let focus = nodes.iter().find(|node| node.focused).unwrap().id.0;
            let snapshot = snapshot(
                "Bareline",
                900.0,
                600.0,
                None,
                nodes
                    .iter()
                    .map(|node| semantic_node(node, WINDOW_ID))
                    .collect(),
                focus,
            );
            snapshot.validate().unwrap();
            assert_eq!(snapshot.focus, 6000);
            assert!(
                snapshot
                    .nodes
                    .iter()
                    .find(|node| node.id == 6000)
                    .unwrap()
                    .focusable
            );
            assert_eq!(snapshot.nodes.iter().any(|node| node.id == 6001), replacing);
        }
    }
}
