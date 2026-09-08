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
pub fn status(editor: &crate::workspace::WorkspaceEditor, width: f64, height: f64) -> Vec<AccessibilityNode> {
    use crate::workspace::WorkspaceEditor;
    let (size, position, eol) = match editor {
        WorkspaceEditor::Resident(e) => {
            let snapshot = e.snapshot();
            let line = snapshot.line_at(bareline_document::TextOffset(e.selection.caret)).ok();
            let position = line.map(|line| {
                let column = snapshot.line_range(line).ok().and_then(|range| snapshot.read(range.start..bareline_document::TextOffset(e.selection.caret), MAX_ACCESSIBLE_TEXT_BYTES).ok()).map(|text| (text.graphemes(true).count()+1).to_string()).unwrap_or_else(|| "indexing".into());
                format!("Line {}, column {column}", line+1)
            }).unwrap_or_else(|| "Position unavailable".into());
            let size = if snapshot.is_complete() { format!("{} bytes, {} lines",snapshot.len(),snapshot.line_count()) } else { format!("{} bytes loaded, indexing",snapshot.len()) };
            (size,position,e.eol_status_label().to_owned())
        }
        WorkspaceEditor::Paged(e) => {
            let lines = match e.snapshot().line_count() { bareline_document::paged::LineCount::Known(count) => format!("{count} lines"), bareline_document::paged::LineCount::Unknown => "lines indexing".into() };
            (format!("{} bytes, {lines}",e.snapshot().len()), format!("Byte {}, line and column indexing",e.global_selection().1.0), e.surface.eol_status_label().to_owned())
        }
    };
    let values = [("Language",editor.language.label().to_owned()),("Document size",size),("Caret position",position),("Line endings",eol),("Encoding",editor.encoding_label.clone()),("Editing mode",if editor.read_only(){"Read only"}else{"Insert"}.into())];
    values.into_iter().enumerate().map(|(index,(name,value))| AccessibilityNode {
        id: 90_001_000+index as u64, parent: WINDOW_ID, role: AccessibilityRole::Status,
        name:name.into(),value:Some(value),bounds:[width*index as f64/6.0,(height-24.0).max(0.0),width/6.0,24.0],disabled:false,selected:false,expanded:None,focusable:false,invokable:false,
    }).collect()
}
/// The full source identity, including for a paged editor whose rendered surface
/// is only a local window. Recheck this immediately before applying queued UIA.
pub fn source_identity(editor: &crate::workspace::WorkspaceEditor) -> (u64, u64) {
    match editor {
        crate::workspace::WorkspaceEditor::Resident(editor) => {
            let snapshot = editor.snapshot();
            snapshot.identity_token()
        }
        crate::workspace::WorkspaceEditor::Paged(editor) => {
            let snapshot = editor.snapshot();
            snapshot.identity_token()
        }
    }
}
/// Immutable read bridge. Paged reads use one bounded shared worker, never UIA
/// or the UI thread. The single cached result is at most one text window.
pub fn text_source(editor: &crate::workspace::WorkspaceEditor, notify: std::sync::Arc<dyn Fn() + Send + Sync>) -> std::sync::Arc<dyn AccessibilityTextSource> {
    use crate::workspace::WorkspaceEditor;
    match editor {
        WorkspaceEditor::Resident(editor) => std::sync::Arc::new(ResidentText(editor.snapshot().clone())),
        WorkspaceEditor::Paged(editor) => std::sync::Arc::new(PagedText {
            handle: editor.read_handle(), state: Default::default(), notify,
        }),
    }
}
struct ResidentText(bareline_document::DocumentSnapshot);
impl AccessibilityTextSource for ResidentText {
    fn identity(&self) -> (u64, u64) { self.0.identity_token() }
    fn len(&self) -> usize { self.0.len() }
    fn read(&self, mut start: usize, limit: usize) -> AccessibleRead {
        use bareline_document::TextOffset;
        if start > self.len() || limit > MAX_ACCESSIBLE_TEXT_BYTES { return AccessibleRead::Unavailable; }
        let mut end = start.saturating_add(limit).min(self.len());
        while start < end && !self.0.is_boundary(TextOffset(start)) { start += 1; }
        while end > start && !self.0.is_boundary(TextOffset(end)) { end -= 1; }
        match self.0.read(TextOffset(start)..TextOffset(end), limit) {
            Ok(text) => AccessibleRead::Ready { start, text }, _ => AccessibleRead::Unavailable,
        }
    }
}
#[derive(Default)]
struct ReadState { pending: bool, cached: Option<(usize, usize, AccessibleRead)> }
struct PagedText {
    handle: bareline_editor_surface::paged_view::PagedReadHandle,
    state: std::sync::Arc<std::sync::Mutex<ReadState>>,
    notify: std::sync::Arc<dyn Fn() + Send + Sync>,
}
type ReadJob = Box<dyn FnOnce() + Send>;
fn text_worker() -> Option<&'static std::sync::mpsc::SyncSender<ReadJob>> {
    static WORKER: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<ReadJob>>> = std::sync::OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ReadJob>(8);
        std::thread::Builder::new().name("accessibility-read".into()).spawn(move || {
            while let Ok(job) = rx.recv() { job(); }
        }).ok().map(|_| tx)
    }).as_ref()
}
impl AccessibilityTextSource for PagedText {
    fn identity(&self) -> (u64,u64) { let s = self.handle.snapshot(); s.identity_token() }
    fn len(&self) -> usize { self.handle.snapshot().len() }
    fn read(&self, start: usize, limit: usize) -> AccessibleRead {
        if start > self.len() || limit > MAX_ACCESSIBLE_TEXT_BYTES { return AccessibleRead::Unavailable; }
        let Ok(mut state) = self.state.try_lock() else { return AccessibleRead::Pending; };
        if let Some((at, count, value)) = &state.cached {
            if *at == start && *count == limit { return value.clone(); }
        }
        if state.pending { return AccessibleRead::Pending; }
        let Some(worker) = text_worker() else { return AccessibleRead::Unavailable; };
        let weak = std::sync::Arc::downgrade(&self.state);
        let handle = self.handle.clone();
        let notify = self.notify.clone();
        state.pending = true;
        if worker.try_send(Box::new(move || {
            use bareline_document::{Budget, TextOffset, paged::WindowPoll};
            if weak.strong_count() == 0 { return; }
            let result = (|| {
                let mut request = handle.snapshot().begin_viewport(TextOffset(start), limit, &Budget::new(MAX_ACCESSIBLE_TEXT_BYTES)).ok()?;
                // One text window and a fixed page-resolution cap per job.
                for _ in 0..256 {
                    if weak.strong_count() == 0 { return None; }
                    match request.poll() {
                        WindowPoll::Ready(window) => return Some(AccessibleRead::Ready { start: window.range().start.0, text: window.text().to_owned() }),
                        WindowPoll::Pending(ticket) => match handle.resolve_page(ticket) {
                            Ok(true) => (), Ok(false) => return Some(AccessibleRead::Pending), Err(_) => return None,
                        },
                        _ => return None,
                    }
                }
                None
            })().unwrap_or(AccessibleRead::Unavailable);
            if let Some(state) = weak.upgrade() {
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                state.pending = false;
                if result != AccessibleRead::Pending { state.cached = Some((start, limit, result)); }
                drop(state);
                notify();
            }
        })).is_err() { state.pending = false; }
        AccessibleRead::Pending
    }
}
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
pub fn bounded_text(
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
        text_geometry: Vec::new(),
        text_context: editor.map(|e| AccessibilityTextContext {
            source_identity: e.snapshot().identity_token(),
            selection: (e.selection.anchor, e.selection.caret),
            composition: e.composition_text().filter(|s| s.len() <= MAX_ACCESSIBLE_TEXT_BYTES).map(str::to_owned),
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// These focused projection regressions inspect selected fields. Complete
    /// retained native hierarchy/focus/action JSON baselines live in the native
    /// composition tests, which exercise the actual owner layout fixtures.
    fn semantic_json(snapshot: &AccessibilitySnapshot) -> serde_json::Value {
        snapshot.validate().unwrap();
        serde_json::to_value(snapshot).unwrap()
    }
    #[test]
    fn default_editor_semantic_fields_regression() {
        let document=bareline_document::Document::from_utf8("abc",bareline_document::Budget::new(1<<20),bareline_document::Budget::new(1<<20)).unwrap();
        let mut editor=EditorSurface::loading(document.snapshot(),std::sync::Arc::new(||{}));
        editor.visible_text=bareline_document::TextOffset(0)..bareline_document::TextOffset(3);
        let tree=semantic_json(&snapshot("Bareline",800.0,600.0,Some(&editor),vec![],EDITOR_ID));
        let actual:Vec<_>=tree["nodes"].as_array().unwrap().iter().map(|n|serde_json::json!([n["id"],n["parent"],n["role"],n["name"],n["focusable"],n["invokable"]])).collect();
        assert_eq!(serde_json::Value::Array(actual),serde_json::json!([
            [1,0,"Window","Bareline",false,false],
            [2,1,"Editor","Editor",true,false],
            [18446744073709551614u64,1,"Button","Previous editor viewport",false,true],
            [18446744073709551613u64,1,"Button","Next editor viewport",false,true]
        ]));
        assert_eq!(tree["focus"],2);
        assert_eq!(tree["text"]["value"],"abc");
        assert_eq!(tree["text"]["character_lengths"],serde_json::json!([1,1,1]));
    }
    #[test]
    fn app_control_fields_regression() {
        let mut find=crate::find::FindController::default();find.show_replace();
        find.field.insert("needle");find.replacement.insert("replacement");
        let mut search=crate::search_panel::SearchPanel::default();search.open=true;search.field.insert("workspace");
        let mut settings=crate::settings::SettingsController::new(bareline_settings::SettingsDocument::empty(bareline_settings::Scope::User),None,bareline_settings::SystemAppearance::default());settings.show();
        let mut palette=crate::palette::PaletteController::default();palette.open=true;palette.field.insert("command");
        let mut manager=crate::macros::MacroManager::default();manager.show(&Default::default(),None);
        let semantics=find.semantics(1000.0).into_iter().chain(search.semantics()).chain(settings.semantics()).chain(palette.semantics()).chain(manager.semantics());
        let chrome=semantics.map(|n|semantic_node(&n,WINDOW_ID)).collect();
        let tree=semantic_json(&snapshot("Bareline",1000.0,800.0,None,chrome,11000));
        let mut fields:Vec<_>=tree["nodes"].as_array().unwrap().iter().filter(|n|n["role"]=="TextField").map(|n|serde_json::json!([n["id"],n["name"],n["value"],n["focusable"]])).collect();
        fields.sort_by_key(|row|row[0].as_u64().unwrap());
        assert_eq!(serde_json::Value::Array(fields),serde_json::json!([
            [6000,"Find","needle",true],[6001,"Replace with","replacement",true],
            [7000,"Find in open documents","workspace",true],
            [8000,"Search settings","",true],[11000,"Search commands","command",true],
            [23100,"Macro name","",true],[23101,"Repeat count","1",true],
            [23102,"Macro shortcut","",true],[23103,"Typing delay in milliseconds","50",true]
        ]));
        assert_eq!(tree["focus"],11000);
    }
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
