// SPDX-License-Identifier: MPL-2.0
use accesskit::{
    Action, ActionData, ActionHandler, ActivationHandler, Node, NodeId, Role, TextPosition,
    TextSelection, TreeInfo, TreeUpdate,
};
use accesskit_windows::SubclassingAdapter;
use bareline_platform::accessibility::*;
use std::sync::{Arc, Mutex};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IsWindow, IsWindowVisible},
};
mod text_provider;

fn tree(snapshot: &AccessibilitySnapshot) -> TreeUpdate {
    let mut nodes = Vec::new();
    for item in &snapshot.nodes {
        let role = match item.role {
            AccessibilityRole::Window => Role::Window,
            AccessibilityRole::Group => Role::Group,
            AccessibilityRole::Button => Role::Button,
            AccessibilityRole::Checkbox => Role::CheckBox,
            AccessibilityRole::Radio => Role::RadioButton,
            AccessibilityRole::Combo => Role::ComboBox,
            AccessibilityRole::List => Role::List,
            AccessibilityRole::ListItem => Role::ListItem,
            AccessibilityRole::TextField => Role::TextInput,
            AccessibilityRole::Slider => Role::Slider,
            AccessibilityRole::SpinButton => Role::SpinButton,
            AccessibilityRole::Separator => Role::Splitter,
            AccessibilityRole::Tooltip => Role::Tooltip,
            AccessibilityRole::Alert => Role::Alert,
            AccessibilityRole::Tab => Role::Tab,
            AccessibilityRole::Scrollbar => Role::ScrollBar,
            AccessibilityRole::Tree => Role::Tree,
            AccessibilityRole::TreeItem => Role::TreeItem,
            AccessibilityRole::Editor => Role::MultilineTextInput,
            AccessibilityRole::Status => Role::Status,
        };
        let mut node = Node::new(role);
        // AccessKit raises LiveRegionChanged when the accessible name changes.
        // Include status values in that name so progress/preedit updates speak.
        node.set_label(match (&item.role, &item.value) {
            (AccessibilityRole::Status | AccessibilityRole::Alert, Some(value)) => {
                format!("{}: {value}", item.name)
            }
            _ => item.name.clone(),
        });
        if let Some(value) = &item.value {
            node.set_value(value.clone());
        }
        let [x, y, w, h] = item.bounds;
        node.set_bounds(accesskit::Rect::new(x, y, x + w, y + h));
        if item.disabled {
            node.set_disabled();
        }
        if item.focusable {
            node.add_action(Action::Focus);
        }
        if item.invokable {
            node.add_action(Action::Click);
        }
        if item.role == AccessibilityRole::TextField && !item.disabled {
            node.add_action(Action::SetValue);
        }
        if item.role == AccessibilityRole::Tab || item.role == AccessibilityRole::ListItem {
            node.set_selected(item.selected);
        }
        if matches!(
            item.role,
            AccessibilityRole::Checkbox | AccessibilityRole::Radio
        ) {
            node.set_toggled(accesskit::Toggled::from(item.selected));
        }
        if let Some(expanded) = item.expanded {
            node.set_expanded(expanded);
        }
        if item.role == AccessibilityRole::Alert || item.role == AccessibilityRole::Status {
            node.set_live(accesskit::Live::Polite);
        }
        let mut children: Vec<_> = snapshot
            .nodes
            .iter()
            .filter(|n| n.id != item.id && n.parent == item.id)
            .map(|n| NodeId(n.id))
            .collect();
        if let Some(text) = snapshot.text.as_ref().filter(|t| t.editor_id == item.id) {
            children.extend(text_runs(text).iter().map(|r| r.id));
            if let Some((anchor, focus)) = text.selection {
                node.set_text_selection(TextSelection {
                    anchor: text_position(text, anchor),
                    focus: text_position(text, focus),
                });
                node.add_action(Action::SetTextSelection);
            }
        }
        node.set_children(children);
        nodes.push((NodeId(item.id), node));
    }
    if let Some(text) = &snapshot.text {
        for line in text_runs(text) {
            let mut run = Node::new(Role::TextRun);
            run.set_value(line.value.to_owned());
            run.set_character_lengths(line.lengths.to_vec());
            nodes.push((line.id, run));
        }
    }
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(NodeId(snapshot.root))),
        tree_id: accesskit::TreeId::ROOT,
        focus: NodeId(snapshot.focus),
    }
}
struct TextRun<'a> {
    id: NodeId,
    character_start: usize,
    byte_start: usize,
    value: &'a str,
    lengths: &'a [u8],
}
// Each hard line is a separate run, so UIA line units remain meaningful.
// Secondary run IDs occupy a reserved band below the high application IDs.
fn text_runs(text: &AccessibilityText) -> Vec<TextRun<'_>> {
    let mut result = Vec::new();
    let mut byte_start = 0;
    let mut character_start = 0;
    let mut end = 0;
    for (index, length) in text.character_lengths.iter().enumerate() {
        end += usize::from(*length);
        if text.value[..end].ends_with('\n') || index + 1 == text.character_lengths.len() {
            let id = if result.is_empty() {
                text.run_id
            } else {
                text.run_id.wrapping_sub(65536 + result.len() as u64)
            };
            result.push(TextRun {
                id: NodeId(id),
                character_start,
                byte_start,
                value: &text.value[byte_start..end],
                lengths: &text.character_lengths[character_start..=index],
            });
            byte_start = end;
            character_start = index + 1;
        }
    }
    if result.is_empty() || text.value.ends_with('\n') {
        let id = if result.is_empty() {
            text.run_id
        } else {
            text.run_id.wrapping_sub(65536 + result.len() as u64)
        };
        result.push(TextRun {
            id: NodeId(id),
            character_start,
            byte_start,
            value: "",
            lengths: &[],
        });
    }
    result
}
fn text_position(text: &AccessibilityText, index: usize) -> TextPosition {
    let runs = text_runs(text);
    let line = runs
        .iter()
        .rev()
        .find(|r| r.character_start <= index)
        .unwrap();
    TextPosition {
        node: line.id,
        character_index: index - line.character_start,
    }
}
fn validate(snapshot: &AccessibilitySnapshot) -> Result<(), &'static str> {
    snapshot.validate()?;
    if let Some(text) = &snapshot.text {
        let mut ids: std::collections::BTreeSet<_> =
            snapshot.nodes.iter().map(|n| NodeId(n.id)).collect();
        for run in text_runs(text) {
            if !ids.insert(run.id) {
                return Err("text run ID collides with chrome");
            }
        }
    }
    Ok(())
}
struct Shared {
    snapshot: AccessibilitySnapshot,
    actions: Vec<AccessibilityAction>,
}
struct Activate(Arc<Mutex<Shared>>);
impl ActivationHandler for Activate {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(tree(
            &self.0.lock().unwrap_or_else(|e| e.into_inner()).snapshot,
        ))
    }
}
struct Actions {
    shared: Arc<Mutex<Shared>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl ActionHandler for Actions {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        let mut state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if request.target_tree != accesskit::TreeId::ROOT {
            return;
        }
        let Some(target) = state
            .snapshot
            .nodes
            .iter()
            .find(|n| n.id == request.target_node.0 && !n.disabled)
        else {
            return;
        };
        let action = match request.action {
            Action::Focus if target.focusable => {
                Some(AccessibilityAction::Focus(request.target_node.0))
            }
            Action::Click if target.invokable => {
                Some(AccessibilityAction::Invoke(request.target_node.0))
            }
            Action::SetValue if target.role == AccessibilityRole::TextField => {
                if let Some(ActionData::Value(value)) = request.data {
                    (value.len() <= 16 * 1024).then(|| AccessibilityAction::SetValue {
                        id: target.id,
                        value: value.into(),
                    })
                } else {
                    None
                }
            }
            Action::SetTextSelection if target.role == AccessibilityRole::Editor => {
                if let (Some(text), Some(ActionData::SetTextSelection(selection))) =
                    (&state.snapshot.text, request.data)
                {
                    let offset = |position: TextPosition| {
                        let runs = text_runs(text);
                        let run = runs.iter().find(|r| r.id == position.node)?;
                        (position.character_index <= run.lengths.len()).then(|| {
                            text.start_byte
                                + run.byte_start
                                + run.lengths[..position.character_index]
                                    .iter()
                                    .map(|n| usize::from(*n))
                                    .sum::<usize>()
                        })
                    };
                    offset(selection.anchor)
                        .zip(offset(selection.focus))
                        .zip(state.snapshot.text_context.as_ref())
                        .map(|((anchor, caret), context)| AccessibilityAction::SetSelection { source_identity: context.source_identity, anchor, caret })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(action) = action.filter(|_| state.actions.len() < 256) {
            state.actions.push(action);
        }
        drop(state);
        (self.notify)();
    }
}
/// Drop before destroying the HWND. Construct on its owning thread before show.
pub struct WindowsAccessibility {
    adapter: SubclassingAdapter,
    shared: Arc<Mutex<Shared>>,
    _registration: accesskit_windows::PatternRegistration,
    text_provider: Arc<text_provider::Factory>,
}
impl WindowsAccessibility {
    /// # Safety
    /// `raw` is a live HWND owned by this thread and outlives this adapter.
    pub unsafe fn new(
        raw: isize,
        snapshot: AccessibilitySnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        validate(&snapshot)?;
        let hwnd = HWND(raw as *mut _);
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() || unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            return Err("accessibility requires a valid, hidden window");
        }
        let shared = Arc::new(Mutex::new(Shared {
            snapshot,
            actions: Vec::new(),
        }));
        let adapter = SubclassingAdapter::new(
            hwnd,
            Activate(shared.clone()),
            Actions {
                shared: shared.clone(),
                notify: notify.clone(),
            },
        );
        let text_provider = Arc::new(text_provider::Factory::new(shared.clone(), notify));
        let factory: Arc<dyn accesskit_windows::PatternOverride> = text_provider.clone();
        let registration = accesskit_windows::register_pattern_override(hwnd, &factory);
        Ok(Self { adapter, shared, _registration: registration, text_provider })
    }
    pub fn set_text_source(&mut self, source: Option<Arc<dyn AccessibilityTextSource>>) {
        self.text_provider.set_source(source);
    }
    pub fn update(&mut self, snapshot: AccessibilitySnapshot) {
        if let Err(reason) = validate(&snapshot) {
            eprintln!("event=accessibility_snapshot_rejected reason={reason}");
            return;
        }
        let old = {
            let mut state = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut state.snapshot, snapshot.clone())
        };
        if old == snapshot {
            return;
        }
        // This snapshot is already bounded to the visible semantic viewport.
        // Publish it atomically: text runs, their editor selection and dynamic
        // popup children must describe the same frame to native clients.
        if let Some(events) = self.adapter.update_if_active(|| tree(&snapshot)) {
            events.raise();
        }
    }
    pub fn drain_actions(&mut self) -> Vec<AccessibilityAction> {
        std::mem::take(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .actions,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> AccessibilitySnapshot {
        AccessibilitySnapshot {
            root: 1,
            focus: 2,
            text_geometry: Vec::new(),
            text_context: Some(AccessibilityTextContext { source_identity: (7, 3), selection: (5_000_000_001, 5_000_000_003), composition: None }),
            nodes: vec![
                AccessibilityNode {
                    id: 1,
                    parent: 0,
                    role: AccessibilityRole::Window,
                    name: "Bareline".into(),
                    value: None,
                    bounds: [0., 0., 800., 600.],
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: false,
                    invokable: false,
                },
                AccessibilityNode {
                    id: 2,
                    parent: 1,
                    role: AccessibilityRole::Editor,
                    name: "Editor".into(),
                    value: None,
                    bounds: [0., 0., 800., 600.],
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: true,
                    invokable: false,
                },
            ],
            text: Some(AccessibilityText {
                editor_id: 2,
                run_id: 3,
                start_byte: 5_000_000_000,
                value: "aé".into(),
                character_lengths: vec![1, 2],
                selection: Some((1, 2)),
            }),
        }
    }
    #[test]
    fn hard_lines_and_trailing_empty_line_keep_selection_identity() {
        let text = AccessibilityText {
            editor_id: 2,
            run_id: u64::MAX,
            start_byte: 100,
            value: "a\r\né\n".into(),
            character_lengths: vec![1, 2, 2, 1],
            selection: Some((2, 4)),
        };
        let lines = text_runs(&text);
        assert_eq!(
            lines.iter().map(|r| r.value).collect::<Vec<_>>(),
            vec!["a\r\n", "é\n", ""]
        );
        assert_eq!(
            text_position(&text, 2),
            TextPosition {
                node: lines[1].id,
                character_index: 0
            }
        );
        assert_eq!(
            text_position(&text, 4),
            TextPosition {
                node: lines[2].id,
                character_index: 0
            }
        );
        assert_eq!(lines[1].byte_start, 3);
    }
    #[test]
    fn provider_tree_exposes_bounded_text_and_caret() {
        let model = snapshot();
        assert!(model.validate().is_ok());
        let update = tree(&model);
        assert_eq!(update.nodes.len(), 3);
        assert_eq!(
            update.nodes[1]
                .1
                .text_selection()
                .unwrap()
                .focus
                .character_index,
            2
        );
        assert_eq!(update.nodes[2].1.value(), Some("aé"));
    }
    #[test]
    fn provider_actions_map_to_absolute_bytes_without_document_reads() {
        let shared = Arc::new(Mutex::new(Shared {
            snapshot: snapshot(),
            actions: vec![],
        }));
        let mut handler = Actions {
            shared: shared.clone(),
            notify: Arc::new(|| {}),
        };
        handler.do_action(accesskit::ActionRequest {
            action: Action::SetTextSelection,
            target_tree: accesskit::TreeId::ROOT,
            target_node: NodeId(2),
            data: Some(ActionData::SetTextSelection(TextSelection {
                anchor: TextPosition {
                    node: NodeId(3),
                    character_index: 1,
                },
                focus: TextPosition {
                    node: NodeId(3),
                    character_index: 2,
                },
            })),
        });
        assert_eq!(
            shared.lock().unwrap().actions,
            vec![AccessibilityAction::SetSelection {
                source_identity: (7, 3),
                anchor: 5_000_000_001,
                caret: 5_000_000_003
            }]
        );
    }
    #[test]
    fn invalid_native_handle_is_rejected_without_subclassing() {
        assert!(unsafe { WindowsAccessibility::new(0, snapshot(), Arc::new(|| {})) }.is_err());
        let mut model = snapshot();
        model.nodes[1].parent = 2;
        assert!(model.validate().is_err());
        let mut model = snapshot();
        model.text.as_mut().unwrap().selection = Some((0, 99));
        assert!(model.validate().is_err());
    }
}

/// Read the Windows accessibility setting without creating or changing a window.
pub fn high_contrast_enabled() -> std::io::Result<bool> {
    use windows::Win32::UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        WindowsAndMessaging::{
            SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        },
    };
    let mut state = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: state has the Windows-required size and lives through the call.
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            state.cbSize,
            Some((&mut state as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .map_err(|error| std::io::Error::from_raw_os_error(error.code().0 & 0xffff))?;
    Ok(state.dwFlags.contains(HCF_HIGHCONTRASTON))
}
